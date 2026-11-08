//! From atoms to edits (design §9).

use std::collections::HashMap;

use crate::atom::{Atom, Style};
use crate::config::{Form, Profile};
use crate::diag::Diagnostic;
use crate::lex::Span;
use crate::resolve::Occurrence;
use crate::table::Table;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub replacement: String,
}

/// Atoms to swap before printing (`mathfmt rename`).
pub type RenameMap = HashMap<Atom, Atom>;

/// Decide the edits for a file's occurrences.
pub fn plan(
    src: &str,
    occurrences: &[Occurrence],
    profile: &Profile,
    table: &Table,
    rename: &RenameMap,
) -> (Vec<Edit>, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let occurrences = merge_runs(apply_rename(occurrences, rename), profile);

    // First pass: a replacement per occurrence, or none.
    let mut planned: Vec<(Span, Option<String>)> = Vec::with_capacity(occurrences.len());
    for occ in &occurrences {
        match spell_atoms(&occ.atoms, profile, table) {
            Ok(text) => planned.push((occ.span, Some(text))),
            Err(atom) => {
                diags.push(Diagnostic::warning(
                    "no-latex-spelling",
                    occ.span.start,
                    format!(
                        "no {} spelling for {}; `{}` left as written",
                        profile.name,
                        describe(&atom, table),
                        occ.span.text(src)
                    ),
                ));
                planned.push((occ.span, None));
            }
        }
    }

    // Second pass: context. A control word must not run into a letter, and a
    // script operand that becomes a non-ASCII character gets braces.
    let mut edits = Vec::new();
    for i in 0..planned.len() {
        let (span, Some(mut text)) = planned[i].clone() else {
            continue;
        };
        if text == span.text(src) {
            continue;
        }
        // what comes just before, in the output
        let before: &str = match i.checked_sub(1).map(|j| &planned[j]) {
            Some((prev_span, Some(prev_text))) if prev_span.end == span.start => prev_text.as_str(),
            _ => &src[..span.start],
        };
        if ends_with_control_word(before) && starts_alphabetic(&text) {
            text.insert(0, ' ');
        }
        if is_script_operand(&src[..span.start]) && !text.starts_with('{') && !text.is_ascii() {
            text = format!("{{{text}}}");
        }
        // what comes just after
        let after_first: Option<char> = match planned.get(i + 1) {
            Some((next_span, Some(next_text))) if next_span.start == span.end => {
                next_text.chars().next()
            }
            _ => src[span.end..].chars().next(),
        };
        if ends_with_control_word(&text) && after_first.is_some_and(char::is_alphabetic) {
            text.push(' ');
        }
        edits.push(Edit {
            span,
            replacement: text,
        });
    }
    (edits, diags)
}

/// Apply non-overlapping edits, sorted by position.
pub fn apply(src: &str, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    let mut pos = 0;
    for e in edits {
        debug_assert!(e.span.start >= pos, "edits overlap or are unsorted");
        out.push_str(&src[pos..e.span.start]);
        out.push_str(&e.replacement);
        pos = e.span.end;
    }
    out.push_str(&src[pos..]);
    out
}

fn apply_rename(occurrences: &[Occurrence], rename: &RenameMap) -> Vec<Occurrence> {
    if rename.is_empty() {
        return occurrences.to_vec();
    }
    occurrences
        .iter()
        .map(|o| Occurrence {
            atoms: o
                .atoms
                .iter()
                .map(|a| *rename.get(a).unwrap_or(a))
                .collect(),
            ..o.clone()
        })
        .collect()
}

/// Under a LaTeX styled form, adjacent occurrences of one style share a
/// wrapper: `𝐱𝐲` → `\mathbf{xy}`, `\mathbf{x}\mathbf{y}` → `\mathbf{xy}`.
fn merge_runs(occurrences: Vec<Occurrence>, profile: &Profile) -> Vec<Occurrence> {
    if profile.styled != Form::Latex {
        return occurrences;
    }
    let mut out: Vec<Occurrence> = Vec::with_capacity(occurrences.len());
    for occ in occurrences {
        if let Some(last) = out.last_mut() {
            if last.span.end == occ.span.start && same_single_style(&last.atoms, &occ.atoms) {
                last.span = last.span.join(occ.span);
                last.atoms.extend(occ.atoms);
                continue;
            }
        }
        out.push(occ);
    }
    out
}

fn same_single_style(a: &[Atom], b: &[Atom]) -> bool {
    let style = |atoms: &[Atom]| -> Option<Style> {
        let mut it = atoms.iter().map(|x| match x {
            Atom::Styled { style, .. } => Some(*style),
            Atom::Named(_) => None,
        });
        let first = it.next().flatten()?;
        it.all(|s| s == Some(first)).then_some(first)
    };
    matches!((style(a), style(b)), (Some(x), Some(y)) if x == y)
}

