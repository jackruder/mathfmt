//! Math-mode tokens. Every byte of a math region belongs to exactly one token.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
    pub fn join(&self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokKind {
    /// `\` + ASCII letters.
    ControlWord,
    /// `\` + one other character (`\,` `\\` `\{` `\|` `\(`).
    ControlSymbol,
    Open,
    Close,
    Sup,
    Sub,
    Align,
    /// `#1`, or a bare `#`.
    Param,
    /// `$`.
    Dollar,
    /// One scalar value, with any combining marks that follow it.
    Char,
    /// Whitespace, or a `%` comment through its newline.
    Trivia,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Token {
    pub kind: TokKind,
    pub span: Span,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        self.span.text(src)
    }

    /// The name of a control word, without the backslash.
    pub fn name<'a>(&self, src: &'a str) -> &'a str {
        debug_assert!(matches!(
            self.kind,
            TokKind::ControlWord | TokKind::ControlSymbol
        ));
        &src[self.span.start + 1..self.span.end]
    }

    /// The character of a `Char` token (its first scalar).
    pub fn ch(&self, src: &str) -> char {
        self.text(src).chars().next().unwrap_or('\u{0}')
    }
}

pub fn is_combining(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
}

/// The token starting at `pos`, or `None` at end of input.
pub fn lex_one(src: &str, pos: usize) -> Option<Token> {
    let rest = &src[pos..];
    let mut chars = rest.char_indices();
    let (_, c) = chars.next()?;
    let span = |end: usize| Span::new(pos, pos + end);
    let tok = |kind, end| {
        Some(Token {
            kind,
            span: span(end),
        })
    };
    match c {
        '\\' => {
            let mut end = c.len_utf8();
            let mut letters = 0;
            for (i, d) in rest[end..].char_indices() {
                if d.is_ascii_alphabetic() {
                    letters += 1;
                    end = c.len_utf8() + i + d.len_utf8();
                } else {
                    if letters == 0 {
                        end += d.len_utf8();
                    }
                    break;
                }
            }
            if letters > 0 {
                tok(TokKind::ControlWord, end)
            } else {
                // `\` followed by one char, or a lone `\` at end of input
                tok(TokKind::ControlSymbol, end)
            }
        }
        '{' => tok(TokKind::Open, 1),
        '}' => tok(TokKind::Close, 1),
        '^' => tok(TokKind::Sup, 1),
        '_' => tok(TokKind::Sub, 1),
        '&' => tok(TokKind::Align, 1),
        '$' => tok(TokKind::Dollar, 1),
        '#' => {
            let mut end = 1;
            for d in rest[1..].chars() {
                if d.is_ascii_digit() {
                    end += 1;
                } else {
                    break;
                }
            }
            tok(TokKind::Param, end)
        }
        '%' => {
            let end = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
            tok(TokKind::Trivia, end)
        }
        c if c.is_whitespace() => {
            let mut end = 0;
            for (i, d) in rest.char_indices() {
                if d.is_whitespace() {
                    end = i + d.len_utf8();
                } else {
                    break;
                }
            }
            tok(TokKind::Trivia, end)
        }
        c => {
            let mut end = c.len_utf8();
            for (i, d) in rest[end..].char_indices() {
                if is_combining(d) {
                    end = c.len_utf8() + i + d.len_utf8();
                } else {
                    break;
                }
            }
            tok(TokKind::Char, end)
        }
    }
}

/// All tokens of a string, for `--emit tokens` and tests.
pub fn lex_all(src: &str) -> Vec<Token> {
    let mut v = Vec::new();
    let mut pos = 0;
    while let Some(t) = lex_one(src, pos) {
        pos = t.span.end;
        v.push(t);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(TokKind, &str)> {
        lex_all(src)
            .into_iter()
            .map(|t| (t.kind, t.text(src)))
            .collect()
    }

    #[test]
    fn control_sequences() {
        assert_eq!(
            kinds(r"\alpha\\(\,x\{"),
            vec![
                (TokKind::ControlWord, r"\alpha"),
                (TokKind::ControlSymbol, r"\\"),
                (TokKind::Char, "("),
                (TokKind::ControlSymbol, r"\,"),
                (TokKind::Char, "x"),
                (TokKind::ControlSymbol, r"\{"),
            ]
        );
    }

    #[test]
    fn unicode_letters_do_not_extend_control_words() {
        assert_eq!(
            kinds(r"\alphaβ"),
            vec![(TokKind::ControlWord, r"\alpha"), (TokKind::Char, "β")]
        );
    }

    #[test]
    fn params_comments_and_combining() {
        assert_eq!(
            kinds("#1 %c\n$x\u{0302}"),
            vec![
                (TokKind::Param, "#1"),
                (TokKind::Trivia, " "),
                (TokKind::Trivia, "%c\n"),
                (TokKind::Dollar, "$"),
                (TokKind::Char, "x\u{0302}"),
            ]
        );
    }

    #[test]
    fn every_byte_is_covered() {
        let src = r"a^{\mathbf x}_1 & \frac12 \\ % end";
        let toks = lex_all(src);
        let mut pos = 0;
        for t in &toks {
            assert_eq!(t.span.start, pos);
            pos = t.span.end;
        }
        assert_eq!(pos, src.len());
    }
}
