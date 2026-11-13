//! Explicit shrink-only frame fitting. Child geometry and ownership never change.
use crate::{
    geometry_review,
    scene::{self, SceneError},
};
use serde_json::{Value, json};

pub(crate) fn fit(elements: &mut [Value], id: &str, padding: f64) -> Result<(), SceneError> {
    let fail = |message: &str| SceneError::InvalidUpdate(format!("fit_frame: {message}"));
    if !padding.is_finite() || !(8. ..=128.).contains(&padding) {
        return Err(fail("padding must be a finite number within 8..128"));
    }
    let index = elements
        .iter()
        .position(|e| e["id"] == id)
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let frame = &elements[index];
    if frame["type"] != "frame"
        || frame["isDeleted"] == true
        || frame["locked"] == true
        || frame["angle"].as_f64().unwrap_or(0.) != 0.
        || !frame["frameId"].is_null()
    {
        return Err(fail("requires a live unlocked unrotated top-level frame"));
    }
    let bounds = geometry_review::extents(frame).ok_or_else(|| fail("invalid frame bounds"))?;
    let width = frame["width"].as_f64().unwrap();
    let height = frame["height"].as_f64().unwrap();
    if width <= 0. || height <= 0. {
        return Err(fail("frame dimensions must be positive"));
    }
    let mut right = bounds[0];
    let mut bottom = bounds[1];
    let mut count = 0;
    for child in elements
        .iter()
        .filter(|e| e["frameId"] == id && e["isDeleted"] != true)
    {
        count += 1;
        if matches!(child["type"].as_str(), Some("frame" | "magicframe")) {
            return Err(fail("nested frames require explicit editing"));
        }
        let b = geometry_review::extents(child)
            .ok_or_else(|| fail("a child has unsupported or invalid bounds; nothing changed"))?;
        if b[0] < bounds[0] - 1e-6
            || b[1] < bounds[1] - 1e-6
            || b[2] > bounds[2] + 1e-6
            || b[3] > bounds[3] + 1e-6
        {
            return Err(fail(
                "a child extends outside the frame; reconcile membership or geometry first",
            ));
        }
        right = right.max(b[2]);
        bottom = bottom.max(b[3]);
    }
    if count == 0 {
        return Err(fail("frame has no live children"));
    }
    // Preserve the origin and never grow. Padding is retained where the existing
    // frame permits it; fitting does not silently move content to create room.
    let next_width = (right - bounds[0] + padding).max(64.).min(width);
    let next_height = (bottom - bounds[1] + padding).max(64.).min(height);
    if (next_width - width).abs() > 1e-6 || (next_height - height).abs() > 1e-6 {
        elements[index]["width"] = json!(next_width);
        elements[index]["height"] = json!(next_height);
        scene::touch(&mut elements[index]);
    }
    Ok(())
}
