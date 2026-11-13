//! Conservative pixel-grid hint over a prepared SVG preview, never molecular geometry.
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
pub(crate) fn preview_alpha_bounds(data_url: &str) -> Result<Value> {
    let encoded = data_url
        .strip_prefix("data:image/png;base64,")
        .ok_or(Error::Invalid("invalid prepared SVG preview"))?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| Error::Invalid("invalid prepared SVG preview"))?;
    let pixels = image::load_from_memory(&bytes)
        .map_err(|_| Error::Invalid("invalid prepared SVG preview"))?
        .to_rgba8();
    let (width, height) = pixels.dimensions();
    if width == 0 || height == 0 || width > 768 || height > 768 {
        return Err(Error::Invalid(
            "prepared SVG preview exceeds alpha-bounds limits",
        ));
    }
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    let mut count = 0_u32;
    for (x, y, pixel) in pixels.enumerate_pixels() {
        if pixel[3] > 0 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
            count += 1;
        }
    }
    let (pixel_bounds, normalized_bounds) = if count == 0 {
        (Value::Null, Value::Null)
    } else {
        let x = left.saturating_sub(1);
        let y = top.saturating_sub(1);
        let r = right.saturating_add(1).min(width);
        let b = bottom.saturating_add(1).min(height);
        (
            json!([left, top, right - left, bottom - top]),
            json!([
                f64::from(x) / f64::from(width),
                f64::from(y) / f64::from(height),
                f64::from(r - x) / f64::from(width),
                f64::from(b - y) / f64::from(height)
            ]),
        )
    };
    Ok(
        json!({"status":if count==0 {"empty"} else {"available"},"scope":"uncropped_file_normalized_svg_preview","method":"nonzero_8bit_alpha_with_one_preview_pixel_padding","preview_width":width,"preview_height":height,"painted_pixel_count":count,"pixel_bounds":pixel_bounds,"normalized_bounds":normalized_bounds,"padding_preview_pixels":1,"limitations":"Preview-grid approximation, not exact vector/glyph geometry or scientific validation. Includes opaque backgrounds and all visible decorations. Native crop/rotation/flip/element opacity are not applied; inspect the actual candidate PNG before fitting."}),
    )
}
