//! From a math tree to atoms (design §8).

use crate::atom::{is_greek, Atom, Style};
use crate::config::{Profile, StylePreset};
use crate::diag::Diagnostic;
use crate::lex::{Span, TokKind, Token};
use crate::macros::{substitute, Macro, MacroKind, MacroTable};
use crate::notation::{Match, Matcher, Notation};
use crate::parse::{parse_math_str, Arg, Command, Document, Node};
use crate::signatures::Signatures;
use crate::table::Table;

pub const MAX_EXPANSION_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccKind {
    /// A Unicode character: `∈`, `𝐱`.
    Char,
    /// A control sequence: `\in`, `\alpha`, `\|`.
    Name,
    /// A style command over bases: `\mathbf{xy}`.
    StyledGroup,
    /// A user macro call whose expansion is atoms: `\R`, `\vect{x}`.
    MacroCall,
}

/// One piece of a structured occurrence: the i-th atom, or structure copied
/// from the source (`_`, `^`, `{`, `}`) that the style command reached
/// across.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Atom(usize),
    Text(String),
}

/// A stretch of source that stands for one or more atoms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub span: Span,
    pub atoms: Vec<Atom>,
    pub kind: OccKind,
    /// How the atoms sit in the source: empty for a flat occurrence (the
    /// atoms in order with nothing between), else the atoms interleaved with
    /// the scripts and braces of a style group such as `\mathbf{x_1}`.
    pub parts: Vec<Part>,
    /// The occurrence is the whole of an unbraced argument (`\hat\alpha`,
    /// the `\beta` of `\frac\alpha\beta`): its replacement must still be
    /// one argument, and it never merges with a neighbour.
    pub bare_arg: bool,
}

impl Occurrence {
    pub fn is_flat(&self) -> bool {
        self.parts.is_empty()
    }
}

/// Flag the occurrence that is exactly the unbraced argument `arg`.
fn mark_bare_arg(arg: &Node, pushed: &mut [Occurrence]) {
    if matches!(arg, Node::Group(_)) {
        return;
    }
    let span = arg.span();
    for o in pushed {
        if o.span == span {
            o.bare_arg = true;
        }
    }
}

/// Where `inner` sits in `outer`, when it is a sub-slice of it.
fn index_range(outer: &[Node], inner: &[Node]) -> Option<std::ops::Range<usize>> {
    let size = std::mem::size_of::<Node>();
    let base = outer.as_ptr() as usize;
    let at = inner.as_ptr() as usize;
    if inner.is_empty() || size == 0 || at < base {
        return None;
    }
    let start = (at - base) / size;
    (start + inner.len() <= outer.len()).then(|| start..start + inner.len())
}

fn push_text(parts: &mut Vec<Part>, text: &str) {
    if let Some(Part::Text(last)) = parts.last_mut() {
        last.push_str(text);
    } else {
        parts.push(Part::Text(text.to_string()));
    }
}

/// Resolve a document: its atom occurrences, its notation matches (borrowing
/// the tree) and the diagnostics.
pub fn resolve<'d>(
    doc: &'d Document,
    src: &'d str,
    table: &'d Table,
    sigs: &'d Signatures,
    profile: &'d Profile,
    notations: &'d [Notation],
) -> (Vec<Occurrence>, Vec<Match<'d>>, Vec<Diagnostic>) {
    let mut r = Resolver {
        src,
        table,
        sigs,
        macros: &doc.macros,
        profile,
        depth: 0,
        diags: Vec::new(),
        notations,
        matcher: Matcher::new(table, sigs, src, notations),
        matches: Vec::new(),
    };
    let mut out = Vec::new();
    for region in &doc.regions {
        r.walk(&region.nodes, &mut out);
    }
    out.sort_by_key(|o| o.span.start);
    r.matches.sort_by_key(|m| m.span.start);
    (out, r.matches, r.diags)
}

