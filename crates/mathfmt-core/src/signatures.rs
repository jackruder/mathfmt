//! Command signatures: how many arguments a math command takes and of what
//! kind. Loaded from `data/signatures.tsv`.

use std::collections::HashMap;

use crate::atom::Style;

const SIGNATURES: &str = include_str!("../data/signatures.tsv");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    /// A brace group or one token, in math mode.
    Math,
    /// A brace group scanned in text mode; may hold `$…$`.
    Text,
    /// `[…]`, opaque.
    Optional,
    /// One delimiter token, after `\left` and friends.
    Delim,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub args: Vec<ArgKind>,
    /// A math-alphabet command, and which alphabet.
    pub style: Option<Style>,
    /// `\mathnormal`: a style command that resets to the default alphabet.
    pub plain: bool,
    /// A `*` right after the name belongs to it.
    pub star: bool,
    /// Its argument counts as the caller's surroundings (`\ensuremath`):
    /// atoms inside it are the caller's atoms.
    pub transparent: bool,
}

impl Signature {
    pub fn none() -> Signature {
        Signature {
            args: Vec::new(),
            style: None,
            plain: false,
            star: false,
            transparent: false,
        }
    }

    pub fn math_args(n: usize) -> Signature {
        Signature {
            args: vec![ArgKind::Math; n],
            ..Signature::none()
        }
    }
}

#[derive(Debug)]
pub struct Signatures {
    map: HashMap<String, Signature>,
}

impl Signatures {
    pub fn builtin() -> Signatures {
        let mut map = HashMap::new();
        for line in SIGNATURES.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut cols = line.split('\t');
            let name = cols.next().unwrap_or("").trim();
            let args = cols.next().unwrap_or("").trim();
            let flags = cols.next().unwrap_or("").trim();
            let mut sig = Signature::none();
            for c in args.chars() {
                sig.args.push(match c {
                    'm' => ArgKind::Math,
                    't' => ArgKind::Text,
                    'o' => ArgKind::Optional,
                    'd' => ArgKind::Delim,
                    other => panic!("signatures.tsv: unknown argument kind `{other}` for \\{name}"),
                });
            }
            for flag in flags.split_whitespace() {
                if let Some(style) = flag.strip_prefix("style=") {
                    sig.style = Some(style.parse().expect("style in signatures.tsv"));
                } else if flag == "plain" {
                    sig.plain = true;
                } else if flag == "star" {
                    sig.star = true;
                } else if flag == "transparent" {
                    sig.transparent = true;
                } else {
                    panic!("signatures.tsv: unknown flag `{flag}` for \\{name}");
                }
            }
            map.insert(name.to_string(), sig);
        }
        Signatures { map }
    }

    pub fn get(&self, name: &str) -> Option<&Signature> {
        self.map.get(name)
    }

    /// The math-alphabet commands (`mathbf`, `symbb`, `mathnormal`, …).
    pub fn style_commands(&self) -> impl Iterator<Item = &str> {
        self.map
            .iter()
            .filter(|(_, s)| s.style.is_some() || s.plain)
            .map(|(n, _)| n.as_str())
    }

    /// Names whose first argument is text; the scanner needs them.
    pub fn text_commands(&self) -> impl Iterator<Item = &str> {
        self.map
            .iter()
            .filter(|(_, s)| s.args.first() == Some(&ArgKind::Text))
            .map(|(n, _)| n.as_str())
    }

    /// Add a text command from the config (`text_commands = ["\\mynote"]`).
    pub fn add_text_command(&mut self, name: &str) {
        let name = name.trim_start_matches('\\').to_string();
        self.map.entry(name).or_insert(Signature {
            args: vec![ArgKind::Text],
            ..Signature::none()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads() {
        let s = Signatures::builtin();
        assert_eq!(
            s.get("frac").unwrap().args,
            vec![ArgKind::Math, ArgKind::Math]
        );
        assert_eq!(
            s.get("sqrt").unwrap().args,
            vec![ArgKind::Optional, ArgKind::Math]
        );
        assert_eq!(s.get("mathbf").unwrap().style, Some(Style::Bf));
        assert_eq!(s.get("boldsymbol").unwrap().style, Some(Style::BfIt));
        assert!(s.get("mathnormal").unwrap().plain);
        assert!(s.get("operatorname").unwrap().star);
        assert!(s.get("ensuremath").unwrap().transparent);
        assert_eq!(s.get("text").unwrap().args, vec![ArgKind::Text]);
        assert_eq!(s.get("left").unwrap().args, vec![ArgKind::Delim]);
        assert_eq!(
            s.get("textcolor").unwrap().args,
            vec![ArgKind::Text, ArgKind::Math]
        );
        assert!(s.get("alpha").is_none());
        assert!(s.text_commands().any(|n| n == "label"));
    }
}
