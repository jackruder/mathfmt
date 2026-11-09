//! The scanner and the math parser (design §5 and §6), one recursive descent
//! over two modes.
//!
//! Text mode finds math regions, skips comments and verbatim, and records
//! macro definitions. Math mode builds a tree of tokens, groups and commands
//! with their arguments; a text argument (`\text{…}`) is a nested text-mode
//! scan, so `$a \text{ if $b$ } c$` nests correctly. Everything is positioned
//! by byte span into the one source string, and nothing is copied out of it:
//! the output is produced as edits on the original bytes.

use std::collections::HashSet;

use crate::diag::Diagnostic;
use crate::lex::{lex_one, Span, TokKind, Token};
use crate::macros::{opaque_reason, Macro, MacroKind, MacroTable};
use crate::signatures::{ArgKind, Signature, Signatures};

/// What the scanner treats as math, verbatim, or a URL argument.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub math_environments: HashSet<String>,
    pub verbatim_environments: HashSet<String>,
    /// Commands whose first brace argument is skipped unscanned (`\url`).
    pub url_commands: HashSet<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        let set = |names: &[&str]| names.iter().map(|s| s.to_string()).collect();
        ScanOptions {
            math_environments: set(&[
                "equation",
                "equation*",
                "align",
                "align*",
                "alignat",
                "alignat*",
                "gather",
                "gather*",
                "multline",
                "multline*",
                "flalign",
                "flalign*",
                "eqnarray",
                "eqnarray*",
                "displaymath",
                "math",
            ]),
            verbatim_environments: set(&[
                "verbatim",
                "verbatim*",
                "lstlisting",
                "minted",
                "comment",
                "Verbatim",
            ]),
            url_commands: set(&["url", "href", "path", "nolinkurl", "lstinline"]),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionKind {
    Inline,
    Display,
    Env(String),
    Ensuremath,
}

#[derive(Debug)]
pub struct MathRegion {
    pub kind: RegionKind,
    /// The opening delimiter: `$`, `$$`, `\(`, `\[`, `\begin{env}`, `\ensuremath{`.
    pub open: Span,
    /// The closing delimiter, when found.
    pub close: Option<Span>,
    pub nodes: Vec<Node>,
}

impl MathRegion {
    /// The bytes between the delimiters.
    pub fn body(&self) -> Span {
        let start = self.open.end;
        let end = match self.close {
            Some(c) => c.start,
            None => self.nodes.last().map(|n| n.span().end).unwrap_or(start),
        };
        Span::new(start, end.max(start))
    }

    pub fn span(&self) -> Span {
        Span::new(
            self.open.start,
            self.close.map(|c| c.end).unwrap_or(self.body().end),
        )
    }
}

#[derive(Debug)]
pub enum Node {
    Leaf(Token),
    Group(Group),
    Command(Command),
}

#[derive(Debug)]
pub struct Group {
    pub open: Token,
    pub children: Vec<Node>,
    pub close: Option<Token>,
}

#[derive(Debug)]
pub struct Command {
    pub name: Token,
    pub star: Option<Token>,
    pub args: Vec<Arg>,
    /// Byte offset just past the last argument.
    pub end: usize,
}

#[derive(Debug)]
pub enum Arg {
    Math(Node),
    Text(TextArg),
    Optional(Span),
    Delim(Node),
}

#[derive(Debug)]
pub struct TextArg {
    pub open: Token,
    pub regions: Vec<MathRegion>,
    pub close: Option<Token>,
}

impl Node {
    pub fn span(&self) -> Span {
        match self {
            Node::Leaf(t) => t.span,
            Node::Group(g) => {
                let end = g
                    .close
                    .map(|c| c.span.end)
                    .or_else(|| g.children.last().map(|n| n.span().end))
                    .unwrap_or(g.open.span.end);
                Span::new(g.open.span.start, end)
            }
            Node::Command(c) => Span::new(c.name.span.start, c.end),
        }
    }
}

impl Arg {
    pub fn span(&self) -> Span {
        match self {
            Arg::Math(n) | Arg::Delim(n) => n.span(),
            Arg::Optional(s) => *s,
            Arg::Text(t) => {
                let end = t
                    .close
                    .map(|c| c.span.end)
                    .or_else(|| t.regions.last().map(|r| r.span().end))
                    .unwrap_or(t.open.span.end);
                Span::new(t.open.span.start, end)
            }
        }
    }
}

/// A scanned file: its math regions, the macro table after its definitions,
/// and what the scanner had to say.
#[derive(Debug)]
pub struct Document {
    pub regions: Vec<MathRegion>,
    pub macros: MacroTable,
    pub diagnostics: Vec<Diagnostic>,
}

/// Scan a whole file. `macros` are the definitions already in force (from the
/// config's definition files); the file's own definitions are added.
pub fn scan(src: &str, sigs: &Signatures, opts: &ScanOptions, macros: &MacroTable) -> Document {
    let mut p = Parser {
        src,
        pos: 0,
        sigs,
        opts,
        base: macros,
        local: MacroTable::new(),
        diags: Vec::new(),
    };
    let regions = p.parse_text(TextEnd::Eof);
    Document {
        regions,
        macros: p.merged_macros(),
        diagnostics: p.diags,
    }
}

/// Parse a string that is already math (a macro expansion).
pub fn parse_math_str(
    src: &str,
    sigs: &Signatures,
    macros: &MacroTable,
) -> (Vec<Node>, Vec<Diagnostic>) {
    let opts = ScanOptions::default();
    let mut p = Parser {
        src,
        pos: 0,
        sigs,
        opts: &opts,
        base: macros,
        local: MacroTable::new(),
        diags: Vec::new(),
    };
    let (nodes, _) = p.parse_math(&MathEnd::Eof);
    (nodes, p.diags)
}

const DEFINITION_COMMANDS: &[&str] = &[
    "newcommand",
    "renewcommand",
    "providecommand",
    "DeclareRobustCommand",
    "DeclareMathOperator",
    "def",
    "gdef",
    "edef",
    "xdef",
    "NewDocumentCommand",
    "RenewDocumentCommand",
    "ProvideDocumentCommand",
    "DeclareDocumentCommand",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextEnd {
    Eof,
    CloseBrace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MathEnd {
    Eof,
    Dollar,
    DoubleDollar,
    Paren,
    Bracket,
    Env(String),
    Brace,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    sigs: &'a Signatures,
    opts: &'a ScanOptions,
    /// Definitions in force before this scan (borrowed, never copied per
    /// expansion).
    base: &'a MacroTable,
    /// Definitions this scan found.
    local: MacroTable,
    diags: Vec<Diagnostic>,
}

fn has_blank_line(s: &str) -> bool {
    let mut seen_newline = false;
    for c in s.chars() {
        if c == '\n' {
            if seen_newline {
                return true;
            }
            seen_newline = true;
        } else if !c.is_whitespace() {
            seen_newline = false;
        }
    }
    false
}

impl<'a> Parser<'a> {
    /// The table after this scan: what was in force plus what it defined.
    fn merged_macros(&self) -> MacroTable {
        let mut t = self.base.clone();
        t.extend(self.local.clone());
        t
    }

    /// At `{`: the span of the contents up to the matching `}`, counting
    /// braces only (`%` and `\` are literal), with `pos` left after it.
    fn read_raw_group(&mut self) -> Option<Span> {
        if self.peek() != Some('{') {
            return None;
        }
        let start = self.pos + 1;
        let mut depth = 0usize;
        for (i, b) in self.src.as_bytes()[self.pos..].iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos += i + 1;
                        return Some(Span::new(start, start + i - 1));
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The argument of `\url`, `\href`, `\lstinline` and friends is verbatim:
    /// `%` and `\` mean nothing in it, and it may be delimited by a repeated
    /// character instead of braces. Skip it, and any `[options]` before it.
    fn skip_verbatim_argument(&mut self) {
        let save = self.pos;
        self.skip_trivia();
        if self.peek() == Some('[') {
            if self.read_bracket().is_none() {
                self.pos = save;
                return;
            }
            self.skip_trivia();
        }
        match self.peek() {
            Some('{') => {
                if self.read_raw_group().is_none() {
                    self.pos = save;
                }
            }
            Some(d)
                if d.is_ascii_punctuation()
                    && !matches!(d, '{' | '}' | '[' | ']' | '*' | '\\' | '%' | '$') =>
            {
                self.advance(d);
                match self.src[self.pos..].find(d) {
                    Some(i) => self.pos += i + d.len_utf8(),
                    None => self.pos = save,
                }
            }
            _ => self.pos = save,
        }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn advance(&mut self, c: char) {
        self.pos += c.len_utf8();
    }

    /// Skip whitespace and `%` comments (text or math, the same).
    fn skip_trivia(&mut self) {
        while let Some(t) = lex_one(self.src, self.pos) {
            if t.kind == TokKind::Trivia {
                self.pos = t.span.end;
            } else {
                break;
            }
        }
    }

    fn skip_comment(&mut self) {
        match self.src[self.pos..].find('\n') {
            Some(i) => self.pos += i + 1,
            None => self.pos = self.src.len(),
        }
    }

    /// At `{`: the span of the balanced contents, with `pos` left after `}`.
    fn read_balanced_group(&mut self) -> Option<Span> {
        if self.peek() != Some('{') {
            return None;
        }
        let start = self.pos + 1;
        let mut depth = 0usize;
        let mut i = self.pos;
        let bytes = self.src.as_bytes();
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => {
                    i += 1;
                    if i < bytes.len() {
                        i += self.src[i..]
                            .chars()
                            .next()
                            .map(char::len_utf8)
                            .unwrap_or(1);
                    }
                    continue;
                }
                b'%' => {
                    i = self.src[i..]
                        .find('\n')
                        .map(|n| i + n + 1)
                        .unwrap_or(bytes.len());
                    continue;
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos = i + 1;
                        return Some(Span::new(start, i));
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// At `[`: the span of the contents up to the matching `]`, braces balanced.
    fn read_bracket(&mut self) -> Option<Span> {
        if self.peek() != Some('[') {
            return None;
        }
        let start = self.pos + 1;
        let mut depth = 0usize;
        let mut i = self.pos + 1;
        let bytes = self.src.as_bytes();
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => {
                    i += 1;
                    if i < bytes.len() {
                        i += self.src[i..]
                            .chars()
                            .next()
                            .map(char::len_utf8)
                            .unwrap_or(1);
                    }
                    continue;
                }
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                b']' if depth == 0 => {
                    self.pos = i + 1;
                    return Some(Span::new(start, i));
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    fn warn(&mut self, code: &'static str, offset: usize, message: impl Into<String>) {
        self.diags.push(Diagnostic::warning(code, offset, message));
    }

    fn info(&mut self, code: &'static str, offset: usize, message: impl Into<String>) {
        self.diags.push(Diagnostic::info(code, offset, message));
    }

    // ---- text mode -------------------------------------------------------

    fn parse_text(&mut self, end: TextEnd) -> Vec<MathRegion> {
        let mut regions = Vec::new();
        let mut depth = 0usize;
        loop {
            let Some(c) = self.peek() else {
                if end == TextEnd::CloseBrace {
                    self.warn("unbalanced-brace", self.pos, "text argument never closed");
                }
                return regions;
            };
            match c {
                '%' => self.skip_comment(),
                '\\' => {
                    let tok = lex_one(self.src, self.pos).expect("a token at a backslash");
                    self.pos = tok.span.end;
                    match tok.kind {
                        TokKind::ControlSymbol => match tok.name(self.src) {
                            "(" => regions.push(self.region(
                                RegionKind::Inline,
                                tok.span,
                                MathEnd::Paren,
                            )),
                            "[" => regions.push(self.region(
                                RegionKind::Display,
                                tok.span,
                                MathEnd::Bracket,
                            )),
                            _ => {}
                        },
                        TokKind::ControlWord => {
                            let name = tok.name(self.src);
                            self.text_control_word(name, tok, &mut regions);
                        }
                        _ => unreachable!("a backslash lexes as a control sequence"),
                    }
                }
                '$' => {
                    if self.peek_at(1) == Some('$') {
                        let open = Span::new(self.pos, self.pos + 2);
                        self.pos += 2;
                        regions.push(self.region(RegionKind::Display, open, MathEnd::DoubleDollar));
                    } else {
                        let open = Span::new(self.pos, self.pos + 1);
                        self.pos += 1;
                        regions.push(self.region(RegionKind::Inline, open, MathEnd::Dollar));
                    }
                }
                '{' => {
                    depth += 1;
                    self.pos += 1;
                }
                '}' => {
                    if end == TextEnd::CloseBrace {
                        if depth == 0 {
                            return regions;
                        }
                        depth -= 1;
                    }
                    self.pos += 1;
                }
                c => self.advance(c),
            }
        }
    }

    fn text_control_word(&mut self, name: &str, tok: Token, regions: &mut Vec<MathRegion>) {
        match name {
            "begin" => {
                let save = self.pos;
                self.skip_trivia();
                match self.read_balanced_group() {
                    Some(inner) => {
                        let env = inner.text(self.src).trim().to_string();
                        if self.opts.math_environments.contains(&env) {
                            let open = Span::new(tok.span.start, self.pos);
                            regions.push(self.region(
                                RegionKind::Env(env.clone()),
                                open,
                                MathEnd::Env(env),
                            ));
                        } else if self.opts.verbatim_environments.contains(&env) {
                            let needle = format!("\\end{{{env}}}");
                            match self.src[self.pos..].find(&needle) {
                                Some(i) => self.pos += i + needle.len(),
                                None => {
                                    self.warn(
                                        "unterminated-verbatim",
                                        tok.span.start,
                                        format!("\\begin{{{env}}} never ends"),
                                    );
                                    self.pos = self.src.len();
                                }
                            }
                        }
                    }
                    None => self.pos = save,
                }
            }
            "verb" => {
                if self.peek() == Some('*') {
                    self.pos += 1;
                }
                if let Some(d) = self.peek() {
                    self.advance(d);
                    match self.src[self.pos..].find(d) {
                        Some(i) => self.pos += i + d.len_utf8(),
                        None => self.pos = self.src.len(),
                    }
                }
            }
            "ensuremath" => {
                let save = self.pos;
                self.skip_trivia();
                if self.peek() == Some('{') {
                    let open = Span::new(tok.span.start, self.pos + 1);
                    self.pos += 1;
                    regions.push(self.region(RegionKind::Ensuremath, open, MathEnd::Brace));
                } else {
                    self.pos = save;
                }
            }
            n if self.opts.url_commands.contains(n) => self.skip_verbatim_argument(),
            n if DEFINITION_COMMANDS.contains(&n) => self.parse_definition(n, tok),
            _ => {}
        }
    }

    fn region(&mut self, kind: RegionKind, open: Span, end: MathEnd) -> MathRegion {
        let (nodes, close) = self.parse_math(&end);
        if close.is_none() && end != MathEnd::Eof {
            self.warn(
                "unterminated-math",
                open.start,
                "math region never closed; left as is",
            );
        }
        MathRegion {
            kind,
            open,
            close,
            nodes,
        }
    }

    // ---- definitions -----------------------------------------------------

    /// Read `{\name}` or `\name`; `None` restores nothing (caller restores).
    fn read_defined_name(&mut self) -> Option<String> {
        self.skip_trivia();
        if self.peek() == Some('{') {
            let inner = self.read_balanced_group()?;
            let text = inner.text(self.src).trim();
            let t = lex_one(text, 0)?;
            if t.kind == TokKind::ControlWord && t.span.end == text.len() {
                return Some(t.name(text).to_string());
            }
            return None;
        }
        let t = lex_one(self.src, self.pos)?;
        if t.kind == TokKind::ControlWord || t.kind == TokKind::ControlSymbol {
            self.pos = t.span.end;
            return Some(t.name(self.src).to_string());
        }
        None
    }

    fn parse_definition(&mut self, cmd: &str, tok: Token) {
        let save = self.pos;
        let ok = match cmd {
            "newcommand" | "renewcommand" | "DeclareRobustCommand" => self.def_newcommand(false),
            "providecommand" => self.def_newcommand(true),
            "DeclareMathOperator" => self.def_operator(),
            "def" | "gdef" | "edef" | "xdef" => self.def_tex(),
            _ => self.def_document_command(cmd == "ProvideDocumentCommand"),
        };
        if !ok {
            self.pos = save;
            self.info(
                "definition-not-understood",
                tok.span.start,
                format!("could not read this \\{cmd}; its body is scanned as ordinary text"),
            );
        }
    }

    /// `\newcommand` and friends. With `provide`, as `\providecommand`: a
    /// name that is already defined keeps its definition, so a section's
    /// fallback does not override the package it falls back from.
    fn def_newcommand(&mut self, provide: bool) -> bool {
        if self.peek() == Some('*') {
            self.pos += 1;
        }
        let Some(name) = self.read_defined_name() else {
            return false;
        };
        self.skip_trivia();
        let mut params = 0usize;
        let mut default = None;
        if self.peek() == Some('[') {
            let Some(n) = self.read_bracket() else {
                return false;
            };
            let Ok(n) = n.text(self.src).trim().parse::<usize>() else {
                return false;
            };
            params = n;
            self.skip_trivia();
            if self.peek() == Some('[') {
                let Some(d) = self.read_bracket() else {
                    return false;
                };
                default = Some(d.text(self.src).to_string());
                params = params.saturating_sub(1);
                self.skip_trivia();
            }
        }
        let body = if self.peek() == Some('{') {
            let Some(b) = self.read_balanced_group() else {
                return false;
            };
            b.text(self.src).to_string()
        } else if self.peek() == Some('\\') {
            let t = lex_one(self.src, self.pos).expect("token");
            self.pos = t.span.end;
            t.text(self.src).to_string()
        } else {
            return false;
        };
        if provide && (self.local.get(&name).is_some() || self.base.get(&name).is_some()) {
            return true;
        }
        self.local.insert(Macro {
            name,
            params,
            optional_default: default,
            opaque: opaque_reason(&body),
            body,
            kind: MacroKind::Command,
        });
        true
    }

    fn def_operator(&mut self) -> bool {
        if self.peek() == Some('*') {
            self.pos += 1;
        }
        let Some(name) = self.read_defined_name() else {
            return false;
        };
        self.skip_trivia();
        let Some(b) = self.read_balanced_group() else {
            return false;
        };
        self.local.insert(Macro {
            name,
            params: 0,
            optional_default: None,
            body: b.text(self.src).to_string(),
            kind: MacroKind::Operator,
            opaque: None,
        });
        true
    }

    fn def_tex(&mut self) -> bool {
        self.skip_trivia();
        let Some(t) = lex_one(self.src, self.pos) else {
            return false;
        };
        if !matches!(t.kind, TokKind::ControlWord | TokKind::ControlSymbol) {
            return false;
        }
        self.pos = t.span.end;
        let name = t.name(self.src).to_string();
        let Some(brace) = self.src[self.pos..].find('{') else {
            return false;
        };
        let param_text = self.src[self.pos..self.pos + brace].trim().to_string();
        self.pos += brace;
        let Some(b) = self.read_balanced_group() else {
            return false;
        };
        let body = b.text(self.src).to_string();
        let mut params = 0usize;
        let mut opaque = opaque_reason(&body);
        let mut rest = param_text.as_str();
        while !rest.is_empty() {
            match rest
                .strip_prefix('#')
                .and_then(|r| r.chars().next().filter(char::is_ascii_digit))
            {
                Some(d) if d.to_digit(10) == Some(params as u32 + 1) => {
                    params += 1;
                    rest = &rest[2..];
                }
                _ => {
                    opaque = Some("its parameters are delimited".to_string());
                    break;
                }
            }
        }
        self.local.insert(Macro {
            name,
            params,
            optional_default: None,
            body,
            kind: MacroKind::Command,
            opaque,
        });
        true
    }

    fn def_document_command(&mut self, provide: bool) -> bool {
        let Some(name) = self.read_defined_name() else {
            return false;
        };
        if provide && (self.local.get(&name).is_some() || self.base.get(&name).is_some()) {
            // \ProvideDocumentCommand: read past the definition, keep what is.
            self.skip_trivia();
            self.read_balanced_group();
            self.skip_trivia();
            return self.read_balanced_group().is_some();
        }
        self.skip_trivia();
        let Some(spec) = self.read_balanced_group() else {
            return false;
        };
        let spec_text = spec.text(self.src).to_string();
        self.skip_trivia();
        let Some(b) = self.read_balanced_group() else {
            return false;
        };
        let body = b.text(self.src).to_string();
        let mut params = 0usize;
        let mut default = None;
        let mut opaque = opaque_reason(&body);
        let mut chars = spec_text.chars().peekable();
        let mut first = true;
        while let Some(c) = chars.next() {
            match c {
                c if c.is_whitespace() => continue,
                'm' => params += 1,
                'o' if first => default = Some(String::new()),
                'O' if first && chars.peek() == Some(&'{') => {
                    let mut depth = 0;
                    let mut d = String::new();
                    for c in chars.by_ref() {
                        if c == '{' {
                            depth += 1;
                            if depth == 1 {
                                continue;
                            }
                        } else if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        d.push(c);
                    }
                    default = Some(d);
                }
                other => {
                    opaque = Some(format!("its argument spec uses `{other}`"));
                    break;
                }
            }
            first = false;
        }
        self.local.insert(Macro {
            name,
            params,
            optional_default: default,
            body,
            kind: MacroKind::Command,
            opaque,
        });
        true
    }

    // ---- math mode -------------------------------------------------------

    fn would_terminate(&self, t: &Token, end: &MathEnd) -> bool {
        match t.kind {
            TokKind::Dollar => matches!(end, MathEnd::Dollar | MathEnd::DoubleDollar),
            TokKind::Close => true,
            TokKind::ControlSymbol => {
                let n = t.name(self.src);
                (n == ")" && *end == MathEnd::Paren) || (n == "]" && *end == MathEnd::Bracket)
            }
            TokKind::ControlWord => t.name(self.src) == "end" && matches!(end, MathEnd::Env(_)),
            _ => false,
        }
    }

    fn parse_math(&mut self, end: &MathEnd) -> (Vec<Node>, Option<Span>) {
        let mut nodes = Vec::new();
        loop {
            let Some(tok) = lex_one(self.src, self.pos) else {
                return (nodes, None);
            };
            match tok.kind {
                TokKind::Dollar => match end {
                    MathEnd::Dollar => {
                        self.pos = tok.span.end;
                        return (nodes, Some(tok.span));
                    }
                    MathEnd::DoubleDollar => {
                        if self.peek_at(1) == Some('$') {
                            let span = Span::new(tok.span.start, tok.span.start + 2);
                            self.pos = span.end;
                            return (nodes, Some(span));
                        }
                        self.warn(
                            "single-dollar-in-display",
                            tok.span.start,
                            "a single $ inside $$…$$; region ends here",
                        );
                        self.pos = tok.span.end;
                        return (nodes, Some(tok.span));
                    }
                    _ => {
                        self.warn(
                            "unexpected-dollar",
                            tok.span.start,
                            "a $ inside a math region; left as is",
                        );
                        self.pos = tok.span.end;
                        nodes.push(Node::Leaf(tok));
                    }
                },
                TokKind::Close => {
                    self.pos = tok.span.end;
                    if *end == MathEnd::Brace {
                        return (nodes, Some(tok.span));
                    }
                    self.warn("unbalanced-brace", tok.span.start, "a } with no matching {");
                    nodes.push(Node::Leaf(tok));
                }
                TokKind::Open => {
                    self.pos = tok.span.end;
                    nodes.push(self.parse_group(tok));
                }
                TokKind::ControlSymbol => {
                    let n = tok.name(self.src);
                    if (n == ")" && *end == MathEnd::Paren)
                        || (n == "]" && *end == MathEnd::Bracket)
                    {
                        self.pos = tok.span.end;
                        return (nodes, Some(tok.span));
                    }
                    self.pos = tok.span.end;
                    nodes.push(Node::Leaf(tok));
                }
                TokKind::ControlWord => {
                    self.pos = tok.span.end;
                    if tok.name(self.src) == "end" {
                        if let MathEnd::Env(e) = end {
                            let save = self.pos;
                            self.skip_trivia();
                            if let Some(inner) = self.read_balanced_group() {
                                if inner.text(self.src).trim() == e {
                                    return (nodes, Some(Span::new(tok.span.start, self.pos)));
                                }
                            }
                            self.pos = save;
                        }
                    }
                    let node = self.parse_command(tok, end);
                    nodes.push(node);
                }
                TokKind::Trivia => {
                    if matches!(end, MathEnd::Dollar | MathEnd::Paren)
                        && has_blank_line(tok.text(self.src))
                    {
                        self.warn(
                            "unterminated-math",
                            tok.span.start,
                            "inline math runs into a blank line; region ends here",
                        );
                        return (nodes, None);
                    }
                    self.pos = tok.span.end;
                    nodes.push(Node::Leaf(tok));
                }
                _ => {
                    self.pos = tok.span.end;
                    nodes.push(Node::Leaf(tok));
                }
            }
        }
    }

    fn parse_group(&mut self, open: Token) -> Node {
        let (children, close) = self.parse_math(&MathEnd::Brace);
        if close.is_none() {
            self.warn(
                "unbalanced-brace",
                open.span.start,
                "a { with no matching }",
            );
        }
        Node::Group(Group {
            open,
            children,
            close: close.map(|span| Token {
                kind: TokKind::Close,
                span,
            }),
        })
    }

    fn signature_for(&self, name: &str) -> Signature {
        if let Some(m) = self.local.get(name).or_else(|| self.base.get(name)) {
            return match m.kind {
                MacroKind::Operator => Signature::none(),
                MacroKind::Command => {
                    let mut s = Signature::math_args(m.params);
                    if m.optional_default.is_some() {
                        s.args.insert(0, ArgKind::Optional);
                    }
                    s
                }
            };
        }
        self.sigs.get(name).cloned().unwrap_or_else(Signature::none)
    }

    fn parse_command(&mut self, name: Token, end: &MathEnd) -> Node {
        let sig = self.signature_for(name.name(self.src));
        let mut star = None;
        if sig.star && self.peek() == Some('*') {
            star = Some(Token {
                kind: TokKind::Char,
                span: Span::new(self.pos, self.pos + 1),
            });
            self.pos += 1;
        }
        let mut args = Vec::new();
        for kind in &sig.args {
            let save = self.pos;
            self.skip_trivia();
            let Some(t) = lex_one(self.src, self.pos) else {
                self.pos = save;
                break;
            };
            if self.would_terminate(&t, end) {
                self.pos = save;
                break;
            }
            match kind {
                ArgKind::Math => match t.kind {
                    TokKind::Open => {
                        self.pos = t.span.end;
                        let g = self.parse_group(t);
                        args.push(Arg::Math(g));
                    }
                    TokKind::ControlWord => {
                        self.pos = t.span.end;
                        let c = self.parse_command(t, end);
                        args.push(Arg::Math(c));
                    }
                    _ => {
                        self.pos = t.span.end;
                        args.push(Arg::Math(Node::Leaf(t)));
                    }
                },
                ArgKind::Text => {
                    if t.kind == TokKind::Open {
                        self.pos = t.span.end;
                        let regions = self.parse_text(TextEnd::CloseBrace);
                        let close = if self.peek() == Some('}') {
                            let c = Token {
                                kind: TokKind::Close,
                                span: Span::new(self.pos, self.pos + 1),
                            };
                            self.pos += 1;
                            Some(c)
                        } else {
                            None
                        };
                        args.push(Arg::Text(TextArg {
                            open: t,
                            regions,
                            close,
                        }));
                    } else {
                        self.pos = t.span.end;
                        args.push(Arg::Math(Node::Leaf(t)));
                    }
                }
                ArgKind::Optional => {
                    if t.kind == TokKind::Char && t.ch(self.src) == '[' {
                        match self.read_bracket() {
                            Some(s) => args.push(Arg::Optional(s)),
                            None => self.pos = save,
                        }
                    } else {
                        self.pos = save;
                    }
                }
                ArgKind::Delim => match t.kind {
                    TokKind::ControlWord | TokKind::ControlSymbol | TokKind::Char => {
                        self.pos = t.span.end;
                        args.push(Arg::Delim(Node::Leaf(t)));
                    }
                    _ => {
                        self.pos = save;
                        break;
                    }
                },
            }
        }
        Node::Command(Command {
            name,
            star,
            args,
            end: self.pos,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_default(src: &str) -> Document {
        let sigs = Signatures::builtin();
        scan(src, &sigs, &ScanOptions::default(), &MacroTable::new())
    }

    fn bodies(src: &str) -> Vec<String> {
        scan_default(src)
            .regions
            .iter()
            .map(|r| r.body().text(src).to_string())
            .collect()
    }

    #[test]
    fn finds_every_delimiter_kind() {
        let src =
            "a $x$ b \\(y\\) c \\[z\\] d $$w$$ e \\begin{align} v \\end{align} f \\ensuremath{u} g";
        assert_eq!(bodies(src), vec!["x", "y", "z", "w", " v ", "u"]);
        let d = scan_default(src);
        assert_eq!(d.regions[4].kind, RegionKind::Env("align".into()));
        assert!(d.diagnostics.is_empty(), "{:?}", d.diagnostics);
    }

    #[test]
    fn escapes_comments_and_verbatim_are_not_math() {
        let src = "cost \\$5 % not $math$\n\\verb|$x$| \\begin{verbatim}\n$y$\n\\end{verbatim} \\url{http://a.b/$c} $z$ \\\\(not) $w$";
        assert_eq!(bodies(src), vec!["z", "w"]);
    }

    #[test]
    fn url_arguments_are_verbatim() {
        let src = "See \\url{http://x.y/%7Euser} and $\\alpha$. \\lstinline|$\\beta$| then $\\gamma$, \\lstinline[language=C]!$x$! and $\\delta$. \\href{http://a/%20b}{see $\\epsilon$}";
        assert_eq!(
            bodies(src),
            vec!["\\alpha", "\\gamma", "\\delta", "\\epsilon"]
        );
    }

    #[test]
    fn dollar_inside_text_inside_math_nests() {
        let src = "$a \\text{ if $b$ } c$ tail";
        let d = scan_default(src);
        assert_eq!(d.regions.len(), 1);
        assert_eq!(d.regions[0].body().text(src), "a \\text{ if $b$ } c");
        let Node::Command(cmd) = &d.regions[0].nodes[2] else {
            panic!("expected \\text")
        };
        let Arg::Text(t) = &cmd.args[0] else {
            panic!("expected a text arg")
        };
        assert_eq!(t.regions.len(), 1);
        assert_eq!(t.regions[0].body().text(src), "b");
    }

    #[test]
    fn comments_inside_display_math_and_nested_envs() {
        let src =
            "\\begin{align}\n a &= b % c $ d\n\\\\ \\begin{aligned} e \\end{aligned}\n\\end{align}";
        let d = scan_default(src);
        assert_eq!(d.regions.len(), 1);
        assert!(d.regions[0].close.is_some());
        assert!(d.diagnostics.is_empty(), "{:?}", d.diagnostics);
    }

    #[test]
    fn blank_line_ends_a_runaway_inline_region() {
        let src = "$x + y\n\nNext paragraph with $z$.";
        let d = scan_default(src);
        assert_eq!(d.regions.len(), 2);
        assert!(d.regions[0].close.is_none());
        assert_eq!(d.regions[1].body().text(src), "z");
        assert!(d.diagnostics.iter().any(|g| g.code == "unterminated-math"));
    }

    #[test]
    fn arguments_bind() {
        let src = "$\\frac12 \\sqrt[3]{x} \\mathbf x \\left\\langle a\\right\\rangle \\operatorname*{argmax}_y$";
        let d = scan_default(src);
        let nodes = &d.regions[0].nodes;
        let cmds: Vec<&Command> = nodes
            .iter()
            .filter_map(|n| {
                if let Node::Command(c) = n {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            cmds[0].args.len(),
            2,
            "\\frac12 takes two single-token args"
        );
        assert_eq!(cmds[1].args.len(), 2);
        assert!(matches!(cmds[1].args[0], Arg::Optional(_)));
        assert_eq!(cmds[2].args.len(), 1);
        assert_eq!(cmds[2].args[0].span().text(src), "x");
        assert!(matches!(cmds[3].args[0], Arg::Delim(_)));
        assert_eq!(cmds[3].args[0].span().text(src), "\\langle");
        assert!(cmds[5].star.is_some());
    }

    #[test]
    fn definitions_are_recorded_and_their_bodies_skipped() {
        let src = "\\newcommand{\\R}{\\mathbb{R}} \\newcommand*\\E{\\mathbb{E}} \\newcommand{\\KL}[2]{D(#1 \\| #2)}\n\
                   \\newcommand{\\norm}[2][2]{\\|#2\\|_{#1}} \\DeclareMathOperator*{\\argmax}{arg\\,max}\n\
                   \\def\\foo#1#2{#1$#2$} \\NewDocumentCommand\\bar{O{x} m}{#1#2} $\\R$";
        let d = scan_default(src);
        assert_eq!(d.regions.len(), 1, "the $ in \\def's body is not a region");
        let m = &d.macros;
        assert_eq!(m.get("R").unwrap().body, "\\mathbb{R}");
        assert_eq!(m.get("E").unwrap().body, "\\mathbb{E}");
        assert_eq!(m.get("KL").unwrap().params, 2);
        let norm = m.get("norm").unwrap();
        assert_eq!(
            (norm.params, norm.optional_default.as_deref()),
            (1, Some("2"))
        );
        assert_eq!(m.get("argmax").unwrap().kind, MacroKind::Operator);
        assert_eq!(m.get("foo").unwrap().params, 2);
        let bar = m.get("bar").unwrap();
        assert_eq!(
            (bar.params, bar.optional_default.as_deref()),
            (1, Some("x"))
        );
        assert!(d.diagnostics.is_empty(), "{:?}", d.diagnostics);
    }

    #[test]
    fn providecommand_keeps_an_existing_definition() {
        let sigs = Signatures::builtin();
        let mut base = MacroTable::new();
        base.insert(Macro::new("R", 0, "\\mathbb{R}"));
        let src = "\\providecommand{\\R}{\\mathbf{R}} \\providecommand{\\Q}{\\mathbb{Q}}\n\
                   \\ProvideDocumentCommand{\\Q}{}{\\mathbf{Q}} \\renewcommand{\\R}{\\mathrm{R}}";
        let d = scan(src, &sigs, &ScanOptions::default(), &base);
        assert_eq!(
            d.macros.get("Q").unwrap().body,
            "\\mathbb{Q}",
            "the first definition stands"
        );
        assert_eq!(
            d.macros.get("R").unwrap().body,
            "\\mathrm{R}",
            "\\renewcommand does replace"
        );
        let fresh = scan(
            "\\providecommand{\\R}{\\mathbf{R}}",
            &sigs,
            &ScanOptions::default(),
            &MacroTable::new(),
        );
        assert_eq!(fresh.macros.get("R").unwrap().body, "\\mathbf{R}");
    }

    #[test]
    fn user_macro_arity_binds_arguments() {
        let src = "\\newcommand{\\vect}[1]{\\mathbf{#1}} $\\vect{x}_t \\vect y$";
        let d = scan_default(src);
        let nodes = &d.regions[0].nodes;
        let Node::Command(c) = &nodes[0] else {
            panic!()
        };
        assert_eq!(c.args.len(), 1);
        assert_eq!(c.args[0].span().text(src), "{x}");
    }

    #[test]
    fn spans_cover_the_body() {
        let src = "$a^{\\mathbf x}_1 \\\\ b$";
        let d = scan_default(src);
        let r = &d.regions[0];
        let mut pos = r.body().start;
        for n in &r.nodes {
            assert_eq!(n.span().start, pos, "gap before {:?}", n.span().text(src));
            pos = n.span().end;
        }
        assert_eq!(pos, r.body().end);
    }
}