struct Resolver<'a, 'n> {
    src: &'a str,
    table: &'a Table,
    sigs: &'a Signatures,
    macros: &'a MacroTable,
    profile: &'a Profile,
    depth: usize,
    diags: Vec<Diagnostic>,
    /// Empty inside a macro expansion, whose spans are not the document's.
    notations: &'a [Notation],
    matcher: Matcher<'a>,
    matches: Vec<Match<'n>>,
}

impl<'a, 'n> Resolver<'a, 'n> {
    /// Walk nodes, pushing occurrences. Returns true when every non-trivia node
    /// was accounted for by an occurrence: the test for inlining a macro.
    fn walk(&mut self, nodes: &'n [Node], out: &mut Vec<Occurrence>) -> bool {
        let mut complete = true;
        let mut i = 0;
        // Inside a notation kept as written, only what its holes hold can
        // start another match: its literals are spoken for. The end of the
        // kept match and the index ranges of its holes at this level.
        let mut kept: Option<(usize, Vec<std::ops::Range<usize>>)> = None;
        while i < nodes.len() {
            if kept.as_ref().is_some_and(|(end, _)| i >= *end) {
                kept = None;
            }
            // Try the notations at a real node only: trying at the space
            // before one would find the same match twice.
            let trivia = matches!(&nodes[i], Node::Leaf(t) if t.kind == TokKind::Trivia);
            let open = match &kept {
                Some((_, holes)) => holes.iter().any(|r| r.contains(&i)),
                None => true,
            };
            if !self.notations.is_empty() && !trivia && open {
                if let Some(mut m) = self.matcher.match_at(self.notations, nodes, i) {
                    // A notation that is a script operand (`e^\KL{p}{q}`) is
                    // kept as written: what the script takes depends on the
                    // macro's definition, which a rewrite would not carry.
                    m.rewritten = self
                        .profile
                        .notations
                        .contains_key(&self.notations[m.notation].name)
                        && !crate::print::is_script_operand(&self.src[..m.span.start]);
                    if m.rewritten {
                        // The form supplies the literal parts; only the holes
                        // are ours to convert, and nested notations in them.
                        for hole in &m.holes {
                            self.walk(hole.nodes, out);
                        }
                        i += m.consumed;
                        complete = false;
                        self.matches.push(m);
                        continue;
                    }
                    // Recognized but kept as written: recorded, then walked
                    // like anything else.
                    if kept.is_none() {
                        let holes = m
                            .holes
                            .iter()
                            .filter_map(|h| index_range(nodes, h.nodes))
                            .collect();
                        kept = Some((i + m.consumed, holes));
                    }
                    self.matches.push(m);
                }
            }
            match &nodes[i] {
                Node::Leaf(t) => complete &= self.leaf(t, out),
                Node::Group(g) => {
                    // A group is transparent; its atoms are its children's.
                    complete &= self.walk(&g.children, out);
                }
                Node::Command(c) => complete &= self.command(c, out),
            }
            i += 1;
        }
        complete
    }

    /// The atom a name reads as, in the document's package family.
    fn atom_of_name(&self, name: &str) -> Option<Atom> {
        if self.profile.preset == StylePreset::UnicodeMath {
            self.table.atom_of_name_under_unicode_math(name)
        } else {
            self.table.atom_of_name(name)
        }
    }

