//! Parameterized notations (design §13.1, amendment of 2026-11-08): a named
//! symbol with holes, several spellings (forms), and a profile's choice of
//! one.
//!
//! A form is a pattern with `#n` holes, parsed as math with the document's
//! own definitions so that `\KL{#1}{#2}` has the shape `\KL{p}{q}` has.
//! Matching is tree matching modulo atoms: a leaf matches a leaf that stands
//! for the same atom (`\|` and `‖`), else the same text; a group matches a
//! group with matching children, and a group of one matches that one; a
//! hole binds the shortest run of nodes, balanced in its delimiters, that
//! lets the rest of the pattern match, or, when it fills a group, that
//! group's contents. Forms are tried most specific first (most tokens)
//! across all notations, so `(#1 \,\|\, #2)` wins over `(#1 \| #2)` where
//! both would match.
//!
//! The matcher prefers no match to a doubtful one: a stretch of source that
//! holds a comment or an unclosed group is never a match, a group standing
//! alone in the document (`{\mid}`) is not its content, and a form must
//! begin and end with something literal.
//!
//! Rewriting fills the profile's form with the holes' converted text and
//! runs the result through the profile once more, so the form's own
//! literals (`\|`) come out in the profile's spelling (`‖`) and one form
//! serves every profile.

use std::cmp::Reverse;

use crate::atom::{is_greek, Atom, Style};
use crate::config::Profile;
use crate::lex::{lex_all, Span, TokKind};
use crate::macros::MacroTable;
use crate::parse::{parse_math_str, Arg, Group, Node};
use crate::print::{apply, ends_with_control_word, Edit};
use crate::signatures::Signatures;
use crate::table::Table;

/// One spelling of a notation, parsed.
#[derive(Debug)]
pub struct Form {
    pub name: String,
    pub pattern: String,
    pub nodes: Vec<Node>,
    /// Non-trivia tokens in the pattern: its specificity.
    pub weight: usize,
}

#[derive(Debug)]
pub struct Notation {
    pub name: String,
    pub holes: usize,
    /// Most specific first.
    pub forms: Vec<Form>,
}

/// Parse a notation's forms with the definitions in force (the config has
/// already validated the patterns).
pub fn parse_notations(
    decls: &[(String, Vec<(String, String)>)],
    sigs: &Signatures,
    macros: &MacroTable,
) -> Vec<Notation> {
    decls
        .iter()
        .map(|(name, forms)| {
            let mut parsed: Vec<Form> = forms
                .iter()
                .map(|(fname, pattern)| {
                    let (nodes, _) = parse_math_str(pattern, sigs, macros);
                    let weight = lex_all(pattern)
                        .iter()
                        .filter(|t| t.kind != TokKind::Trivia)
                        .count();
                    Form {
                        name: fname.clone(),
                        pattern: pattern.clone(),
                        nodes,
                        weight,
                    }
                })
                .collect();
            parsed.sort_by_key(|f| (Reverse(f.weight), f.name.clone()));
            let holes = parsed
                .first()
                .map(|f| check_pattern(&f.pattern).unwrap_or(0))
                .unwrap_or(0);
            Notation {
                name: name.clone(),
                holes,
                forms: parsed,
            }
        })
        .collect()
}

