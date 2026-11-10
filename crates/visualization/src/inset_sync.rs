//! Reposition decorations around the frozen source pixels without refreshing the detail.
use crate::{
    inset, inset_refresh,
    scene::{self, SceneError},
};
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
fn invalid(message: impl ToString) -> SceneError {
    SceneError::InvalidInset(message.to_string())
}
fn number(e: &Value, key: &str) -> Result<f64, SceneError> {
    e[key]
        .as_f64()
        .filter(|v| v.is_finite() && v.abs() <= 1e6)
        .ok_or_else(|| invalid("invalid image geometry"))
}
pub(crate) fn apply(elements: &mut [Value], files: &Value, id: &str) -> Result<(), SceneError> {
    let document = json!({"elements":elements,"files":files});
    let m = inset_refresh::members(&document, id).map_err(invalid)?;
    if [m.anchor, m.region, m.callout]
        .iter()
        .any(|e| e["locked"] == true)
    {
        return Err(invalid("inset decorations are locked"));
    }
    let source = elements
        .iter()
        .find(|e| {
            e["id"] == m.reference["source_element_id"]
                && e["isDeleted"] != true
                && e["type"] == "image"
        })
        .ok_or_else(|| invalid("source image is missing"))?;
    if source["fileId"] != m.reference["source_file_id"] {
        return Err(invalid(
            "source file changed; review a detail refresh instead",
        ));
    }
    let file_id = source["fileId"]
        .as_str()
        .ok_or_else(|| invalid("missing source file"))?;
    let data = files[file_id]["dataURL"]
        .as_str()
        .and_then(|s| s.split_once(";base64,"))
        .ok_or_else(|| invalid("source requires a base64 image"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.1)
        .map_err(invalid)?;
    if format!("{:x}", Sha256::digest(&bytes))
        != m.reference["source_sha256"].as_str().unwrap_or("")
    {
        return Err(invalid(
            "source bytes changed; review a detail refresh instead",
        ));
    }
    let frozen = &m.reference["crop"];
    let natural = |key| {
        number(frozen, key).and_then(|v| {
            if v >= 1. && v.fract() == 0. && v <= u32::MAX as f64 {
                Ok(v as u32)
            } else {
                Err(invalid("invalid source dimensions"))
            }
        })
    };
    let visible = inset::crop(source, natural("naturalWidth")?, natural("naturalHeight")?)
        .map_err(invalid)?;
    let mut u = (number(frozen, "x")? - visible[0]) / visible[2];
    let mut v = (number(frozen, "y")? - visible[1]) / visible[3];
    let rw = number(frozen, "width")? / visible[2];
    let rh = number(frozen, "height")? / visible[3];
    if u < -1e-9 || v < -1e-9 || rw <= 0. || rh <= 0. || u + rw > 1. + 1e-9 || v + rh > 1. + 1e-9 {
        return Err(invalid("frozen region is outside the current source crop"));
    }
    let scale = source["scale"]
        .as_array()
        .filter(|s| s.len() == 2 && s.iter().all(|v| v == 1 || v == -1))
        .ok_or_else(|| invalid("invalid source flip"))?;
    if scale[0] == -1 {
        u = 1. - u - rw;
    }
    if scale[1] == -1 {
        v = 1. - v - rh;
    }
    let w = number(source, "width")?;
    let h = number(source, "height")?;
    let angle = number(source, "angle")?;
    if w <= 0.
        || h <= 0.
        || number(m.anchor, "angle")?.abs() > 1e-9
        || m.callout["elbowed"] == true
        || m.callout["points"]
            .as_array()
            .is_none_or(|points| points.len() != 2)
        || number(m.callout, "angle")?.abs() > 1e-9
    {
        return Err(invalid(
            "requires a valid source and an unrotated detail panel with a straight callout",
        ));
    }
    let delta = [(u + rw / 2. - 0.5) * w, (v + rh / 2. - 0.5) * h];
    let (s, c) = angle.sin_cos();
    let center = [
        number(source, "x")? + w / 2. + c * delta[0] - s * delta[1],
        number(source, "y")? + h / 2. + s * delta[0] + c * delta[1],
    ];
    let size = [rw * w, rh * h];
    let panel_size = [number(m.anchor, "width")?, number(m.anchor, "height")?];
    let panel = [
        number(m.anchor, "x")? + panel_size[0] / 2.,
        number(m.anchor, "y")? + panel_size[1] / 2.,
    ];
    if size.iter().chain(panel_size.iter()).any(|v| *v < 1e-6)
        || (center[0] - panel[0]).hypot(center[1] - panel[1]) < 1.
    {
        return Err(invalid("degenerate region or coincident panel"));
    }
    let start = inset::edge(center, size, angle, panel);
    let end = inset::edge(panel, panel_size, 0., center);
    let mut region = m.region.clone();
    region["x"] = json!(center[0] - size[0] / 2.);
    region["y"] = json!(center[1] - size[1] / 2.);
    region["width"] = json!(size[0]);
    region["height"] = json!(size[1]);
    region["angle"] = json!(angle);
    region["frameId"] = source["frameId"].clone();
    let mut callout = m.callout.clone();
    callout["x"] = json!(start[0]);
    callout["y"] = json!(start[1]);
    callout["width"] = json!((end[0] - start[0]).abs());
    callout["height"] = json!((end[1] - start[1]).abs());
    callout["angle"] = json!(0.);
    callout["points"] = json!([[0., 0.], [end[0] - start[0], end[1] - start[1]]]);
    callout["startBinding"] = json!({"elementId":m.region["id"],"focus":0,"gap":8});
    callout["endBinding"] = json!({"elementId":m.anchor["id"],"focus":0,"gap":8});
    for mut replacement in [region, callout] {
        for key in ["x", "y", "width", "height"] {
            number(&replacement, key)?;
        }
        scene::touch(&mut replacement);
        let target = elements
            .iter_mut()
            .find(|e| e["id"] == replacement["id"])
            .unwrap();
        *target = replacement;
    }
    Ok(())
}