    fn leaf(&mut self, t: &Token, out: &mut Vec<Occurrence>) -> bool {
        match t.kind {
            TokKind::Trivia => true,
            TokKind::Char => {
                let text = t.text(self.src);
                let mut chars = text.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    return false;
                };
                if let Some(atom) = self.table.atom_of_char(c) {
                    out.push(Occurrence {
                        span: t.span,
                        atoms: vec![self.read_style(atom)],
                        kind: OccKind::Char,
                        parts: Vec::new(),
                        bare_arg: false,
                    });
                    return true;
                }
                // ASCII letters, digits and punctuation are not atoms, so an
                // expansion holding them is not inlined.
                false
            }
            TokKind::ControlWord | TokKind::ControlSymbol => {
                if let Some(atom) = self.atom_of_name(t.name(self.src)) {
                    out.push(Occurrence {
                        span: t.span,
                        atoms: vec![atom],
                        kind: OccKind::Name,
                        parts: Vec::new(),
                        bare_arg: false,
                    });
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// `cal` and `scr` share codepoints; the profile says which one a
    /// character reads as.
    fn read_style(&self, atom: Atom) -> Atom {
        match atom {
            Atom::Styled {
                base,
                style: Style::Cal,
            } if self.profile.script_reads_as == Style::Scr => Atom::styled(base, Style::Scr),
            Atom::Styled {
                base,
                style: Style::BfCal,
            } if self.profile.script_reads_as == Style::Scr => Atom::styled(base, Style::BfScr),
            a => a,
        }
    }

    fn command(&mut self, c: &'n Command, out: &mut Vec<Occurrence>) -> bool {
        let name = c.name.name(self.src);
        if let Some(m) = self.macros.get(name) {
            return self.macro_call(c, m, out);
        }
        if let Some(sig) = self.sigs.get(name) {
            if let Some(style) = sig.style {
                return self.styled_group(c, style, out);
            }
            // \mathnormal and ordinary commands: look inside the arguments.
            let transparent = sig.plain || sig.transparent;
            let mut complete = transparent;
            for arg in &c.args {
                match arg {
                    Arg::Math(n) => {
                        let from = out.len();
                        complete &= self.walk(std::slice::from_ref(n), out);
                        mark_bare_arg(n, &mut out[from..]);
                    }
                    Arg::Delim(n) => complete &= self.walk(std::slice::from_ref(n), out),
                    Arg::Text(t) => {
                        for r in &t.regions {
                            self.walk(&r.nodes, out);
                        }
                        complete = false;
                    }
                    Arg::Optional(_) => complete = false,
                }
            }
            return complete && transparent;
        }
        if c.args.is_empty() {
            if let Some(atom) = self.atom_of_name(name) {
                out.push(Occurrence {
                    span: c.name.span,
                    atoms: vec![atom],
                    kind: OccKind::Name,
                    parts: Vec::new(),
                    bare_arg: false,
                });
                return true;
            }
        }
        false
    }

    /// The base a child of a style group contributes, if it is a legal one.
    fn base_of(&self, node: &Node) -> Option<char> {
        match node {
            Node::Leaf(t) if t.kind == TokKind::Char => {
                let text = t.text(self.src);
                let mut chars = text.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    return None;
                };
                if c.is_ascii_alphanumeric() {
                    return Some(c);
                }
                match self.table.atom_of_char(c) {
                    Some(Atom::Named(n)) if is_greek(n) || n == '∇' || n == '∂' => Some(n),
                    _ => None,
                }
            }
            Node::Command(c) if c.args.is_empty() => {
                match self.atom_of_name(c.name.name(self.src)) {
                    Some(Atom::Named(n)) if is_greek(n) || n == '∇' || n == '∂' => Some(n),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn styled_group(&mut self, c: &'n Command, style: Style, out: &mut Vec<Occurrence>) -> bool {
        let Some(Arg::Math(arg)) = c.args.first() else {
            return false;
        };
        let children: Vec<&Node> = match arg {
            Node::Group(g) => g.children.iter().collect(),
            other => vec![other],
        };
        let mut atoms = Vec::new();
        let mut parts = Vec::new();
        if !self.distribute(&children, style, &mut atoms, &mut parts) {
            self.diags.push(Diagnostic::info(
                "style-group-left-whole",
                c.name.span.start,
                format!(
                    "{} holds more than letters, digits, Greek, scripts and braces; left as written",
                    Span::new(c.name.span.start, c.end).text(self.src)
                ),
            ));
            return false;
        }
        if atoms.is_empty() {
            return false;
        }
        // An upright group of ASCII letters or digits (`\mathrm{Var}`,
        // `\mathrm{d}`) is an operator word, not a symbol: it has no Unicode
        // form and is never rewritten or counted.
        let ascii_base =
            |a: &Atom| matches!(a, Atom::Styled { base, .. } if base.is_ascii_alphanumeric());
        if style == Style::Up && atoms.iter().any(ascii_base) {
            return false;
        }
        // Only scripts and braces make the occurrence structured; a run of
        // bases is flat and merges with its neighbours.
        if !parts.iter().any(|p| matches!(p, Part::Text(_))) {
            parts.clear();
        }
        out.push(Occurrence {
            span: Span::new(c.name.span.start, c.end),
            atoms,
            kind: OccKind::StyledGroup,
            parts,
            bare_arg: false,
        });
        true
    }

    /// Apply `style` to every base under a style command, as TeX does: the
    /// alphabet reaches through scripts and braces, so `\mathbf{x_1}` is bold
    /// `x` and bold `1`. Whitespace between bases means nothing in math and
    /// is dropped. Anything else inside (an operator, a comma, another
    /// command, a comment) stops the distribution and the group is left whole.
    fn distribute(
        &self,
        nodes: &[&Node],
        style: Style,
        atoms: &mut Vec<Atom>,
        parts: &mut Vec<Part>,
    ) -> bool {
        for node in nodes {
            match node {
                Node::Leaf(t) if t.kind == TokKind::Trivia => {
                    if t.text(self.src).contains('%') {
                        return false;
                    }
                }
                Node::Leaf(t) if matches!(t.kind, TokKind::Sup | TokKind::Sub) => {
                    push_text(parts, t.text(self.src));
                }
                Node::Group(g) => {
                    if g.close.is_none() {
                        return false;
                    }
                    push_text(parts, "{");
                    let children: Vec<&Node> = g.children.iter().collect();
                    if !self.distribute(&children, style, atoms, parts) {
                        return false;
                    }
                    push_text(parts, "}");
                }
                other => match self.base_of(other) {
                    Some(b) => {
                        parts.push(Part::Atom(atoms.len()));
                        atoms.push(Atom::styled(b, style));
                    }
                    None => return false,
                },
            }
        }
        true
    }

    fn arg_text(&self, arg: &Arg) -> String {
        match arg {
            // TeX strips one level of braces from a group argument.
            Arg::Math(Node::Group(g)) => {
                let start = g.open.span.end;
                let end = g.close.map(|c| c.span.start).unwrap_or(start);
                self.src[start..end].to_string()
            }
            other => other.span().text(self.src).to_string(),
        }
    }

    fn macro_call(&mut self, c: &'n Command, m: &Macro, out: &mut Vec<Occurrence>) -> bool {
        if m.kind == MacroKind::Operator {
            return false;
        }
        let walk_args = |this: &mut Self, out: &mut Vec<Occurrence>| {
            for arg in &c.args {
                if let Arg::Math(n) = arg {
                    let from = out.len();
                    this.walk(std::slice::from_ref(n), out);
                    mark_bare_arg(n, &mut out[from..]);
                }
            }
        };
        if let Some(reason) = &m.opaque {
            self.diags.push(Diagnostic::info(
                "opaque-macro",
                c.name.span.start,
                format!("\\{} is never expanded: {reason}", m.name),
            ));
            walk_args(self, out);
            return false;
        }
        if !self.profile.inline_macros {
            walk_args(self, out);
            return false;
        }
        if self.depth >= MAX_EXPANSION_DEPTH {
            self.diags.push(Diagnostic::warning(
                "expansion-depth",
                c.name.span.start,
                format!(
                    "\\{} expands more than {MAX_EXPANSION_DEPTH} levels deep; left as written",
                    m.name
                ),
            ));
            return false;
        }
        let mut optional = None;
        let mut args = Vec::new();
        for arg in &c.args {
            match arg {
                Arg::Optional(s) => optional = Some(s.text(self.src).to_string()),
                other => args.push(self.arg_text(other)),
            }
        }
        if args.len() < m.params {
            walk_args(self, out);
            return false;
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let expansion = substitute(m, optional.as_deref(), &arg_refs);
        let (nodes, _) = parse_math_str(&expansion, self.sigs, self.macros);
        let mut sub = Resolver {
            src: &expansion,
            table: self.table,
            sigs: self.sigs,
            macros: self.macros,
            profile: self.profile,
            depth: self.depth + 1,
            diags: Vec::new(),
            notations: &[],
            matcher: Matcher::new(self.table, self.sigs, &expansion, &[]),
            matches: Vec::new(),
        };
        let mut inner = Vec::new();
        let complete = sub.walk(&nodes, &mut inner);
        // Diagnostics inside an expansion point nowhere useful; keep only
        // the depth warning, re-anchored at the call.
        for d in sub.diags {
            if d.code == "expansion-depth" {
                let mut d = d;
                d.offset = c.name.span.start;
                self.diags.push(d);
            }
        }
        if !complete || inner.is_empty() {
            walk_args(self, out);
            return false;
        }
        // Concatenate the expansion's occurrences, renumbering the parts of
        // any structured one.
        let mut atoms: Vec<Atom> = Vec::new();
        let mut parts: Vec<Part> = Vec::new();
        let mut structured = false;
        for o in inner {
            let offset = atoms.len();
            if o.is_flat() {
                for i in 0..o.atoms.len() {
                    parts.push(Part::Atom(offset + i));
                }
            } else {
                structured = true;
                for p in o.parts {
                    match p {
                        Part::Atom(i) => parts.push(Part::Atom(offset + i)),
                        Part::Text(t) => push_text(&mut parts, &t),
                    }
                }
            }
            atoms.extend(o.atoms);
        }
        if !structured {
            parts.clear();
        }
        out.push(Occurrence {
            span: Span::new(c.name.span.start, c.end),
            atoms,
            kind: OccKind::MacroCall,
            parts,
            bare_arg: false,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::parse::{scan, ScanOptions};

    fn run(src: &str) -> (Vec<Occurrence>, Vec<Diagnostic>, String) {
        let table = Table::builtin();
        let sigs = Signatures::builtin();
        let doc = scan(src, &sigs, &ScanOptions::default(), &MacroTable::new());
        let profile = Config::default().profile("unicode").unwrap();
        let (occ, _, diags) = resolve(&doc, src, &table, &sigs, &profile, &[]);
        (occ, diags, src.to_string())
    }

    fn show(occ: &[Occurrence], src: &str) -> Vec<(String, String)> {
        occ.iter()
            .map(|o| {
                (
                    o.span.text(src).to_string(),
                    o.atoms
                        .iter()
                        .map(|a| a.key())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            })
            .collect()
    }

    #[test]
    fn chars_names_and_styled_groups() {
        let (occ, _, src) =
            run(r"$x \in ℝ, \alpha + β, \mathbf{xy}, \mathbb{R}^d, \mathbf{\alpha}, \|v\|$");
        assert_eq!(
            show(&occ, &src),
            vec![
                (r"\in".into(), "∈".into()),
                ("ℝ".into(), "bb:R".into()),
                (r"\alpha".into(), "α".into()),
                ("β".into(), "β".into()),
                (r"\mathbf{xy}".into(), "bf:x bf:y".into()),
                (r"\mathbb{R}".into(), "bb:R".into()),
                (r"\mathbf{\alpha}".into(), "bf:α".into()),
                (r"\|".into(), "‖".into()),
                (r"\|".into(), "‖".into()),
            ]
        );
    }

    #[test]
    fn style_groups_distribute_through_scripts_and_braces() {
        let (occ, diags, src) = run(
            r"$\mathbf{\sigma_\theta} + \bm{\mu_k^{2}} + \mathbf{x+y} + \mathrm{Var} + \mathrm{d}x + \mathrm{\alpha} + \mathrm{x_1}$",
        );
        assert_eq!(
            show(&occ, &src),
            vec![
                (r"\mathbf{\sigma_\theta}".into(), "bf:σ bf:θ".into()),
                (r"\bm{\mu_k^{2}}".into(), "bfit:μ bfit:k bfit:2".into()),
                (r"\mathrm{\alpha}".into(), "up:α".into()),
            ],
            "scripts and braces are reached across; an operator stops it; upright ASCII words are operator names"
        );
        assert_eq!(
            occ[1].parts,
            vec![
                Part::Atom(0),
                Part::Text("_".into()),
                Part::Atom(1),
                Part::Text("^{".into()),
                Part::Atom(2),
                Part::Text("}".into()),
            ]
        );
        assert!(occ[2].is_flat());
        assert_eq!(
            diags
                .iter()
                .filter(|d| d.code == "style-group-left-whole")
                .count(),
            1,
            "only \\mathbf{{x+y}} is reported"
        );
    }

    #[test]
    fn a_macro_over_a_structured_group_inlines() {
        let (occ, _, src) = run("\\newcommand{\\vect}[1]{\\mathbf{#1}} $\\vect{x_t} \\vect{x}$");
        assert_eq!(
            show(&occ, &src),
            vec![
                (r"\vect{x_t}".into(), "bf:x bf:t".into()),
                (r"\vect{x}".into(), "bf:x".into()),
            ]
        );
        assert!(!occ[0].is_flat() && occ[1].is_flat());
    }

    #[test]
    fn macros_inline_when_their_expansion_is_atoms() {
        let (occ, _, src) = run(
            "\\newcommand{\\R}{\\mathbb{R}}\\newcommand{\\vect}[1]{\\mathbf{#1}}\\newcommand{\\KL}[2]{D(#1\\|#2)}\\newcommand{\\RR}{\\R^n}\n\
             $\\R \\vect{x} \\vect y \\KL{\\alpha}{β} \\RR$",
        );
        assert_eq!(
            show(&occ, &src),
            vec![
                (r"\R".into(), "bb:R".into()),
                (r"\vect{x}".into(), "bf:x".into()),
                (r"\vect y".into(), "bf:y".into()),
                (r"\alpha".into(), "α".into()),
                ("β".into(), "β".into()),
            ]
        );
    }

    #[test]
    fn operators_and_opaque_macros_do_not_inline() {
        let (occ, diags, src) = run("\\DeclareMathOperator{\\Var}{Var}\\newcommand{\\w}{\\ifmmode a\\else b\\fi} $\\Var(x) \\w$");
        assert!(show(&occ, &src).is_empty());
        assert!(diags.iter().any(|d| d.code == "opaque-macro"));
    }

    #[test]
    fn text_arguments_are_walked_as_text() {
        let (occ, _, src) = run(r"$\text{for all } x \in A \text{ and $y ∈ B$}$");
        assert_eq!(
            show(&occ, &src),
            vec![(r"\in".into(), "∈".into()), ("∈".into(), "∈".into())]
        );
    }

    #[test]
    fn ensuremath_bodies_inline() {
        let (occ, _, src) = run("\\newcommand{\\R}{\\ensuremath{\\mathbb{R}}} $x \\in \\R$");
        assert_eq!(
            show(&occ, &src),
            vec![(r"\in".into(), "∈".into()), (r"\R".into(), "bb:R".into())]
        );
    }

    #[test]
    fn recursion_is_bounded() {
        let (occ, diags, _) = run("\\newcommand{\\a}{\\a} $\\a$");
        assert!(occ.is_empty());
        assert!(diags.iter().any(|d| d.code == "expansion-depth"));
    }
}
