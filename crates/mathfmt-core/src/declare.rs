//! What a preamble must hold for a project's notation to compile: the
//! packages its commands come from, and a meaning for every Unicode
//! character its math uses (`mathfmt declarations`).
//!
//! A character is declared to mean exactly what the profile spells it as
//! with commands, so the Unicode source and the macro source typeset the
//! same page; `--verify` is what proves it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde::Deserialize;

use crate::atom::Atom;
use crate::lex::{lex_all, TokKind};

/// How a document reads Unicode math characters.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Mechanism {
    /// pdflatex with the `unicode-math-input` package, which reads a
    /// character after `\big` or as a script where the kernel's mechanism
    /// does not. Every character is declared all the same: the package's
    /// own table lacks some symbols (↝, ⊳) and reads others as something
    /// else (√ as the radical, ∖ as a set difference of another shape).
    UnicodeMathInput,
    /// pdflatex with the kernel's `\DeclareUnicodeCharacter` and nothing
    /// else: every character is declared, inside `\ensuremath`, so it also
    /// works in text.
    Kernel,
    /// lualatex or xelatex with `unicode-math`: the characters are native.
    UnicodeMath,
}

impl Mechanism {
    pub fn as_str(self) -> &'static str {
        match self {
            Mechanism::UnicodeMathInput => "unicode-math-input",
            Mechanism::Kernel => "kernel",
            Mechanism::UnicodeMath => "unicode-math",
        }
    }
}

/// One character and the commands it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharDecl {
    pub ch: char,
    pub atom: Atom,
    pub spelling: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declarations {
    pub mechanism: Mechanism,
    /// False when no root document was read: every package the commands
    /// need is listed, loaded or not.
    pub preamble_known: bool,
    /// Package → the commands that need it. With a preamble, only the
    /// packages it does not load.
    pub packages: BTreeMap<String, BTreeSet<String>>,
    /// The characters to declare, by codepoint.
    pub chars: Vec<CharDecl>,
    /// Characters the sources use that cannot be declared, and why.
    pub undeclared: Vec<(char, String)>,
}

impl Declarations {
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.chars.is_empty()
    }

    /// The preamble lines, each ending in a newline; comments say what the
    /// lines are for and name what could not be declared.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.packages.is_empty() {
            out.push_str(if self.preamble_known {
                "% packages the notation's commands need and the preamble does not load\n"
            } else {
                "% packages the notation's commands need\n"
            });
            for (package, commands) in &self.packages {
                let _ = write!(out, "\\usepackage{{{package}}}");
                if !commands.is_empty() {
                    let names: Vec<String> = commands.iter().map(|c| format!("\\{c}")).collect();
                    let _ = write!(out, "  % {}", names.join(" "));
                }
                out.push('\n');
            }
        }
        if !self.chars.is_empty() {
            out.push_str(match self.mechanism {
                Mechanism::Kernel => {
                    "% Unicode math input: each character means what the profile spells it as\n"
                }
                _ => "% Unicode math input: each character means what the profile spells it as (unicode-math-input's own table lacks some and reads others differently)\n",
            });
            for d in &self.chars {
                match self.mechanism {
                    Mechanism::Kernel => {
                        let _ = writeln!(
                            out,
                            "\\DeclareUnicodeCharacter{{{:04X}}}{{\\ensuremath{{{}}}}}  % {}",
                            d.ch as u32, d.spelling, d.ch
                        );
                    }
                    _ => {
                        let _ = writeln!(out, "\\umiDeclareMathChar{{{}}}{{{}}}", d.ch, d.spelling);
                    }
                }
            }
        }
        for (c, why) in &self.undeclared {
            let _ = writeln!(out, "% {c} (U+{:04X}) is not declared: {why}", *c as u32);
        }
        out
    }
}

/// The control words of a spelling, without their backslashes.
pub(crate) fn control_words(spelling: &str) -> Vec<String> {
    lex_all(spelling)
        .into_iter()
        .filter(|t| t.kind == TokKind::ControlWord)
        .map(|t| t.name(spelling).to_string())
        .collect()
}
