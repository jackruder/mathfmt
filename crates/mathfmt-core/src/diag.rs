//! Diagnostics and source positions.

use std::fmt;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
        })
    }
}

/// A message about one place in one file. The offset is a byte offset; the
/// line and column are filled in by [`Diagnostic::locate`] before printing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub level: Level,
    pub code: &'static str,
    pub message: String,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub line: usize,
    pub column: usize,
}

impl Diagnostic {
    pub fn new(
        level: Level,
        code: &'static str,
        offset: usize,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            level,
            code,
            message: message.into(),
            offset,
            file: None,
            line: 0,
            column: 0,
        }
    }

    pub fn warning(code: &'static str, offset: usize, message: impl Into<String>) -> Self {
        Self::new(Level::Warning, code, offset, message)
    }

    pub fn info(code: &'static str, offset: usize, message: impl Into<String>) -> Self {
        Self::new(Level::Info, code, offset, message)
    }

    pub fn error(code: &'static str, offset: usize, message: impl Into<String>) -> Self {
        Self::new(Level::Error, code, offset, message)
    }

    /// Fill in file, line and column from the source the offset points into.
    pub fn locate(mut self, file: Option<&str>, index: &LineIndex) -> Self {
        let (line, column) = index.line_col(self.offset);
        self.file = file.map(str::to_string);
        self.line = line;
        self.column = column;
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.file {
            Some(file) => write!(f, "{file}:{}:{}: ", self.line, self.column)?,
            None => write!(f, "{}:{}: ", self.line, self.column)?,
        }
        write!(f, "{}[{}]: {}", self.level, self.code, self.message)
    }
}

/// Byte offset → 1-based line and column (column counted in characters).
#[derive(Debug)]
pub struct LineIndex {
    line_starts: Vec<usize>,
    text_len: usize,
    src: String,
}

impl LineIndex {
    pub fn new(src: &str) -> LineIndex {
        let mut line_starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        LineIndex {
            line_starts,
            text_len: src.len(),
            src: src.to_string(),
        }
    }

    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text_len);
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line];
        let col = self.src[start..offset].chars().count() + 1;
        (line + 1, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_count_characters() {
        let idx = LineIndex::new("ab\nαβ\n");
        assert_eq!(idx.line_col(0), (1, 1));
        assert_eq!(idx.line_col(2), (1, 3));
        assert_eq!(idx.line_col(3), (2, 1));
        assert_eq!(idx.line_col(5), (2, 2)); // after α (2 bytes)
        assert_eq!(idx.line_col(100), (3, 1));
    }

    #[test]
    fn display() {
        let d = Diagnostic::warning("x", 0, "m").locate(Some("f.tex"), &LineIndex::new("a"));
        assert_eq!(d.to_string(), "f.tex:1:1: warning[x]: m");
    }
}
