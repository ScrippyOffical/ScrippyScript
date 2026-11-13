//! Byte ranges into the source text.

use std::fmt;
use std::ops::Range;

/// Half-open byte range into the original source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub fn range(self) -> Range<usize> {
        self.start..self.end
    }

    /// Move a span that was lexed from a slice so it points into the full source.
    pub fn offset(self, base: usize) -> Span {
        Span::new(self.start + base, self.end + base)
    }
}

impl From<Range<usize>> for Span {
    fn from(range: Range<usize>) -> Self {
        Span::new(range.start, range.end)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// 1-based line and column of a byte offset. Columns count characters, not bytes.
pub fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(src.len());
    while offset > 0 && !src.is_char_boundary(offset) {
        offset -= 1;
    }

    let before = &src[..offset];
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = src[line_start..offset].chars().count() + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_are_one_based() {
        let src = "let x = 1\nlet y = 2";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 8), (1, 9));
        assert_eq!(line_col(src, 10), (2, 1));
        assert_eq!(line_col(src, 13), (2, 4));
    }

    #[test]
    fn columns_count_chars() {
        let src = "let s = \"héllo\" x";
        let x = src.find('x').unwrap();
        assert_eq!(line_col(src, x), (1, 17));
    }
}