/// Check a pattern for the config: holes numbered `#1`…`#n` without gaps,
/// never two holes with nothing but space between them, braces balanced,
/// no comment, and something literal at both ends: a hole at the edge of a
/// form has no boundary, and a form of holes alone would match everything.
/// Returns `n`.
pub fn check_pattern(pattern: &str) -> Result<usize, String> {
    let toks = lex_all(pattern);
    if toks
        .iter()
        .any(|t| t.kind == TokKind::Trivia && t.text(pattern).contains('%'))
    {
        return Err("a form cannot hold a comment".to_string());
    }
    let mut seen = [false; 10];
    let mut last_was_hole = false;
    let mut depth = 0usize;
    let real: Vec<_> = toks.iter().filter(|t| t.kind != TokKind::Trivia).collect();
    for t in &real {
        match t.kind {
            TokKind::Open => depth += 1,
            TokKind::Close => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "a `}` closes nothing".to_string())?;
            }
            _ => {}
        }
        if t.kind == TokKind::Param {
            let n: usize = t.text(pattern)[1..]
                .parse()
                .map_err(|_| format!("`{}` is not a hole", t.text(pattern)))?;
            if n == 0 || n > 9 {
                return Err(format!("hole `#{n}` is out of range (1 to 9)"));
            }
            if last_was_hole {
                return Err(format!(
                    "two holes with nothing between them (`#{n}` follows another); the split would be ambiguous"
                ));
            }
            seen[n] = true;
            last_was_hole = true;
        } else {
            last_was_hole = false;
        }
    }
    if depth != 0 {
        return Err("a `{` is never closed".to_string());
    }
    if !real
        .iter()
        .any(|t| !matches!(t.kind, TokKind::Param | TokKind::Open | TokKind::Close))
    {
        return Err(
            "a form needs something literal beside its holes, or it would match everything"
                .to_string(),
        );
    }
    for (edge, t) in [("begins", real.first()), ("ends", real.last())] {
        if t.is_some_and(|t| t.kind == TokKind::Param) {
            return Err(format!(
                "the form {edge} with a bare hole, which has no boundary there; enclose it (`{{#1}}`) or put a literal beside it"
            ));
        }
    }
    let count = seen.iter().rposition(|&s| s).unwrap_or(0);
    for (n, s) in seen.iter().enumerate().take(count + 1).skip(1) {
        if !s {
            return Err(format!(
                "holes are not numbered without gaps: `#{n}` is missing"
            ));
        }
    }
    Ok(count)
}

/// A hole's binding: the nodes it covers (a raw slice of the document's
/// tree, trivia included) and their source span.
#[derive(Debug)]
pub struct Binding<'n> {
    pub span: Span,
    pub nodes: &'n [Node],
}

/// A form matched in the document.
#[derive(Debug)]
pub struct Match<'n> {
    pub notation: usize,
    pub form: usize,
    pub span: Span,
    /// One per hole, in hole order.
    pub holes: Vec<Binding<'n>>,
    /// Raw nodes consumed from the position the match was tried at.
    pub consumed: usize,
    /// Whether the profile rewrites this match; the resolver decides.
    pub rewritten: bool,
}

/// An owned record of a match, for the outcome and `--emit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotationOccurrence {
    pub name: String,
    pub form: String,
    pub span: Span,
    pub holes: Vec<Span>,
    /// Whether the profile rewrites it (false: recognized only).
    pub rewritten: bool,
}

pub struct Matcher<'a> {
    table: &'a Table,
    sigs: &'a Signatures,
    doc_src: &'a str,
    /// Every (notation, form), most specific form first, then by notation
    /// and form name.
    order: Vec<(usize, usize)>,
}

/// +1 for a node that opens a delimited stretch, -1 for one that closes
/// one, 0 otherwise. Kinds are not told apart, so `[0, 1)` is balanced.
fn delimiter(node: &Node, src: &str) -> i32 {
    match node {
        Node::Leaf(t) => match (t.kind, t.text(src)) {
            (TokKind::Char, "(" | "[" | "⟨" | "⌊" | "⌈") => 1,
            (TokKind::Char, ")" | "]" | "⟩" | "⌋" | "⌉") => -1,
            (TokKind::ControlSymbol, "\\{") => 1,
            (TokKind::ControlSymbol, "\\}") => -1,
            (TokKind::ControlWord, "\\langle" | "\\lfloor" | "\\lceil") => 1,
            (TokKind::ControlWord, "\\rangle" | "\\rfloor" | "\\rceil") => -1,
            _ => 0,
        },
        Node::Command(c) => match c.name.name(src) {
            "left" => 1,
            "right" => -1,
            "langle" | "lfloor" | "lceil" if c.args.is_empty() => 1,
            "rangle" | "rfloor" | "rceil" if c.args.is_empty() => -1,
            _ => 0,
        },
        Node::Group(_) => 0,
    }
}

fn is_script(node: &Node) -> bool {
    matches!(node, Node::Leaf(t) if matches!(t.kind, TokKind::Sup | TokKind::Sub))
}

/// Every group and text argument under these nodes is closed.
fn all_closed(nodes: &[Node]) -> bool {
    nodes.iter().all(|n| match n {
        Node::Leaf(_) => true,
        Node::Group(g) => g.close.is_some() && all_closed(&g.children),
        Node::Command(c) => c.args.iter().all(|a| match a {
            Arg::Math(n) | Arg::Delim(n) => all_closed(std::slice::from_ref(n)),
            Arg::Text(t) => t.close.is_some(),
            Arg::Optional(_) => true,
        }),
    })
}