fn describe(atom: &Atom, table: &Table) -> String {
    match (atom, table.char_of(atom)) {
        (Atom::Named(c), _) => format!("{c} (U+{:04X})", *c as u32),
        (Atom::Styled { base, style }, Some(c)) => {
            format!("{c} (U+{:04X}, {style} {base})", c as u32)
        }
        (Atom::Styled { base, style }, None) => format!("{style} {base}"),
    }
}

/// The spelling a profile gives one atom, if it has one (for `mathfmt table`).
pub fn spelling(atom: &Atom, profile: &Profile, table: &Table) -> Option<String> {
    spell_atoms(std::slice::from_ref(atom), profile, table).ok()
}

/// One piece of output: a wrapper run or a single symbol.
enum Piece {
    Wrapper { command: String, bases: String },
    Plain(String),
}

/// Spell a sequence of atoms. `Err(atom)` names the first atom with no
/// spelling under this profile.
fn spell_atoms(atoms: &[Atom], profile: &Profile, table: &Table) -> Result<String, Atom> {
    let mut pieces: Vec<Piece> = Vec::new();
    for atom in atoms {
        match spell_one(atom, profile, table)? {
            Piece::Wrapper { command, bases } => {
                if let Some(Piece::Wrapper {
                    command: c,
                    bases: b,
                }) = pieces.last_mut()
                {
                    if *c == command {
                        if ends_with_control_word(b) && starts_alphabetic(&bases) {
                            b.push(' ');
                        }
                        b.push_str(&bases);
                        continue;
                    }
                }
                pieces.push(Piece::Wrapper { command, bases });
            }
            p => pieces.push(p),
        }
    }
    let mut out = String::new();
    for p in pieces {
        let text = match p {
            Piece::Plain(t) => t,
            Piece::Wrapper { command, bases } => {
                if command.contains("#1") {
                    command.replace("#1", &bases)
                } else {
                    format!("{command}{{{bases}}}")
                }
            }
        };
        if ends_with_control_word(&out) && starts_alphabetic(&text) {
            out.push(' ');
        }
        out.push_str(&text);
    }
    Ok(out)
}

fn spell_one(atom: &Atom, profile: &Profile, table: &Table) -> Result<Piece, Atom> {
    // Per-atom overrides, by key and by character.
    if let Some(s) = profile.spellings.get(&atom.key()) {
        return Ok(Piece::Plain(s.clone()));
    }
    if let Some(c) = table.char_of(atom) {
        if let Some(s) = profile.spellings.get(&c.to_string()) {
            return Ok(Piece::Plain(s.clone()));
        }
    }
    // A curated spelling that is plain ASCII (`-` for U+2212) is the one
    // spelling in every profile.
    if let Some(s) = table.latex_of(atom).filter(|s| !s.starts_with('\\')) {
        return Ok(Piece::Plain(s.to_string()));
    }
    match atom {
        Atom::Named(c) => {
            let form = if table.is_greek_letter(atom) {
                profile.greek
            } else {
                profile.symbols
            };
            match form {
                Form::Unicode => Ok(Piece::Plain(c.to_string())),
                Form::Latex => table
                    .latex_of(atom)
                    .map(|s| Piece::Plain(s.to_string()))
                    .ok_or(*atom),
            }
        }
        Atom::Styled { base, style } => {
            if profile.styled == Form::Unicode {
                if let Some(c) = table.char_of(atom) {
                    return Ok(Piece::Plain(c.to_string()));
                }
            } else if let Some(s) = table.latex_of(atom) {
                return Ok(Piece::Plain(s.to_string()));
            }
            let key = match (style, base) {
                (Style::Bf, b) if !b.is_ascii() => "bf_greek",
                (Style::Bb, b) if b.is_ascii_digit() => "bb_digit",
                (s, _) => s.as_str(),
            };
            let command = profile.style_commands.get(key).ok_or(*atom)?.clone();
            let base_text = if base.is_ascii() {
                base.to_string()
            } else {
                let base_atom = Atom::Named(*base);
                let form = if table.is_greek_letter(&base_atom) {
                    profile.greek
                } else {
                    profile.symbols
                };
                match form {
                    Form::Unicode => base.to_string(),
                    Form::Latex => table.latex_of(&base_atom).ok_or(*atom)?.to_string(),
                }
            };
            Ok(Piece::Wrapper {
                command,
                bases: base_text,
            })
        }
    }
}

/// Does `s` end in a control word (`\alpha`, not `\\alpha` or `\,`)?
pub fn ends_with_control_word(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_alphabetic() {
        i -= 1;
    }
    if i == bytes.len() || i == 0 {
        return false;
    }
    let mut backslashes = 0;
    while i > 0 && bytes[i - 1] == b'\\' {
        backslashes += 1;
        i -= 1;
    }
    backslashes % 2 == 1
}

