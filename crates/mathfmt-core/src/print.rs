//! From atoms to edits (design §9).

use std::collections::HashMap;

use crate::atom::{Atom, Style};
use crate::config::{Form, Profile, StylePreset};
use crate::diag::Diagnostic;
use crate::lex::{Span, TokKind};
use crate::resolve::{Occurrence, Part};
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
        match spell_occurrence(occ, profile, table) {
            Ok(mut text) => {
                // A style group that had scripts or braces inside was one
                // unit to whatever follows it. Spelled piece by piece it is
                // not, so a script or prime after it would attach to its
                // last piece (`\mathbf{x^2}^3` → `𝐱^{𝟐}^3`, a double
                // superscript): it keeps a group of its own.
                if !occ.is_flat()
                    && text != occ.span.text(src)
                    && !is_one_unit(&text)
                    && followed_by_script(&src[occ.span.end..])
                {
                    text = format!("{{{text}}}");
                }
                // An unbraced argument (`\hat\alpha`) must stay one
                // argument: a macro takes one token, which is one byte of a
                // multi-byte character under pdflatex and the bare command
                // of `\mathbf{x}`.
                if occ.bare_arg && text != occ.span.text(src) && needs_script_braces(&text) {
                    text = format!("{{{text}}}");
                }
                planned.push((occ.span, Some(text)));
            }
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
        if is_script_operand(&src[..span.start]) && needs_script_braces(&text) {
            text = format!("{{{text}}}");
        }
        if ends_with_control_word(before) && starts_alphabetic(&text) {
            text.insert(0, ' ');
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

/// Under a LaTeX styled form, adjacent flat occurrences of one style share a
/// wrapper: `𝐱𝐲` → `\mathbf{xy}`, `\mathbf{x}\mathbf{y}` → `\mathbf{xy}`.
fn merge_runs(occurrences: Vec<Occurrence>, profile: &Profile) -> Vec<Occurrence> {
    if profile.styled != Form::Latex {
        return occurrences;
    }
    let mut out: Vec<Occurrence> = Vec::with_capacity(occurrences.len());
    for occ in occurrences {
        if let Some(last) = out.last_mut() {
            if last.is_flat()
                && occ.is_flat()
                && !last.bare_arg
                && !occ.bare_arg
                && last.span.end == occ.span.start
                && same_single_style(&last.atoms, &occ.atoms)
            {
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

impl Piece {
    fn render(&self) -> String {
        match self {
            Piece::Plain(t) => t.clone(),
            Piece::Wrapper { command, bases } => wrap(command, bases),
        }
    }
}

/// `\mathbf{xy}`, or the template filled in when the command holds `#1`.
fn wrap(command: &str, bases: &str) -> String {
    if command.contains("#1") {
        command.replace("#1", bases)
    } else {
        format!("{command}{{{bases}}}")
    }
}

/// Spell one occurrence: a flat one as its atoms in a row, a structured one
/// (a style group with scripts or braces inside) by filling its atoms back
/// into the structure. Under a LaTeX styled form, when every atom wants the
/// same wrapper, one wrapper encloses the whole structure as the source had
/// it (`\bm{\mu_k}` stays `\bm{\mu_k}`); otherwise each atom is spelled on
/// its own inside the structure (`\mathbf{\sigma_x}` →
/// `\boldsymbol{\sigma}_{\mathbf{x}}`, `𝛔_{𝛉}`).
fn spell_occurrence(occ: &Occurrence, profile: &Profile, table: &Table) -> Result<String, Atom> {
    if occ.is_flat() {
        return spell_atoms(&occ.atoms, profile, table);
    }
    let pieces = occ
        .atoms
        .iter()
        .map(|a| spell_one(a, profile, table))
        .collect::<Result<Vec<_>, _>>()?;
    let shared: Option<&str> = match pieces.first() {
        Some(Piece::Wrapper { command, .. })
            if pieces
                .iter()
                .all(|p| matches!(p, Piece::Wrapper { command: c, .. } if c == command)) =>
        {
            Some(command)
        }
        _ => None,
    };
    let mut body = String::new();
    for part in &occ.parts {
        let text = match part {
            Part::Text(t) => t.clone(),
            Part::Atom(i) => match (&pieces[*i], shared) {
                (Piece::Wrapper { bases, .. }, Some(_)) => bases.clone(),
                (piece, _) => {
                    let t = piece.render();
                    if is_script_operand(&body) && needs_script_braces(&t) {
                        format!("{{{t}}}")
                    } else {
                        t
                    }
                }
            },
        };
        if ends_with_control_word(&body) && starts_alphabetic(&text) {
            body.push(' ');
        }
        body.push_str(&text);
    }
    Ok(match shared {
        Some(command) => wrap(command, &body),
        None => body,
    })
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
        let text = p.render();
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
    // The curated command spelling, where the document's package family
    // defines it. unicode-math replaces amssymb and lacks a few of its
    // names (`\square`, `\leadsto`); there its own name for the character
    // (`\mdlgwhtsquare`) is the spelling that exists.
    let command_spelling = |atom: &Atom| -> Option<String> {
        let curated = table.latex_of(atom)?;
        let exists = profile.preset != StylePreset::UnicodeMath
            || crate::declare::control_words(curated)
                .iter()
                .all(|w| table.in_unicode_math(w));
        if exists {
            Some(curated.to_string())
        } else {
            table.unicode_math_name(atom).map(|n| format!("\\{n}"))
        }
    };
    match atom {
        Atom::Named(c) => {
            let form = if table.is_greek_letter(atom) {
                profile.greek
            } else {
                profile.symbols
            };
            match form {
                Form::Unicode => Ok(Piece::Plain(c.to_string())),
                Form::Latex => command_spelling(atom).map(Piece::Plain).ok_or(*atom),
            }
        }
        Atom::Styled { base, style } => {
            if profile.styled == Form::Unicode {
                if let Some(c) = table.char_of(atom) {
                    return Ok(Piece::Plain(c.to_string()));
                }
            } else if let Some(s) = command_spelling(atom) {
                return Ok(Piece::Plain(s));
            }
            let key = match (style, base) {
                (Style::Bf, b) if !b.is_ascii() => "bf_greek",
                (Style::Bb, b) if b.is_ascii_digit() => "bb_digit",
                (Style::Bb, b) if b.is_ascii_lowercase() => "bb_lower",
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

/// Is the last thing before an occurrence a script operator (`^`, `_`,
/// `\sp`, `\sb`), with any whitespace, newlines and comments between, so
/// the occurrence is the operand of a script? The lines before are lexed
/// from their start, since only that tells a comment from code.
pub(crate) fn is_script_operand(before: &str) -> bool {
    let mut end = before.trim_end().len();
    loop {
        let start = before[..end].rfind('\n').map_or(0, |i| i + 1);
        let line = &before[start..end];
        if let Some(t) = crate::lex::lex_all(line)
            .into_iter()
            .rev()
            .find(|t| t.kind != TokKind::Trivia)
        {
            return match t.kind {
                TokKind::Sup | TokKind::Sub => true,
                TokKind::ControlWord => matches!(t.name(line), "sp" | "sb"),
                _ => false,
            };
        }
        if start == 0 {
            return false;
        }
        end = before[..start].trim_end().len();
    }
}

/// A script operand that is a multi-byte character (pdflatex reads it a byte
/// at a time) or a command with an argument (`x_\boldsymbol{\theta}` is
/// "Missing { inserted") needs braces; a bare control word or ASCII does
/// not, and neither does text that is already one group. Text that only
/// starts with a group is not one (`{𝐱}_{𝟏}`).
pub(crate) fn needs_script_braces(text: &str) -> bool {
    one_group_contents(text).is_none() && (!text.is_ascii() || text.contains('{'))
}

/// What is inside the braces, when `text` is exactly one balanced group.
pub(crate) fn one_group_contents(text: &str) -> Option<&str> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'{') || bytes.last() != Some(&b'}') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 && i + 1 != bytes.len() {
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    (depth == 0).then(|| &text[1..text.len() - 1])
}

/// One group, or one command over one group (`\boldsymbol{\sigma_\theta}`):
/// a script after it attaches to all of it.
fn is_one_unit(text: &str) -> bool {
    let rest = match crate::lex::lex_one(text, 0) {
        Some(t) if t.kind == crate::lex::TokKind::ControlWord => &text[t.span.end..],
        _ => text,
    };
    one_group_contents(rest).is_some() && !rest.starts_with(char::is_whitespace)
}

/// Does a script operator or a prime come next, trivia aside: `^`, `_`,
/// `\sp`, `\sb`, `'`, or a prime character?
fn followed_by_script(after: &str) -> bool {
    let mut pos = 0;
    while let Some(t) = crate::lex::lex_one(after, pos) {
        match t.kind {
            TokKind::Trivia => pos = t.span.end,
            TokKind::Sup | TokKind::Sub => return true,
            TokKind::ControlWord => return matches!(t.name(after), "sp" | "sb"),
            TokKind::Char => {
                return matches!(t.ch(after), '\'' | '′' | '″' | '‴' | '⁗');
            }
            _ => return false,
        }
    }
    false
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
        let (occ, _, mut diags) = resolve(&doc, src, &table, &sigs, &profile, &[]);
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
    fn a_structured_group_stays_one_unit_before_a_script() {
        let (u, _) = convert(
            r"$\mathbf{x^2}^3 \mathbf{x_t}^2 \mathbf{x^2}' \mathbf{x_1} + y$",
            "unicode",
        );
        assert_eq!(
            u, "${𝐱^{𝟐}}^3 {𝐱_{𝐭}}^2 {𝐱^{𝟐}}' 𝐱_{𝟏} + y$",
            "a script after the group attaches to all of it; none after, no braces"
        );
        let (a, _) = convert(
            r"$\mathbf{\sigma^x}^2 \mathfrak{I_x}_2 \bm{\mu_k}^2$",
            "ascii",
        );
        assert_eq!(
            a,
            r"${\boldsymbol{\sigma}^{\mathbf{x}}}^2 {\Im_{\mathfrak{x}}}_2 \boldsymbol{\mu_k}^2$",
            "pieces get a group; one wrapper over the whole is already a unit"
        );
        for (out, profile) in [(&u, "unicode"), (&a, "ascii")] {
            assert_eq!(&convert(out, profile).0, out, "stable under {profile}");
        }
    }

    #[test]
    fn a_script_is_seen_through_sp_sb_comments_newlines_and_primes() {
        let (u, _) = convert(
            "$\\mathbf{x^2}\\sp3 + \\mathbf{x_1}\\sb2 + \\mathbf{x^2}′ + \\mathbf{x^2}% c\n^3 + \\mathbf{x^2}\n'$",
            "unicode",
        );
        assert_eq!(
            u,
            "${𝐱^{𝟐}}\\sp3 + {𝐱_{𝟏}}\\sb2 + {𝐱^{𝟐}}′ + {𝐱^{𝟐}}% c\n^3 + {𝐱^{𝟐}}\n'$"
        );
        assert!(is_script_operand("x^"));
        assert!(is_script_operand("x_ "));
        assert!(is_script_operand("x^% a comment\n"));
        assert!(is_script_operand("x^% a\n% b\n  "));
        assert!(is_script_operand("x\\sp"));
        assert!(is_script_operand("x\\sb "));
        assert!(!is_script_operand("x\\sum "));
        assert!(!is_script_operand("x^y "));
        assert!(!is_script_operand("% x^\n"));
        assert!(followed_by_script("\\sp3"));
        assert!(followed_by_script("% c\n^3"));
        assert!(followed_by_script(" ′"));
        assert!(!followed_by_script(" + 1"));
        assert!(!followed_by_script("\\prime"));
    }

    #[test]
    fn a_group_as_a_script_operand_is_braced_whole() {
        let (u, _) = convert(
            r"$y^\mathbf{{x}_1} x^\mathbf{{a}{b}} z_\mathbf{{w}}$",
            "unicode",
        );
        assert_eq!(
            u, "$y^{{𝐱}_{𝟏}} x^{{𝐚}{𝐛}} z_{𝐰}$",
            "text that only starts with a group is not one group"
        );
        let (a, _) = convert(r"$y^\mathbf{{\sigma}_1}$", "ascii");
        assert_eq!(a, r"$y^{{\boldsymbol{\sigma}}_{\mathbf{1}}}$");
        assert_eq!(one_group_contents("{a}{b}"), None);
        assert_eq!(one_group_contents(r"{a\}b}"), Some(r"a\}b"));
        assert_eq!(one_group_contents("{{a}_1}"), Some("{a}_1"));
        assert_eq!(one_group_contents("a"), None);
    }

    #[test]
    fn an_unbraced_argument_stays_one_argument() {
        let (u, _) = convert(
            r"$\hat\alpha + \frac\alpha\beta + \vec\mu + \sqrt\alpha + \hat{\alpha} + \bar x$",
            "unicode",
        );
        assert_eq!(
            u, r"$\hat{α} + \frac{α}{β} + \vec{μ} + \sqrt{α} + \hat{α} + \bar x$",
            "a macro would take one byte of the character under pdflatex"
        );
        let (a, _) = convert(r"$\hat 𝐱 + \frac 𝐱𝐲 + \hat α + \frac{𝐱}{𝐲}𝐱𝐲$", "ascii");
        assert_eq!(
            a,
            r"$\hat {\mathbf{x}} + \frac {\mathbf{x}}{\mathbf{y}} + \hat \alpha + \frac{\mathbf{x}}{\mathbf{y}}\mathbf{xy}$",
            "a wrapper is braced, a control word is not, and two arguments never merge"
        );
        for (out, profile) in [(&u, "unicode"), (&a, "ascii")] {
            assert_eq!(&convert(out, profile).0, out, "stable under {profile}");
        }
    }

    #[test]
    fn style_groups_distribute_to_unicode_and_back() {
        let src = r"$\mathbf{\sigma_\theta} \bm{\mu_k^{2}} \mathbf{x_1} \mathbf{x+y} x_\mathbf{y}$";
        let (uni, _) = convert(src, "unicode");
        assert_eq!(
            uni, "$𝛔_{𝛉} 𝝁_{𝒌}^{\\boldsymbol{2}} 𝐱_{𝟏} \\mathbf{x+y} x_{𝐲}$",
            "Unicode has bold digits but no bold italic ones, so that 2 keeps its wrapper"
        );
        let (asc, _) = convert(&uni, "ascii");
        assert_eq!(
            asc,
            r"$\boldsymbol{\sigma}_{\boldsymbol{\theta}} \boldsymbol{\mu}_{\boldsymbol{k}}^{\boldsymbol{2}} \mathbf{x}_{\mathbf{1}} \mathbf{x+y} x_{\mathbf{y}}$"
        );
        let (direct, _) = convert(src, "ascii");
        assert_eq!(
            direct,
            r"$\boldsymbol{\sigma_\theta} \boldsymbol{\mu_k^{2}} \mathbf{x_1} \mathbf{x+y} x_\mathbf{y}$",
            "one wrapper when every base wants the same one; untouched when already right"
        );
        let (mixed, _) = convert(r"$\mathbf{\sigma_x} \mathfrak{I_x}$", "ascii");
        assert_eq!(
            mixed, r"$\boldsymbol{\sigma}_{\mathbf{x}} \Im_{\mathfrak{x}}$",
            "different wrappers inside one group are spelled one by one"
        );
        for out in [&uni, &asc, &direct, &mixed] {
            let p = if out.contains('𝛔') {
                "unicode"
            } else {
                "ascii"
            };
            let (again, _) = convert(out, p);
            assert_eq!(&again, out, "idempotent under {p}");
        }
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