fn is_trivia(n: &Node) -> bool {
    matches!(n, Node::Leaf(t) if t.kind == TokKind::Trivia)
}

fn non_trivia(nodes: &[Node]) -> Vec<&Node> {
    nodes.iter().filter(|n| !is_trivia(n)).collect()
}

fn hole_index(node: &Node, src: &str) -> Option<usize> {
    match node {
        Node::Leaf(t) if t.kind == TokKind::Param => t.text(src)[1..].parse().ok(),
        _ => None,
    }
}

/// A group whose only non-trivia child is a hole: `{#1}`.
fn hole_only_group(node: &Node, src: &str) -> Option<usize> {
    match node {
        Node::Group(g) => match non_trivia(&g.children).as_slice() {
            [one] => hole_index(one, src),
            _ => None,
        },
        _ => None,
    }
}

/// The span of a raw run, from its first to its last non-trivia node.
fn span_of_run(nodes: &[Node]) -> Span {
    let first = nodes.iter().find(|n| !is_trivia(n));
    let last = nodes.iter().rev().find(|n| !is_trivia(n));
    match (first, last) {
        (Some(f), Some(l)) => Span::new(f.span().start, l.span().end),
        _ => Span::new(0, 0),
    }
}

/// `doc[..k]` where `k` is just past the `count`-th non-trivia node.
fn take_nodes(doc: &[Node], count: usize) -> Option<usize> {
    let mut seen = 0;
    for (i, n) in doc.iter().enumerate() {
        if !is_trivia(n) {
            seen += 1;
            if seen == count {
                return Some(i + 1);
            }
        }
    }
    None
}

