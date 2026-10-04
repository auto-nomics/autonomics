//! ASCII decode table — semantic cells → plain-ASCII glyphs.
//!
//! An equal projection of the composed canvas (the charset ruling):
//! ASCII output never passes through Unicode glyphs first. Semantics
//! match zigraph's `toAscii` so both libraries produce identical ASCII:
//!
//! - arrows → `v ^ > <`, self-loop → `@`, dummy marker → `o`
//! - vertical strokes (any weight) → `|`
//! - horizontal strokes → `-`, or `=` when double-weight
//! - anything with both a vertical and a horizontal arm → `+`
//! - text passes through untouched (user content is not transliterated)

use super::super::cell::{Cell, Dir, MarkerKind, Weight};

/// Decode one cell to its ASCII glyph.
pub(crate) fn decode(cell: Cell) -> char {
    if cell.is_empty() {
        return ' ';
    }
    if cell.is_text() {
        return cell.text_char();
    }
    if cell.is_marker() {
        return match cell.marker_kind() {
            MarkerKind::SelfLoop => '@',
            MarkerKind::Dummy => 'o',
            MarkerKind::Arrow => match cell.marker_dir() {
                Dir::Down => 'v',
                Dir::Up => '^',
                Dir::Right => '>',
                Dir::Left => '<',
            },
        };
    }

    let (up, down, left, right) = cell.arms();
    let vertical = up != Weight::None || down != Weight::None;
    let horizontal = left != Weight::None || right != Weight::None;
    match (vertical, horizontal) {
        (true, true) => '+',
        (true, false) => '|',
        (false, true) => {
            if left == Weight::Double || right == Weight::Double {
                '='
            } else {
                '-'
            }
        }
        (false, false) => ' ',
    }
}
