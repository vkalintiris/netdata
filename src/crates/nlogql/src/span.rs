//! Byte-range spans into the original query string.

use std::ops::Range;

/// Half-open byte range `[start, end)` into the source query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn as_range(&self) -> Range<usize> {
        self.start..self.end
    }
}

impl From<Range<usize>> for Span {
    fn from(r: Range<usize>) -> Self {
        Self::new(r.start, r.end)
    }
}

impl From<Span> for Range<usize> {
    fn from(s: Span) -> Self {
        s.as_range()
    }
}

impl From<chumsky::span::SimpleSpan> for Span {
    fn from(s: chumsky::span::SimpleSpan) -> Self {
        Self::new(s.start, s.end)
    }
}
