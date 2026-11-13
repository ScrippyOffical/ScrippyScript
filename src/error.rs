//! Lexer and parser errors, plus a small source snippet renderer.

use std::error::Error;
use std::fmt;

use crate::span::{line_col, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexErrorKind {
    UnexpectedChar(char),
    UnterminatedString,
    UnterminatedComment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub span: Span,
    pub kind: LexErrorKind,
}

impl LexError {
    pub fn new(span: Span, kind: LexErrorKind) -> Self {
        Self { span, kind }
    }

    pub fn render(&self, src: &str) -> String {
        render_at(src, self.span, &self.kind.to_string())
    }
}

impl fmt::Display for LexErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexErrorKind::UnexpectedChar(c) => write!(f, "unexpected character `{c}`"),
            LexErrorKind::UnterminatedString => write!(f, "unterminated string literal"),
            LexErrorKind::UnterminatedComment => write!(f, "unterminated block comment"),
        }
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl Error for LexError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    Unexpected {
        found: String,
        expected: String,
    },
    UnexpectedEof {
        expected: String,
    },
    /// Well-formed syntax that this parser does not handle yet. Holds the
    /// complete message, e.g. "event handlers aren't supported yet".
    Unsupported(&'static str),
    BadNumber(String),
    BadEscape(char),
    UnterminatedInterpolation,
    /// A lexer error inside a `${...}` fragment.
    BadInterpolation(String),
    /// Left side of an assignment is not a name, member or index.
    InvalidAssignTarget,
    /// Nested expressions/blocks past the parser's limit. Guards the host stack
    /// against pathological input instead of aborting the process.
    TooDeep,
    /// Map keys are stored as plain strings, so `${...}` has nowhere to go.
    BadMapKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub kind: ParseErrorKind,
}

impl ParseError {
    pub fn new(span: Span, kind: ParseErrorKind) -> Self {
        Self { span, kind }
    }

    pub fn unexpected(span: Span, found: impl Into<String>, expected: impl Into<String>) -> Self {
        Self::new(
            span,
            ParseErrorKind::Unexpected {
                found: found.into(),
                expected: expected.into(),
            },
        )
    }

    pub fn eof(span: Span, expected: impl Into<String>) -> Self {
        Self::new(
            span,
            ParseErrorKind::UnexpectedEof {
                expected: expected.into(),
            },
        )
    }

    pub fn unsupported(span: Span, feature: &'static str) -> Self {
        Self::new(span, ParseErrorKind::Unsupported(feature))
    }

    pub fn bad_number(span: Span, raw: &str) -> Self {
        Self::new(span, ParseErrorKind::BadNumber(raw.to_string()))
    }

    pub fn bad_escape(span: Span, escaped: char) -> Self {
        Self::new(span, ParseErrorKind::BadEscape(escaped))
    }

    pub fn invalid_target(span: Span) -> Self {
        Self::new(span, ParseErrorKind::InvalidAssignTarget)
    }

    pub fn render(&self, src: &str) -> String {
        render_at(src, self.span, &self.kind.to_string())
    }
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseErrorKind::Unexpected { found, expected } => {
                write!(f, "expected {expected}, found {found}")
            }
            ParseErrorKind::UnexpectedEof { expected } => {
                write!(f, "unexpected end of input, expected {expected}")
            }
            ParseErrorKind::Unsupported(message) => f.write_str(message),
            ParseErrorKind::BadNumber(raw) => write!(f, "invalid number literal `{raw}`"),
            ParseErrorKind::BadEscape(c) => write!(f, "unknown escape sequence `\\{c}`"),
            ParseErrorKind::UnterminatedInterpolation => {
                write!(f, "unterminated `${{...}}` interpolation")
            }
            ParseErrorKind::BadInterpolation(detail) => {
                write!(f, "invalid interpolation: {detail}")
            }
            ParseErrorKind::InvalidAssignTarget => {
                write!(f, "cannot assign to this expression")
            }
            ParseErrorKind::TooDeep => write!(f, "expression or block nesting is too deep"),
            ParseErrorKind::BadMapKey => {
                write!(f, "map keys cannot contain `${{...}}` interpolation")
            }
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl Error for ParseError {}

/// Formats `src` with a caret under `span`.
fn render_at(src: &str, span: Span, message: &str) -> String {
    let (line, column) = line_col(src, span.start);
    let text = src.lines().nth(line - 1).unwrap_or("");

    // Keep tabs so the caret lines up with tab-indented source, and count
    // characters rather than bytes for the caret width.
    let pad: String = text
        .chars()
        .take(column - 1)
        .map(|c| if c == '\t' { '\t' } else { ' ' })
        .collect();
    let width = src
        .get(span.range())
        .map_or(1, |slice| slice.chars().take_while(|c| *c != '\n').count())
        .max(1);

    format!(
        "{line}:{column}: {message}\n  {text}\n  {pad}{}",
        "^".repeat(width)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_points_at_the_span() {
        let src = "let x = 1\nlet y = $";
        let err = ParseError::unexpected(Span::new(18, 19), "`$`", "an expression");
        let out = err.render(src);
        assert!(out.starts_with("2:9: expected an expression, found `$`"));
        assert!(out.contains("let y = $"));
        assert!(out.ends_with("        ^"));
    }
}