fn starts_alphabetic(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_alphabetic)
}

/// Is the text before an occurrence a `^` or `_` (spaces allowed), so the
/// occurrence is the operand of a script?
fn is_script_operand(before: &str) -> bool {
    let trimmed = before.trim_end_matches([' ', '\t']);
    trimmed.ends_with('^') || trimmed.ends_with('_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::macros::MacroTable;
    use crate::parse::{scan, ScanOptions};
    use crate::resolve::resolve;
    use crate::signatures::Signatures;

    fn convert(src: &str, profile: &str) -> (String, Vec<Diagnostic>) {
        let table = Table::builtin();
        let sigs = Signatures::builtin();
        let doc = scan(src, &sigs, &ScanOptions::default(), &MacroTable::new());
        let profile = Config::default().profile(profile).unwrap();
        let (occ, mut diags) = resolve(&doc, src, &table, &sigs, &profile);
        let (edits, more) = plan(src, &occ, &profile, &table, &RenameMap::new());
        diags.extend(more);
        (apply(src, &edits), diags)
    }

    #[test]
    fn control_word_detection() {
        assert!(ends_with_control_word(r"\alpha"));
        assert!(ends_with_control_word(r"x \in"));
        assert!(!ends_with_control_word(r"\\alpha"));
        assert!(ends_with_control_word(r"\\\alpha"));
        assert!(!ends_with_control_word(r"\,"));
        assert!(!ends_with_control_word("alpha"));
        assert!(!ends_with_control_word(r"\mathbf{x}"));
    }

    #[test]
    fn to_unicode() {
        let (out, _) = convert(
            r"$\boldsymbol{\alpha}\in\mathbb{R}^d$ and $\mathcal{F}\subseteq\mathfrak{F}$, $\varphi(x)=\phi(x)$",
            "unicode",
        );
        assert_eq!(out, "$𝜶∈ℝ^d$ and $ℱ⊆𝔉$, $φ(x)=ϕ(x)$");
    }

    #[test]
    fn to_ascii_with_spacing() {
        let (out, _) = convert("$𝜶∈ℝ^d, αx, α x, x^α, \\frac α2$", "ascii");
        assert_eq!(
            out,
            r"$\boldsymbol{\alpha}\in\mathbb{R}^d, \alpha x, \alpha x, x^\alpha, \frac \alpha2$"
        );
    }

    #[test]
    fn script_operands_get_braces_in_unicode() {
        let (out, _) = convert(r"$x^\alpha_\beta y^{\alpha}$", "unicode");
        assert_eq!(out, "$x^{α}_{β} y^{α}$");
        // and the result is stable
        let (again, _) = convert(&out, "unicode");
        assert_eq!(again, out);
    }

    #[test]
    fn runs_merge_under_latex() {
        let (out, _) = convert(
            "$𝐱𝐲 \\mathbf{x}\\mathbf{y} 𝐱_t𝐲 \\mathbf{x\\alpha}$",
            "ascii",
        );
        assert_eq!(
            out,
            r"$\mathbf{xy} \mathbf{xy} \mathbf{x}_t\mathbf{y} \mathbf{x}\boldsymbol{\alpha}$"
        );
    }

    #[test]
    fn digits_and_greek_pick_their_wrappers() {
        let (out, _) = convert("$𝟙 𝛂 𝜶 \\mathbbm{1}$", "ascii");
        assert_eq!(
            out,
            r"$\mathbbm{1} \boldsymbol{\alpha} \boldsymbol{\alpha} \mathbbm{1}$"
        );
    }

    #[test]
    fn no_latex_spelling_is_reported_and_left() {
        let (out, diags) = convert("$x ∆ y$", "ascii");
        assert_eq!(out, "$x ∆ y$");
        assert_eq!(
            diags
                .iter()
                .filter(|d| d.code == "no-latex-spelling")
                .count(),
            1
        );
    }

    #[test]
    fn upright_latin_keeps_its_wrapper_everywhere() {
        let (out, _) = convert(r"$\mathrm{d}x \mathrm{Var}$", "unicode");
        assert_eq!(out, r"$\mathrm{d}x \mathrm{Var}$");
    }

    #[test]
    fn minus_normalizes_and_ascii_is_untouched() {
        let (out, _) = convert("$a − b - c * d$", "unicode");
        assert_eq!(out, "$a - b - c * d$");
    }

    #[test]
    fn idempotent_on_the_spec_examples() {
        for p in ["unicode", "ascii"] {
            let (once, _) = convert(
                r"Let $\bm{\alpha}\in\R^d$, $\mathcal{N}(0, I)$, $\sum_{i=1}^n x_i \le \infty$ and $𝔼[x] ∈ ℝ$.",
                p,
            );
            let (twice, _) = convert(&once, p);
            assert_eq!(once, twice, "profile {p}");
        }
    }
}
