//! The canonical identity of a symbol: what every spelling resolves to.

use std::fmt;
use std::str::FromStr;

/// A math alphabet. Literal semantics: `\mathbf` is `Bf` (bold upright) and
/// `\boldsymbol` is `BfIt`; engine rendering quirks are lint material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Style {
    Up,
    It,
    Bf,
    BfIt,
    Bb,
    Cal,
    Scr,
    BfCal,
    BfScr,
    Frak,
    BfFrak,
    Sf,
    SfIt,
    BfSf,
    BfSfIt,
    Tt,
}

impl Style {
    pub const ALL: [Style; 16] = [
        Style::Up,
        Style::It,
        Style::Bf,
        Style::BfIt,
        Style::Bb,
        Style::Cal,
        Style::Scr,
        Style::BfCal,
        Style::BfScr,
        Style::Frak,
        Style::BfFrak,
        Style::Sf,
        Style::SfIt,
        Style::BfSf,
        Style::BfSfIt,
        Style::Tt,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Style::Up => "up",
            Style::It => "it",
            Style::Bf => "bf",
            Style::BfIt => "bfit",
            Style::Bb => "bb",
            Style::Cal => "cal",
            Style::Scr => "scr",
            Style::BfCal => "bfcal",
            Style::BfScr => "bfscr",
            Style::Frak => "frak",
            Style::BfFrak => "bffrak",
            Style::Sf => "sf",
            Style::SfIt => "sfit",
            Style::BfSf => "bfsf",
            Style::BfSfIt => "bfsfit",
            Style::Tt => "tt",
        }
    }

    /// The style whose codepoints this one shares. `cal` and `scr` are one
    /// Unicode alphabet; so are their bold forms.
    pub fn codepoint_style(self) -> Style {
        match self {
            Style::Cal => Style::Scr,
            Style::BfCal => Style::BfScr,
            s => s,
        }
    }
}

impl fmt::Display for Style {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Style {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Style::ALL
            .iter()
            .copied()
            .find(|st| st.as_str() == s)
            .ok_or_else(|| format!("unknown style `{s}`"))
    }
}

/// What a spelling stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Atom {
    /// One Unicode character with no style dimension: `∈`, `≤`, `∑`, `α`.
    Named(char),
    /// A letter, digit or symbol under a math alphabet: `(R, bb)`, `(α, bfit)`.
    Styled { base: char, style: Style },
}

impl Atom {
    pub fn styled(base: char, style: Style) -> Atom {
        Atom::Styled { base, style }
    }

    /// A short stable key, used for config overrides and JSON output:
    /// the character itself for a `Named` atom, `style:base` for a styled one.
    pub fn key(&self) -> String {
        match self {
            Atom::Named(c) => c.to_string(),
            Atom::Styled { base, style } => format!("{style}:{base}"),
        }
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// True for the Greek letters, upper and lower case, plus the TeX variants.
pub fn is_greek(c: char) -> bool {
    matches!(c, '\u{0391}'..='\u{03A9}' | '\u{03B1}'..='\u{03C9}' | '\u{03D1}' | '\u{03D5}' | '\u{03D6}' | '\u{03DC}' | '\u{03DD}' | '\u{03F0}' | '\u{03F1}' | '\u{03F5}' | '\u{03F4}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_round_trips_through_strings() {
        for s in Style::ALL {
            assert_eq!(s.as_str().parse::<Style>().unwrap(), s);
        }
        assert!("bold".parse::<Style>().is_err());
    }

    #[test]
    fn keys() {
        assert_eq!(Atom::Named('∈').key(), "∈");
        assert_eq!(Atom::styled('R', Style::Bb).key(), "bb:R");
        assert!(is_greek('α') && is_greek('Ω') && is_greek('ϕ') && !is_greek('a'));
    }
}
