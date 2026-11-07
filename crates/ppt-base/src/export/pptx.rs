//! PPTX writer lands in the next milestone; stub keeps the tree green.

use crate::error::{Error, Result};

/// Build a minimal OOXML presentation from full-bleed PNG frames.
/// Implemented in the PPTX milestone.
pub fn build_pptx(
    _pngs: &[Vec<u8>],
    _canvas_width: u32,
    _canvas_height: u32,
    _deck_id: String,
) -> Result<Vec<u8>> {
    Err(Error::Export("pptx writer not implemented yet".into()))
}