impl<'a> Matcher<'a> {
    pub fn new(
        table: &'a Table,
        sigs: &'a Signatures,
        doc_src: &'a str,
        notations: &[Notation],
    ) -> Matcher<'a> {
        let mut order: Vec<(usize, usize)> = notations
            .iter()
            .enumerate()
            .flat_map(|(ni, n)| (0..n.forms.len()).map(move |fi| (ni, fi)))
            .collect();
        order.sort_by(|&(an, af), &(bn, bf)| {
            let (a, b) = (&notations[an].forms[af], &notations[bn].forms[bf]);
            b.weight
                .cmp(&a.weight)
                .then_with(|| notations[an].name.cmp(&notations[bn].name))
                .then_with(|| a.name.cmp(&b.name))
        });
        Matcher {
            table,
            sigs,
            doc_src,
            order,
        }
    }

    /// Try every form at `nodes[start]`, the most specific form of any
    /// notation first; the first match wins. A stretch that holds a comment
    /// or an unclosed group is never a match: a rewrite could not carry the
    /// one and would run past the other.
    pub fn match_at<'n>(
        &self,
        notations: &[Notation],
        nodes: &'n [Node],
        start: usize,
    ) -> Option<Match<'n>> {
        let rest = &nodes[start..];
        if rest.iter().all(is_trivia) {
            return None;
        }
        for &(ni, fi) in &self.order {
            let (notation, form) = (&notations[ni], &notations[ni].forms[fi]);
            let pat = non_trivia(&form.nodes);
            if pat.is_empty() {
                continue;
            }
            let mut binds: Vec<Option<Binding<'n>>> = (0..notation.holes).map(|_| None).collect();
            let Some(used) =
                self.match_seq(&pat, &form.pattern, rest, &mut binds, true, false, false)
            else {
                continue;
            };
            if used == 0 || binds.iter().any(Option::is_none) {
                continue;
            }
            let span = span_of_run(&rest[..used]);
            let commented = lex_all(span.text(self.doc_src)).iter().any(|t| {
                t.kind == TokKind::Trivia && t.text(span.text(self.doc_src)).contains('%')
            });
            if commented || !all_closed(&rest[..used]) {
                continue;
            }
            return Some(Match {
                notation: ni,
                form: fi,
                span,
                holes: binds.into_iter().map(Option::unwrap).collect(),
                consumed: used,
                rewritten: false,
            });
        }
        None
    }

    /// Match `pat` (trivia-free) against a prefix of the raw `doc`. With
    /// `exact`, all of `doc` must be used (trailing trivia aside). `top` is
    /// true at the region level. `after_script` says the node before `doc`
    /// was a `^` or `_`, so a group at its head is an operand, which is the
    /// same thing as its content; a group standing alone is not (`{\mid}`
    /// is an ordinary symbol where `\mid` is a relation), and at a match's
    /// first node matching through it would also cover braces the rewrite
    /// does not reproduce (`\underbrace{\KL{p}{q}}`). Returns how many raw
    /// doc nodes were used.
    #[allow(clippy::too_many_arguments)]
    fn match_seq<'n>(
        &self,
        pat: &[&Node],
        pat_src: &str,
        doc: &'n [Node],
        binds: &mut Vec<Option<Binding<'n>>>,
        top: bool,
        exact: bool,
        after_script: bool,
    ) -> Option<usize> {
        let lead = doc.iter().take_while(|n| is_trivia(n)).count();
        let Some((&first, rest_pat)) = pat.split_first() else {
            return if exact {
                (lead == doc.len()).then_some(doc.len())
            } else {
                Some(0)
            };
        };
        if lead == doc.len() {
            return None;
        }
        let d = &doc[lead..];
        if let Some(h) = hole_index(first, pat_src) {
            let h = h.checked_sub(1)?;
            if rest_pat.is_empty() {
                // A trailing hole: the rest of a group's contents. (At the
                // top level the config refuses one; one node, if it comes
                // to that.)
                let take = if exact || !top {
                    d.len()
                } else {
                    take_nodes(d, 1)?
                };
                return self.bind(h, &d[..take], binds).then_some(lead + take);
            }
            // The shortest run that is balanced in its delimiters and lets
            // the rest match. A run that closes a delimiter it did not open
            // has left the stretch the hole began in, and ends the search.
            let mut depth = 0i32;
            for (i, node) in d.iter().enumerate() {
                if is_trivia(node) {
                    continue;
                }
                depth += delimiter(node, self.doc_src);
                if depth < 0 {
                    return None;
                }
                if depth != 0 {
                    continue;
                }
                let end = i + 1;
                let saved: Vec<bool> = binds.iter().map(Option::is_some).collect();
                if self.bind(h, &d[..end], binds) {
                    if let Some(used) = self.match_seq(
                        rest_pat,
                        pat_src,
                        &d[end..],
                        binds,
                        top,
                        exact,
                        is_script(node),
                    ) {
                        return Some(lead + end + used);
                    }
                }
                for (b, was_set) in binds.iter_mut().zip(saved) {
                    if !was_set {
                        *b = None;
                    }
                }
            }
            return None;
        }
        if !self.match_node(first, pat_src, &d[0], binds, after_script) {
            return None;
        }
        let used = self.match_seq(
            rest_pat,
            pat_src,
            &d[1..],
            binds,
            top,
            exact,
            is_script(&d[0]),
        )?;
        Some(lead + 1 + used)
    }

    /// Bind hole `h` to a raw run; a repeated hole must bind the same text.
    fn bind<'n>(&self, h: usize, run: &'n [Node], binds: &mut [Option<Binding<'n>>]) -> bool {
        let span = span_of_run(run);
        // The config gives every form of a notation the same holes; a form
        // with one more than the others binds nothing.
        let Some(slot) = binds.get(h) else {
            return false;
        };
        match slot {
            Some(existing) => {
                existing.span.text(self.doc_src).trim() == span.text(self.doc_src).trim()
            }
            None => {
                binds[h] = Some(Binding { span, nodes: run });
                true
            }
        }
    }

    /// Bind hole `h` to a group's contents (the text between the braces,
    /// trimmed).
    fn bind_group<'n>(&self, h: usize, g: &'n Group, binds: &mut [Option<Binding<'n>>]) -> bool {
        let start = g.open.span.end;
        let end = g.close.map(|c| c.span.start).unwrap_or(start).max(start);
        let text = &self.doc_src[start..end];
        let lead = text.len() - text.trim_start().len();
        let trail = text.len() - text.trim_end().len();
        let span = Span::new(start + lead, (end - trail).max(start + lead));
        let Some(slot) = binds.get(h) else {
            return false;
        };
        match slot {
            Some(existing) => existing.span.text(self.doc_src).trim() == span.text(self.doc_src),
            None => {
                binds[h] = Some(Binding {
                    span,
                    nodes: &g.children,
                });
                true
            }
        }
    }

    /// The atom a node stands for by itself: a symbol character, a symbol
    /// command, or a style command over one base (`\mathbb{E}`), which is
    /// what the character `𝔼` is.
    fn atom_of(&self, node: &Node, src: &str) -> Option<Atom> {
        match node {
            Node::Leaf(t) => match t.kind {
                TokKind::Char => {
                    let mut it = t.text(src).chars();
                    match (it.next(), it.next()) {
                        (Some(c), None) => self.table.atom_of_char(c),
                        _ => None,
                    }
                }
                TokKind::ControlWord | TokKind::ControlSymbol => {
                    self.table.atom_of_name(t.name(src))
                }
                _ => None,
            },
            Node::Command(c) if c.args.is_empty() => self.table.atom_of_name(c.name.name(src)),
            Node::Command(c) => {
                let style = self.sigs.get(c.name.name(src))?.style?;
                let [Arg::Math(arg)] = c.args.as_slice() else {
                    return None;
                };
                let inner: &Node = match arg {
                    Node::Group(g) => match non_trivia(&g.children).as_slice() {
                        [one] => one,
                        _ => return None,
                    },
                    other => other,
                };
                let base = match inner {
                    Node::Leaf(t) if t.kind == TokKind::Char => {
                        let mut it = t.text(src).chars();
                        match (it.next(), it.next()) {
                            (Some(c), None) if c.is_ascii_alphanumeric() => c,
                            (Some(c), None) => match self.table.atom_of_char(c) {
                                Some(Atom::Named(n)) if is_greek(n) => n,
                                _ => return None,
                            },
                            _ => return None,
                        }
                    }
                    other => match self.atom_of(other, src) {
                        Some(Atom::Named(n)) if is_greek(n) => n,
                        _ => return None,
                    },
                };
                // An upright ASCII letter is an operator word, not a symbol.
                if style == Style::Up && base.is_ascii_alphanumeric() {
                    return None;
                }
                Some(Atom::styled(base, style))
            }
            _ => None,
        }
    }

    fn match_node<'n>(
        &self,
        p: &Node,
        pat_src: &str,
        d: &'n Node,
        binds: &mut Vec<Option<Binding<'n>>>,
        operand: bool,
    ) -> bool {
        // `{#1}` takes a group's contents, or a single node.
        if let Some(h) = hole_only_group(p, pat_src) {
            let Some(h) = h.checked_sub(1) else {
                return false;
            };
            return match d {
                Node::Group(g) => self.bind_group(h, g, binds),
                other => self.bind(h, std::slice::from_ref(other), binds),
            };
        }
        // Same atom, however spelled.
        if let (Some(a), Some(b)) = (self.atom_of(p, pat_src), self.atom_of(d, self.doc_src)) {
            return a == b;
        }
        match (p, d) {
            (Node::Group(pg), Node::Group(dg)) => {
                let pk = non_trivia(&pg.children);
                self.match_seq(&pk, pat_src, &dg.children, binds, false, true, false)
                    .is_some()
            }
            // A group of one is that one.
            (Node::Group(pg), other) => {
                matches!(non_trivia(&pg.children).as_slice(), [one] if self.match_node(one, pat_src, other, binds, operand))
            }
            // The document's group of one is that one only where the braces
            // mean nothing: as a script operand or a command's argument.
            (other, Node::Group(dg)) if operand => {
                matches!(non_trivia(&dg.children).as_slice(), [one] if self.match_node(other, pat_src, one, binds, true))
            }
            (Node::Leaf(pt), Node::Leaf(dt)) => {
                pt.kind == dt.kind && pt.text(pat_src) == dt.text(self.doc_src)
            }
            (Node::Command(pc), Node::Command(dc)) => {
                if pc.name.name(pat_src) != dc.name.name(self.doc_src)
                    || pc.star.is_some() != dc.star.is_some()
                    || pc.args.len() != dc.args.len()
                {
                    return false;
                }
                pc.args.iter().zip(&dc.args).all(|(pa, da)| match (pa, da) {
                    (Arg::Math(pn), Arg::Math(dn)) | (Arg::Delim(pn), Arg::Delim(dn)) => {
                        self.match_node(pn, pat_src, dn, binds, true)
                    }
                    (Arg::Optional(_), Arg::Optional(_)) | (Arg::Text(_), Arg::Text(_)) => {
                        pa.span().text(pat_src).trim() == da.span().text(self.doc_src).trim()
                    }
                    _ => false,
                })
            }
            _ => false,
        }
    }
}

