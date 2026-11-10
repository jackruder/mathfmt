//! User macro definitions and bounded expansion (design §7).
//!
//! Expansion is textual: `#n` in the body is replaced by the argument's source
//! text, and the result is parsed as math by the resolver. A macro whose body
//! uses conditionals, `\csname`, `\expandafter` or further definitions is
//! opaque: recorded, so its calls take the right number of arguments, but
//! never expanded.

use std::collections::HashMap;

use crate::lex::{lex_all, TokKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroKind {
    /// `\newcommand` and friends: expands to its body.
    Command,
    /// `\DeclareMathOperator`: an operator word, never a symbol.
    Operator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Macro {
    /// Without the backslash.
    pub name: String,
    /// Mandatory parameters.
    pub params: usize,
    /// `Some(default)` when the first parameter is optional (`[default]`).
    pub optional_default: Option<String>,
    pub body: String,
    pub kind: MacroKind,
    /// Why this macro is never expanded, when it is not.
    pub opaque: Option<String>,
    /// Defined by `\providecommand`: it stands only if nothing defined the
    /// name before, which for a command of the kernel or a package (the
    /// symbol table and the signatures know those) it did not.
    pub provided: bool,
}

impl Macro {
    pub fn new(name: &str, params: usize, body: &str) -> Macro {
        Macro {
            name: name.to_string(),
            params,
            optional_default: None,
            body: body.to_string(),
            kind: MacroKind::Command,
            opaque: opaque_reason(body),
            provided: false,
        }
    }

    /// Total arguments a call supplies: the optional one plus the mandatory ones.
    pub fn arity(&self) -> usize {
        self.params + usize::from(self.optional_default.is_some())
    }
}

/// Control words that put a body beyond what textual substitution models.
const OPAQUE_WORDS: &[&str] = &[
    "if",
    "ifx",
    "ifnum",
    "ifdim",
    "ifcase",
    "ifmmode",
    "ifdefined",
    "ifcsname",
    "ifstar",
    "IfBooleanTF",
    "IfNoValueTF",
    "IfValueTF",
    "else",
    "fi",
    "or",
    "csname",
    "endcsname",
    "expandafter",
    "noexpand",
    "def",
    "edef",
    "gdef",
    "xdef",
    "let",
    "futurelet",
    "newcommand",
    "renewcommand",
    "providecommand",
    "DeclareMathOperator",
    "uppercase",
    "lowercase",
    "afterassignment",
    "aftergroup",
    "input",
    "include",
    "newif",
];

pub fn opaque_reason(body: &str) -> Option<String> {
    for t in lex_all(body) {
        if t.kind == TokKind::ControlWord {
            let name = t.name(body);
            if OPAQUE_WORDS.contains(&name) {
                return Some(format!("its body uses \\{name}"));
            }
        }
    }
    None
}

/// The definitions in force: later definitions win, as in TeX.
#[derive(Clone, Debug, Default)]
pub struct MacroTable {
    defs: HashMap<String, Macro>,
}

impl MacroTable {
    pub fn new() -> MacroTable {
        MacroTable::default()
    }

    pub fn insert(&mut self, m: Macro) {
        self.defs.insert(m.name.clone(), m);
    }

    pub fn get(&self, name: &str) -> Option<&Macro> {
        self.defs.get(name)
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Macro> {
        self.defs.values()
    }

    /// Definitions from `other` override this table's.
    pub fn extend(&mut self, other: MacroTable) {
        self.defs.extend(other.defs);
    }

    /// Keep only the definitions `keep` accepts.
    pub fn retain(&mut self, mut keep: impl FnMut(&Macro) -> bool) {
        self.defs.retain(|_, m| keep(m));
    }
}

/// Substitute a call's arguments into the body. `optional` is the `[…]`
/// argument's text when the call gave one; `args` the mandatory arguments.
pub fn substitute(m: &Macro, optional: Option<&str>, args: &[&str]) -> String {
    let mut out = String::with_capacity(m.body.len() + 16);
    let mut chars = m.body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '#' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('#') => {
                chars.next();
                out.push('#');
            }
            Some(d) if d.is_ascii_digit() => {
                chars.next();
                let n = d.to_digit(10).unwrap() as usize;
                let text = match (&m.optional_default, n) {
                    (Some(default), 1) => Some(optional.unwrap_or(default.as_str())),
                    (Some(_), n) => args.get(n - 2).copied(),
                    (None, n) => args.get(n - 1).copied(),
                };
                match text {
                    Some(t) => out.push_str(t),
                    None => {
                        out.push('#');
                        out.push(d);
                    }
                }
            }
            _ => out.push('#'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution() {
        let m = Macro::new("KL", 2, r"D_{\mathrm{KL}}(#1 \| #2)");
        assert_eq!(
            substitute(&m, None, &["p", "q"]),
            r"D_{\mathrm{KL}}(p \| q)"
        );
        let mut o = Macro::new("norm", 1, r"\left\|#2\right\|_{#1}");
        o.optional_default = Some("2".into());
        assert_eq!(substitute(&o, None, &["x"]), r"\left\|x\right\|_{2}");
        assert_eq!(substitute(&o, Some("1"), &["x"]), r"\left\|x\right\|_{1}");
        assert_eq!(substitute(&Macro::new("h", 0, "##"), None, &[]), "#");
    }

    #[test]
    fn opacity() {
        assert!(Macro::new("R", 0, r"\mathbb{R}").opaque.is_none());
        assert!(Macro::new("x", 0, r"\ifmmode a\else b\fi").opaque.is_some());
        assert!(Macro::new("y", 1, r"\csname #1\endcsname").opaque.is_some());
    }

    #[test]
    fn later_definitions_win() {
        let mut t = MacroTable::new();
        t.insert(Macro::new("R", 0, r"\mathbb{R}"));
        t.insert(Macro::new("R", 0, r"\mathrm{R}"));
        assert_eq!(t.get("R").unwrap().body, r"\mathrm{R}");
        assert_eq!(t.len(), 1);
    }
}
