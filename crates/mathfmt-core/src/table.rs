//! The symbol tables, embedded from `data/` and parsed once.
//!
//! Five files (see the design, §12):
//! - `unicode-math.tsv`: every row of unicode-math's table.
//! - `alphabets.tsv`: codepoint → (style, base) for styled letters and digits.
//! - `greek.tsv`: the plain Greek letters LaTeX names.
//! - `latex.tsv`: the curated ASCII spellings and input aliases.
//! - `packages.tsv`: which package defines each command of a spelling.

use std::collections::HashMap;

use crate::atom::{Atom, Style};

const UNICODE_MATH: &str = include_str!("../data/unicode-math.tsv");
const ALPHABETS: &str = include_str!("../data/alphabets.tsv");
const GREEK: &str = include_str!("../data/greek.tsv");
const LATEX: &str = include_str!("../data/latex.tsv");
const PACKAGES: &str = include_str!("../data/packages.tsv");

/// Math classes whose codepoints are symbols an author types, as opposed to
/// accents and over/under marks that take an argument.
const ATOM_CLASSES: [&str; 9] = [
    "mathalpha",
    "mathrel",
    "mathord",
    "mathbin",
    "mathop",
    "mathopen",
    "mathclose",
    "mathpunct",
    "mathfence",
];

#[derive(Clone, Debug)]
pub struct SymbolInfo {
    /// unicode-math's primary name, without the backslash, when the table has one.
    pub name: Option<String>,
    pub class: Option<String>,
    pub description: Option<String>,
    /// The spelling the `latex` profile emits, with its backslash (or `-`).
    pub latex: Option<String>,
    /// Every control-word name (without backslash) that reads as this atom.
    pub aliases: Vec<String>,
}

#[derive(Debug)]
pub struct Table {
    /// Codepoint → atom, for every character the tool recognizes.
    by_char: HashMap<char, Atom>,
    /// Control-word name (no backslash) → atom.
    by_name: HashMap<String, Atom>,
    /// Atom → its codepoint, when it has one.
    codepoint: HashMap<Atom, char>,
    /// Codepoint → what we know about it.
    info: HashMap<char, SymbolInfo>,
    /// Plain Greek codepoints LaTeX has a name for.
    greek_names: HashMap<char, String>,
    /// Control-word name → what defines it under pdflatex (`kernel`, a
    /// package, or `-`), and whether unicode-math defines it.
    packages: HashMap<String, (String, bool)>,
}

impl Table {
    pub fn builtin() -> Table {
        let mut t = Table {
            by_char: HashMap::new(),
            by_name: HashMap::new(),
            codepoint: HashMap::new(),
            info: HashMap::new(),
            greek_names: HashMap::new(),
            packages: HashMap::new(),
        };
        for row in Self::rows(PACKAGES) {
            if let [name, package, unicode_math] = row[..] {
                t.packages.insert(
                    name.to_string(),
                    (package.to_string(), unicode_math.trim() == "yes"),
                );
            }
        }
        t.load_unicode_math(UNICODE_MATH);
        t.load_alphabets(ALPHABETS);
        t.load_greek(GREEK);
        t.load_latex(LATEX);
        t
    }

