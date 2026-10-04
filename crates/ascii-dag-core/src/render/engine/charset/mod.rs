//! Charset dispatch — every charset is an equal decode table applied at
//! emission (temp/06 §3; charsets never appear in paint code).
//!
//! Adding a charset = adding a file here and one enum variant (N6b:
//! growth by addition).

pub(crate) mod ascii;
pub(crate) mod unicode;

use super::cell::Cell;

/// Output character set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Charset {
    /// Unicode box-drawing glyphs (default).
    #[default]
    Unicode,
    /// Plain ASCII projection (zigraph `toAscii` semantics).
    Ascii,
}

impl Charset {
    /// Decode one semantic cell to a glyph in this charset.
    #[inline]
    pub(crate) fn decode(self, cell: Cell) -> char {
        match self {
            Charset::Unicode => unicode::decode(cell),
            Charset::Ascii => ascii::decode(cell),
        }
    }

    /// The legend's "from → to" arrow in this charset.
    pub(crate) fn legend_arrow(self) -> &'static str {
        match self {
            Charset::Unicode => "\u{2192}",
            Charset::Ascii => "->",
        }
    }
}