/// The edits for the rewritten notations, merged into the atom edits.
/// Inner matches are rendered first, so an outer hole's text already holds
/// its nested rewrites; the edits a match covers are consumed by it.
/// `convert` runs a rendered fragment through the profile's atom spellings.
pub fn rewrite(
    src: &str,
    mut edits: Vec<Edit>,
    matches: &[Match<'_>],
    notations: &[Notation],
    profile: &Profile,
    convert: &dyn Fn(&str) -> String,
) -> Vec<Edit> {
    let mut order: Vec<&Match<'_>> = matches.iter().filter(|m| m.rewritten).collect();
    order.sort_by_key(|m| (m.span.len(), m.span.start));
    for m in order {
        let Some(pattern) = profile.notations.get(&notations[m.notation].name) else {
            continue;
        };
        // An edit that straddles the match's edge cannot be kept beside the
        // match's own edit; the notation is left as written.
        if edits.iter().any(|e| {
            e.span.start < m.span.end
                && e.span.end > m.span.start
                && !(e.span.start >= m.span.start && e.span.end <= m.span.end)
        }) {
            continue;
        }
        let all = lex_all(pattern);
        let mut out = String::new();
        for (i, t) in all.iter().enumerate() {
            let text: String = if t.kind == TokKind::Param {
                let n: usize = t.text(pattern)[1..].parse().unwrap_or(0);
                let Some(hole) = n.checked_sub(1).and_then(|i| m.holes.get(i)) else {
                    continue;
                };
                let mut text = apply_within(src, hole.span, &edits);
                // Where the form supplies the grouping (`{#1}`, or a script
                // whose operand is braced below), the binding's own braces
                // would be a second pair: `_{x}` into `\E{#1}` is `\E{x}`.
                let script = crate::print::is_script_operand(&out);
                let real = |j: Option<usize>| j.and_then(|j| all.get(j)).map(|t| t.kind);
                let enclosed = real(prev_real(&all, i)) == Some(TokKind::Open)
                    && real(next_real(&all, i)) == Some(TokKind::Close);
                if script || enclosed {
                    if let Some(inner) = crate::print::one_group_contents(&text) {
                        text = inner.trim().to_string();
                    }
                }
                if script && !is_one_token(&text) {
                    format!("{{{text}}}")
                } else {
                    text
                }
            } else {
                t.text(pattern).to_string()
            };
            if ends_with_control_word(&out) && text.chars().next().is_some_and(char::is_alphabetic)
            {
                out.push(' ');
            }
            out.push_str(&text);
        }
        let mut replacement = convert(&out);
        let before = &src[..m.span.start];
        if ends_with_control_word(before)
            && replacement.chars().next().is_some_and(char::is_alphabetic)
        {
            replacement.insert(0, ' ');
        }
        if ends_with_control_word(&replacement)
            && src[m.span.end..]
                .chars()
                .next()
                .is_some_and(char::is_alphabetic)
        {
            replacement.push(' ');
        }
        edits.retain(|e| !(e.span.start >= m.span.start && e.span.end <= m.span.end));
        if replacement != m.span.text(src) {
            edits.push(Edit {
                span: m.span,
                replacement,
            });
        }
    }
    edits.sort_by_key(|e| e.span.start);
    edits
}

/// The index of the nearest token before `i` that is not trivia.
fn prev_real(toks: &[crate::lex::Token], i: usize) -> Option<usize> {
    (0..i).rev().find(|&j| toks[j].kind != TokKind::Trivia)
}

/// The index of the nearest token after `i` that is not trivia.
fn next_real(toks: &[crate::lex::Token], i: usize) -> Option<usize> {
    (i + 1..toks.len()).find(|&j| toks[j].kind != TokKind::Trivia)
}

/// `src[span]` with the edits inside it applied.
fn apply_within(src: &str, span: Span, edits: &[Edit]) -> String {
    let mut inner: Vec<Edit> = edits
        .iter()
        .filter(|e| e.span.start >= span.start && e.span.end <= span.end)
        .map(|e| Edit {
            span: Span::new(e.span.start - span.start, e.span.end - span.start),
            replacement: e.replacement.clone(),
        })
        .collect();
    inner.sort_by_key(|e| e.span.start);
    apply(span.text(src), &inner)
}

/// One character, or one control word: safe as a bare script operand.
fn is_one_token(text: &str) -> bool {
    let toks: Vec<_> = lex_all(text)
        .into_iter()
        .filter(|t| t.kind != TokKind::Trivia)
        .collect();
    matches!(toks.as_slice(), [t] if t.kind != TokKind::Open) && text.trim().is_ascii()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{scan, ScanOptions};

    fn notations(decls: &[(&str, &[(&str, &str)])], macros: &MacroTable) -> Vec<Notation> {
        let sigs = Signatures::builtin();
        let decls: Vec<(String, Vec<(String, String)>)> = decls
            .iter()
            .map(|(n, fs)| {
                (
                    n.to_string(),
                    fs.iter()
                        .map(|(a, b)| (a.to_string(), b.to_string()))
                        .collect(),
                )
            })
            .collect();
        parse_notations(&decls, &sigs, macros)
    }

    /// Match the first region of `src` at node 0: (form, matched text, holes).
    fn first_match(
        src: &str,
        decls: &[(&str, &[(&str, &str)])],
    ) -> Option<(String, String, Vec<String>)> {
        let table = Table::builtin();
        let sigs = Signatures::builtin();
        let doc = scan(src, &sigs, &ScanOptions::default(), &MacroTable::new());
        let nots = notations(decls, &doc.macros);
        let m = Matcher::new(&table, &sigs, src, &nots);
        let region = doc.regions.first()?;
        let found = m.match_at(&nots, &region.nodes, 0)?;
        Some((
            nots[found.notation].forms[found.form].name.clone(),
            found.span.text(src).to_string(),
            found
                .holes
                .iter()
                .map(|b| b.span.text(src).to_string())
                .collect(),
        ))
    }

    const KL: &[(&str, &[(&str, &str)])] = &[(
        "KL",
        &[
            ("macro", r"\KL{#1}{#2}"),
            ("expanded", r"D_{\mathrm{KL}}(#1 \| #2)"),
            ("spaced", r"D_{\mathrm{KL}}(#1 \,\|\, #2)"),
        ],
    )];

    #[test]
    fn expanded_form_matches_modulo_atoms_and_binds_shortest_runs() {
        assert_eq!(
            first_match(r"$D_{\mathrm{KL}}(p \| q) + 1$", KL),
            Some((
                "expanded".into(),
                r"D_{\mathrm{KL}}(p \| q)".into(),
                vec!["p".into(), "q".into()]
            ))
        );
        assert_eq!(
            first_match(r"$D_\mathrm{KL}(\mathcal{N}(0, I) ‖ q_\phi)$", KL).map(|m| m.2),
            Some(vec![r"\mathcal{N}(0, I)".into(), r"q_\phi".into()]),
            "a singleton group matches its one node; ‖ is \\|"
        );
    }

    #[test]
    fn the_most_specific_form_wins() {
        assert_eq!(
            first_match(r"$D_{\mathrm{KL}}(p \,\|\, q)$", KL).map(|m| (m.0, m.2)),
            Some(("spaced".into(), vec!["p".into(), "q".into()]))
        );
    }

    #[test]
    fn macro_form_matches_with_and_without_a_definition() {
        let with = r"\newcommand{\KL}[2]{D_{\mathrm{KL}}(#1 \| #2)} $\KL{p}{q_\phi}$";
        assert_eq!(
            first_match(with, KL).map(|m| (m.0, m.2)),
            Some(("macro".into(), vec!["p".into(), r"q_\phi".into()]))
        );
        assert_eq!(
            first_match(r"$\KL{ p }{q}$", KL).map(|m| (m.1, m.2)),
            Some((r"\KL{ p }{q}".into(), vec!["p".into(), "q".into()]))
        );
    }

    #[test]
    fn a_braced_notation_is_matched_inside_its_braces_not_over_them() {
        assert_eq!(
            first_match(r"${\KL{p}{q}}_x$", KL),
            None,
            "the walker descends into the group instead"
        );
        assert_eq!(
            first_match(r"$\KL{p}{q}$", KL).map(|m| m.1),
            Some(r"\KL{p}{q}".into())
        );
    }

    #[test]
    fn no_match_when_a_literal_differs() {
        assert_eq!(first_match(r"$D_{\mathrm{JS}}(p \| q)$", KL), None);
        assert_eq!(first_match(r"$D_{\mathrm{KL}}(p , q)$", KL), None);
        assert_eq!(
            first_match(r"$\KL{p}$", KL),
            None,
            "a hole left unbound is no match"
        );
    }

    #[test]
    fn a_hole_is_balanced_in_its_delimiters() {
        assert_eq!(
            first_match(r"$D_{\mathrm{KL}}(p \| q(x)) + 1$", KL).map(|m| (m.1, m.2)),
            Some((
                r"D_{\mathrm{KL}}(p \| q(x))".into(),
                vec!["p".into(), "q(x)".into()]
            )),
            "the hole does not stop at the inner parenthesis"
        );
        assert_eq!(
            first_match(r"$D_{\mathrm{KL}}(p_\theta(z | x) \| p(z))$", KL).map(|m| m.2),
            Some(vec![r"p_\theta(z | x)".into(), "p(z)".into()])
        );
        assert_eq!(
            first_match(r"$D_{\mathrm{KL}}(\left(a\right) \| [0, 1))$", KL).map(|m| m.2),
            Some(vec![r"\left(a\right)".into(), "[0, 1)".into()]),
            "kinds of delimiter are not told apart, so a half-open interval is balanced"
        );
        assert_eq!(
            first_match(r"$D_{\mathrm{KL}}(p) + (q \| r)$", KL),
            None,
            "a hole does not leave the parentheses it began in"
        );
    }

    #[test]
    fn a_comment_or_an_unclosed_group_is_never_a_match() {
        assert_eq!(first_match("$\\KL{p}{q$", KL), None);
        assert_eq!(first_match("$\\KL{p % first\n}{q}$", KL), None);
        assert_eq!(first_match("$D_{\\mathrm{KL}}(p % c\n\\| q)$", KL), None);
        assert!(first_match("$\\KL{p\n}{q}$", KL).is_some());
    }

    const COND: &[(&str, &[(&str, &str)])] = &[(
        "cond",
        &[
            ("bar", r"P(#1 \mid #2)"),
            ("sub", r"P_\mathrm{c}(#1 \mid #2)"),
        ],
    )];

    #[test]
    fn a_standing_group_is_not_its_content_and_an_operand_is() {
        assert!(first_match(r"$P(A \mid B)$", COND).is_some());
        assert_eq!(
            first_match(r"$P(A {\mid} B)$", COND),
            None,
            "{{\\mid}} is an ordinary symbol, \\mid a relation"
        );
        assert_eq!(
            first_match(r"$P_{\mathrm{c}}(A \mid B)$", COND).map(|m| m.0),
            Some("sub".into()),
            "a braced script operand is its content"
        );
    }

    #[test]
    fn a_styled_literal_is_its_character() {
        let e: &[(&str, &[(&str, &str)])] = &[
            ("E", &[("sub", r"\mathbb{E}_#1[#2]")]),
            ("N", &[("expanded", r"\mathcal{N}(#1, #2)")]),
        ];
        assert_eq!(
            first_match("$𝔼_{x ∼ p}[f]$", e).map(|m| (m.0, m.2)),
            Some(("sub".into(), vec!["{x ∼ p}".into(), "f".into()]))
        );
        assert_eq!(
            first_match("$𝒩(μ, Σ)$", e).map(|m| m.2),
            Some(vec!["μ".into(), "Σ".into()])
        );
        assert_eq!(
            first_match(r"$\mathbbm{E}_x[f]$", e).map(|m| m.0),
            Some("sub".into()),
            "another command for the same alphabet is the same atom"
        );
        assert_eq!(first_match(r"$\mathbf{E}_x[f]$", e), None);
    }

    #[test]
    fn the_most_specific_form_wins_across_notations() {
        let two: &[(&str, &[(&str, &str)])] = &[
            ("aa", &[("one", r"\foo(#1)")]),
            ("zz", &[("two", r"\foo(#1 \| #2)")]),
        ];
        assert_eq!(
            first_match(r"$\foo(a \| b)$", two).map(|m| m.0),
            Some("two".into()),
            "the longer form of the later notation is tried first"
        );
        assert_eq!(
            first_match(r"$\foo(a)$", two).map(|m| m.0),
            Some("one".into())
        );
    }

    #[test]
    fn patterns_are_checked() {
        assert_eq!(check_pattern(r"\KL{#1}{#2}"), Ok(2));
        assert_eq!(check_pattern(r"x"), Ok(0));
        assert_eq!(check_pattern(r"x_{#1}"), Ok(1));
        assert!(check_pattern(r"\KL{#1}{#3}").is_err());
        assert!(check_pattern(r"#1 #2").is_err());
        assert!(check_pattern(r"(#1, #2)").is_ok());
        for bad in [
            "",
            "   ",
            "#1",
            "{#1}",
            r"{#1",
            r"a}",
            "#1 % c",
            r"\foo{#1} % c",
            r"#1 \cdot #2",
            r"\foo #1",
        ] {
            assert!(check_pattern(bad).is_err(), "`{bad}` should be refused");
        }
    }
}