    fn rows(text: &str) -> impl Iterator<Item = Vec<&str>> {
        text.lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| l.split('\t').collect())
    }

    fn parse_cp(s: &str) -> char {
        let v = u32::from_str_radix(s.trim(), 16)
            .unwrap_or_else(|_| panic!("bad codepoint `{s}` in a data table"));
        char::from_u32(v).unwrap_or_else(|| panic!("codepoint {s} is not a scalar"))
    }

    fn load_unicode_math(&mut self, text: &str) {
        for row in Self::rows(text) {
            let cp = Self::parse_cp(row[0]);
            let name = row[1].to_string();
            let class = row.get(2).unwrap_or(&"").to_string();
            let desc = row.get(3).unwrap_or(&"").to_string();
            let entry = self.info.entry(cp).or_insert_with(|| SymbolInfo {
                name: None,
                class: None,
                description: None,
                latex: None,
                aliases: Vec::new(),
            });
            entry.name = Some(name.clone());
            entry.class = Some(class.clone());
            entry.description = Some(desc);
            // ASCII is literal, never an atom; accents take arguments.
            if (cp as u32) < 0x80 || !ATOM_CLASSES.contains(&class.as_str()) {
                continue;
            }
            let atom = Atom::Named(cp);
            self.by_char.entry(cp).or_insert(atom);
            self.codepoint.entry(atom).or_insert(cp);
            self.by_name.entry(name).or_insert(atom);
        }
    }

    fn load_alphabets(&mut self, text: &str) {
        for row in Self::rows(text) {
            let cp = Self::parse_cp(row[0]);
            let style: Style = row[1].parse().expect("style in alphabets.tsv");
            let base = row[2].chars().next().expect("base in alphabets.tsv");
            let atom = Atom::styled(base, style);
            // A styled codepoint overrides the Named reading from unicode-math.
            self.by_char.insert(cp, atom);
            self.codepoint.insert(atom, cp);
            // `cal` and `scr` share codepoints: give the twin the codepoint too,
            // and let the char read as the primary (`scr` rows read as `cal`).
            match style {
                Style::Scr => {
                    self.codepoint.insert(Atom::styled(base, Style::Cal), cp);
                    self.by_char.insert(cp, Atom::styled(base, Style::Cal));
                }
                Style::BfScr => {
                    self.codepoint.insert(Atom::styled(base, Style::BfCal), cp);
                    self.by_char.insert(cp, Atom::styled(base, Style::BfCal));
                }
                _ => {}
            }
            if let Some(name) = self.info.get(&cp).and_then(|i| i.name.clone()) {
                self.by_name.insert(name, atom);
            }
        }
    }

    fn load_greek(&mut self, text: &str) {
        for row in Self::rows(text) {
            let cp = Self::parse_cp(row[0]);
            let name = row[1].trim_start_matches('\\').to_string();
            let atom = Atom::Named(cp);
            self.by_char.insert(cp, atom);
            self.codepoint.insert(atom, cp);
            self.by_name.insert(name.clone(), atom);
            self.greek_names.insert(cp, format!("\\{name}"));
            let entry = self.info.entry(cp).or_insert_with(|| SymbolInfo {
                name: None,
                class: None,
                description: None,
                latex: None,
                aliases: Vec::new(),
            });
            entry.latex = Some(format!("\\{name}"));
            entry.aliases.push(name);
        }
    }

    fn load_latex(&mut self, text: &str) {
        for row in Self::rows(text) {
            let cp = Self::parse_cp(row[0]);
            let latex = row.get(1).map(|s| s.trim()).filter(|s| !s.is_empty());
            let aliases: Vec<String> = row
                .get(2)
                .map(|s| {
                    s.split(',')
                        .map(|a| a.trim().trim_start_matches('\\').to_string())
                        .filter(|a| !a.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let atom = *self.by_char.entry(cp).or_insert(Atom::Named(cp));
            self.codepoint.entry(atom).or_insert(cp);
            let entry = self.info.entry(cp).or_insert_with(|| SymbolInfo {
                name: None,
                class: None,
                description: None,
                latex: None,
                aliases: Vec::new(),
            });
            if let Some(l) = latex {
                entry.latex = Some(l.to_string());
                if let Some(n) = l.strip_prefix('\\') {
                    if n.chars().all(|c| c.is_ascii_alphabetic()) {
                        self.by_name.insert(n.to_string(), atom);
                        entry.aliases.push(n.to_string());
                    } else {
                        // a control symbol spelling such as `\|`
                        self.by_name.insert(n.to_string(), atom);
                    }
                }
            }
            for a in aliases {
                self.by_name.insert(a.clone(), atom);
                entry.aliases.push(a);
            }
        }
    }

    /// The atom a character stands for, if the character is a math symbol.
    pub fn atom_of_char(&self, c: char) -> Option<Atom> {
        self.by_char.get(&c).copied()
    }

    /// The atom a control sequence stands for (`name` without the backslash:
    /// `in`, `alpha`, `|`).
    pub fn atom_of_name(&self, name: &str) -> Option<Atom> {
        self.by_name.get(name).copied()
    }

    /// The character for an atom, if Unicode has one.
    pub fn char_of(&self, atom: &Atom) -> Option<char> {
        self.codepoint.get(atom).copied()
    }

    /// The curated ASCII spelling for an atom (`\in`, `\alpha`, `\Im`, `-`).
    pub fn latex_of(&self, atom: &Atom) -> Option<&str> {
        let cp = self.char_of(atom)?;
        self.info.get(&cp).and_then(|i| i.latex.as_deref())
    }

    /// The package a command needs (`mathbbm` → `bbm`, `leqslant` →
    /// `amssymb`); `None` for a kernel command and for one the table does
    /// not list. `-` marks a command no package the generator tried defines.
    pub fn package_of(&self, command: &str) -> Option<&str> {
        self.defined_by(command).filter(|p| *p != "kernel")
    }

    /// What `packages.tsv` says defines a command under pdflatex: `kernel`,
    /// a package, or `-` for none the generator tried; `None` when it has
    /// no row.
    pub fn defined_by(&self, command: &str) -> Option<&str> {
        self.packages.get(command).map(|(p, _)| p.as_str())
    }

    /// Does lualatex with unicode-math define this command? A command the
    /// table does not know (a user's macro) is taken to exist.
    pub fn in_unicode_math(&self, command: &str) -> bool {
        self.packages.get(command).map_or(true, |(_, um)| *um)
    }

    /// unicode-math's own name for an atom's character (`mdlgwhtsquare`),
    /// the spelling that package always defines.
    pub fn unicode_math_name(&self, atom: &Atom) -> Option<&str> {
        let cp = self.char_of(atom)?;
        self.info.get(&cp).and_then(|i| i.name.as_deref())
    }

    /// True when the atom is a plain Greek letter LaTeX can name.
    pub fn is_greek_letter(&self, atom: &Atom) -> bool {
        match atom {
            Atom::Named(c) => self.greek_names.contains_key(c),
            Atom::Styled { .. } => false,
        }
    }

    pub fn info(&self, c: char) -> Option<&SymbolInfo> {
        self.info.get(&c)
    }

    /// Every atom with a codepoint, sorted, for `mathfmt table`.
    pub fn all_atoms(&self) -> Vec<(char, Atom)> {
        let mut v: Vec<(char, Atom)> = self.by_char.iter().map(|(c, a)| (*c, *a)).collect();
        v.sort();
        v
    }

    pub fn len(&self) -> usize {
        self.by_char.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_char.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_everything() {
        let t = Table::builtin();
        assert!(t.len() > 1500, "{} atoms", t.len());
    }

    /// `packages.tsv` is asked of TeX, so this is the independent check of
    /// the hand-written `latex.tsv`: every command a spelling emits is
    /// defined by the kernel or by a package the table can name.
    #[test]
    fn every_spelling_is_defined_somewhere() {
        let t = Table::builtin();
        let mut checked = 0;
        for (_, atom) in t.all_atoms() {
            let Some(spelling) = t.latex_of(&atom) else {
                continue;
            };
            for tok in crate::lex::lex_all(spelling) {
                if tok.kind != crate::lex::TokKind::ControlWord {
                    continue;
                }
                let name = tok.name(spelling);
                let source = t.defined_by(name);
                assert!(
                    source.is_some_and(|s| s != "-"),
                    "\\{name} (in `{spelling}`) is defined by no pdflatex package in packages.tsv; a spelling must be, or the row must lose it; run tools/gen-packages.py"
                );
                checked += 1;
            }
        }
        assert!(checked > 300, "{checked} commands checked");
    }

    #[test]
    fn spec_examples() {
        let t = Table::builtin();
        assert_eq!(t.atom_of_name("in"), Some(Atom::Named('∈')));
        assert_eq!(t.atom_of_char('∈'), Some(Atom::Named('∈')));
        assert_eq!(t.atom_of_char('𝐱'), Some(Atom::styled('x', Style::Bf)));
        assert_eq!(t.atom_of_char('ℝ'), Some(Atom::styled('R', Style::Bb)));
        assert_eq!(t.atom_of_name("BbbR"), Some(Atom::styled('R', Style::Bb)));
        assert_eq!(t.atom_of_name("varphi"), Some(Atom::Named('φ')));
        assert_eq!(t.atom_of_name("phi"), Some(Atom::Named('ϕ')));
        assert_eq!(t.atom_of_name("emptyset"), Some(Atom::Named('∅')));
        assert_eq!(t.atom_of_name("varnothing"), Some(Atom::Named('∅')));
        assert_eq!(
            t.latex_of(&Atom::Named('∅')),
            Some("\\varnothing"),
            "the character is the circle with a slash, which is amssymb's \\varnothing; \\emptyset is the slashed zero"
        );
        assert_eq!(t.atom_of_name("le"), Some(Atom::Named('≤')));
        assert_eq!(t.atom_of_name("|"), Some(Atom::Named('‖')));
        assert_eq!(t.atom_of_char('𝟙'), Some(Atom::styled('1', Style::Bb)));
        assert_eq!(t.atom_of_char('𝛂'), Some(Atom::styled('α', Style::Bf)));
        assert_eq!(t.atom_of_char('𝜶'), Some(Atom::styled('α', Style::BfIt)));
        // script codepoints read as cal; cal and scr both print to them
        assert_eq!(t.atom_of_char('ℱ'), Some(Atom::styled('F', Style::Cal)));
        assert_eq!(t.char_of(&Atom::styled('F', Style::Scr)), Some('ℱ'));
        assert_eq!(t.char_of(&Atom::styled('F', Style::Cal)), Some('ℱ'));
        assert_eq!(t.char_of(&Atom::styled('F', Style::Frak)), Some('𝔉'));
        // upright Latin has no codepoint
        assert_eq!(t.char_of(&Atom::styled('d', Style::Up)), None);
        // ASCII is never an atom
        assert_eq!(t.atom_of_char('x'), None);
        assert_eq!(t.atom_of_char('('), None);
        // curated spellings
        assert_eq!(t.latex_of(&Atom::Named('∈')), Some("\\in"));
        assert_eq!(t.latex_of(&Atom::Named('→')), Some("\\rightarrow"));
        assert_eq!(t.atom_of_name("to"), Some(Atom::Named('→')));
        assert_eq!(t.latex_of(&Atom::Named('−')), Some("-"));
        assert_eq!(t.latex_of(&Atom::Named('α')), Some("\\alpha"));
        assert_eq!(t.latex_of(&Atom::styled('I', Style::Frak)), Some("\\Im"));
        assert!(t.is_greek_letter(&Atom::Named('α')));
        assert!(!t.is_greek_letter(&Atom::Named('∈')));
        // where a spelling's command comes from
        assert_eq!(t.package_of("in"), None, "the kernel defines \\in");
        assert_eq!(t.package_of("mathbb"), Some("amsfonts"));
        assert_eq!(t.package_of("leqslant"), Some("amssymb"));
        assert_eq!(t.package_of("mathbbm"), Some("bbm"));
        assert_eq!(t.package_of("boldsymbol"), Some("amsmath"));
        assert_eq!(
            t.package_of("symbfup"),
            Some("-"),
            "no pdflatex package has it"
        );
        assert!(t.in_unicode_math("symbfup") && t.in_unicode_math("leqslant"));
        assert!(!t.in_unicode_math("square") && !t.in_unicode_math("leadsto"));
        assert!(
            t.in_unicode_math("mymacro"),
            "an unknown command is assumed to exist"
        );
        assert_eq!(
            t.package_of("mho"),
            Some("amsfonts"),
            "the kernel's \\mho is a stub"
        );
        assert_eq!(
            t.unicode_math_name(&Atom::Named('□')),
            Some("mdlgwhtsquare")
        );
        assert_eq!(t.latex_of(&Atom::styled('k', Style::Bb)), Some("\\Bbbk"));
        assert_eq!(
            t.latex_of(&Atom::Named('≉')),
            None,
            "no pdflatex package defines \\napprox"
        );
        assert_eq!(t.atom_of_name("napprox"), Some(Atom::Named('≉')));
        assert_eq!(t.atom_of_name("blacktriangleright"), Some(Atom::Named('▸')));
        // a symbol unicode-math knows but LaTeX has no name for
        assert_eq!(t.atom_of_char('∆'), Some(Atom::Named('∆')));
        assert_eq!(t.latex_of(&Atom::Named('∆')), None);
    }
}
