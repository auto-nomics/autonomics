//! Unicode decode table — semantic cells → box-drawing glyphs.
//!
//! Light arm patterns delegate to [`crate::render::chars::mask_to_char`]
//! so the engine and the legacy renderers share one source of truth
//! during the migration. Mixed light/double patterns reproduce the
//! legacy border-merge glyphs **byte-for-byte** (see the pinning notes
//! below) — where the legacy tables were approximate (`┤` + a vertical
//! border shows `╡`, not the typographically exact `╣`), we match the
//! legacy choice, because R2.1 demands byte-identical output.

use super::super::cell::{Cell, Dir, MarkerKind, Weight};
use crate::render::chars::{DIR_DOWN, DIR_LEFT, DIR_RIGHT, DIR_UP, mask_to_char};

/// Decode one cell to its Unicode glyph.
pub(crate) fn decode(cell: Cell) -> char {
    if cell.is_empty() {
        return ' ';
    }
    if cell.is_text() {
        return cell.text_char();
    }
    if cell.is_marker() {
        return decode_marker(cell);
    }
    let (up, down, left, right) = cell.arms();
    decode_stroke(up, down, left, right)
}

fn decode_marker(cell: Cell) -> char {
    match cell.marker_kind() {
        MarkerKind::SelfLoop => '↺',
        MarkerKind::Dummy => '◍', // zigraph parity
        MarkerKind::Arrow => match (cell.marker_dir(), cell.marker_dashed()) {
            (Dir::Down, false) => '↓',
            (Dir::Down, true) => '⇣',
            (Dir::Up, false) => '↑',
            (Dir::Up, true) => '⇡',
            (Dir::Right, false) => '→',
            (Dir::Right, true) => '⇢',
            (Dir::Left, false) => '←',
            (Dir::Left, true) => '⇠',
        },
    }
}

fn decode_stroke(up: Weight, down: Weight, left: Weight, right: Weight) -> char {
    use Weight as W;

    let vertical_only = left == W::None && right == W::None;
    let horizontal_only = up == W::None && down == W::None;

    // Pure dashed lines keep their dashed glyphs; any other pattern
    // folds dashed arms to light (legacy: corners and junctions have no
    // dashed variants — `to_dashed` keeps them solid, and a dashed
    // stroke crossed by anything renders as a solid junction).
    let any_dashed =
        up == W::Dashed || down == W::Dashed || left == W::Dashed || right == W::Dashed;
    if any_dashed {
        let pure_dashed_v = vertical_only && up != W::Light && down != W::Light;
        if pure_dashed_v && up != W::Double && down != W::Double {
            return '┊';
        }
        let pure_dashed_h = horizontal_only && left != W::Light && right != W::Light;
        if pure_dashed_h && left != W::Double && right != W::Double {
            return '┈';
        }
        let fold = |w: W| if w == W::Dashed { W::Light } else { w };
        return decode_stroke(fold(up), fold(down), fold(left), fold(right));
    }

    let any_double =
        up == W::Double || down == W::Double || left == W::Double || right == W::Double;
    if !any_double {
        // All-light pattern: exact legacy table via the shared bitmask.
        let mut mask = 0u8;
        if up != W::None {
            mask |= DIR_UP;
        }
        if down != W::None {
            mask |= DIR_DOWN;
        }
        if left != W::None {
            mask |= DIR_LEFT;
        }
        if right != W::None {
            mask |= DIR_RIGHT;
        }
        return mask_to_char(mask);
    }

    // Double-bearing patterns. `L` = light arm, `D` = double arm,
    // `N` = none. Ordered: pure doubles, then the mixed junctions the
    // legacy border tables produce (pinned to their glyph choices).
    let key = |w: W| -> u8 {
        match w {
            W::None => 0,
            W::Double => 2,
            _ => 1,
        }
    };
    match (key(up), key(down), key(left), key(right)) {
        // Pure double lines / corners / junctions.
        (2, 2, 0, 0) | (2, 0, 0, 0) | (0, 2, 0, 0) => '║',
        (0, 0, 2, 2) | (0, 0, 2, 0) | (0, 0, 0, 2) => '═',
        (0, 2, 0, 2) => '╔',
        (0, 2, 2, 0) => '╗',
        (2, 0, 0, 2) => '╚',
        (2, 0, 2, 0) => '╝',
        (2, 2, 0, 2) => '╠',
        (2, 2, 2, 0) => '╣',
        (0, 2, 2, 2) => '╦',
        (2, 0, 2, 2) => '╩',
        (2, 2, 2, 2) => '╬',
        // Light vertical × double horizontal (legacy `merge_h_border`).
        (1, 1, 2, 2) | (1, 1, 2, 0) | (1, 1, 0, 2) => '╪',
        (0, 1, 2, 2) | (0, 1, 2, 0) | (0, 1, 0, 2) => '╤',
        (1, 0, 2, 2) | (1, 0, 2, 0) | (1, 0, 0, 2) => '╧',
        // Double vertical × light horizontal (legacy `merge_v_border`;
        // pinned to the legacy glyph choices, see module docs).
        (2, 2, 1, 1) | (2, 0, 1, 1) | (0, 2, 1, 1) => '╫',
        (2, 2, 0, 1) | (2, 0, 0, 1) | (0, 2, 0, 1) => '╞',
        (2, 2, 1, 0) | (2, 0, 1, 0) | (0, 2, 1, 0) => '╡',
        // Double + light on the same axis after a partial merge — treat
        // the light arm as absorbed (closest pure-double glyph).
        (a, b, c, d) => {
            // No exact glyph exists for this mix (e.g. a box corner with
            // a light stroke passing through). Decode the double-arm
            // subset only — borders visually dominate, and the stroke
            // resumes on the neighboring cells (the legacy corner look).
            let fold = |k: u8| -> Weight {
                match k {
                    2 => W::Double,
                    _ => W::None,
                }
            };
            decode_double_fallback(fold(a), fold(b), fold(c), fold(d))
        }
    }
}

fn decode_double_fallback(up: Weight, down: Weight, left: Weight, right: Weight) -> char {
    use Weight as W;
    // No exact glyph exists for this mix. Decode the double-arm subset
    // only — borders visually dominate a light stroke passing through
    // (e.g. a box corner crossed by an edge decodes as the corner, the
    // stroke resuming on the next row — the legacy renderers' look).
    let keep = |w: W| if w == W::Double { W::Double } else { W::None };
    let (u, d2, l, r) = (keep(up), keep(down), keep(left), keep(right));
    match (u, d2, l, r) {
        (W::Double, W::None, W::None, W::None) | (W::None, W::Double, W::None, W::None) => '║',
        (W::None, W::None, W::Double, W::None) | (W::None, W::None, W::None, W::Double) => '═',
        (W::Double, W::Double, W::None, W::None) => '║',
        (W::None, W::None, W::Double, W::Double) => '═',
        (W::None, W::Double, W::None, W::Double) => '╔',
        (W::None, W::Double, W::Double, W::None) => '╗',
        (W::Double, W::None, W::None, W::Double) => '╚',
        (W::Double, W::None, W::Double, W::None) => '╝',
        (W::None, W::Double, W::Double, W::Double) => '╦',
        (W::Double, W::None, W::Double, W::Double) => '╩',
        (W::Double, W::Double, W::None, W::Double) => '╠',
        (W::Double, W::Double, W::Double, W::None) => '╣',
        (W::Double, W::Double, W::Double, W::Double) => '╬',
        _ => ' ',
    }
}
