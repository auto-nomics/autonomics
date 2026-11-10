//! Explicit wrapped text edits using the same pinned metrics as scientific plans.
use crate::{
    layout,
    scene::{self, SceneError},
};
use serde_json::{Value, json};
fn fail(message: &str) -> SceneError {
    SceneError::InvalidText(message.into())
}
fn number(element: &Value, key: &str) -> Result<f64, SceneError> {
    element[key]
        .as_f64()
        .filter(|v| v.is_finite() && v.abs() <= 1e6)
        .ok_or_else(|| fail("invalid text/container geometry"))
}
fn align(value: &str, available: f64, used: f64) -> Result<f64, SceneError> {
    match value {
        "left" | "top" => Ok(0.),
        "center" | "middle" => Ok((available - used) / 2.),
        "right" | "bottom" => Ok(available - used),
        _ => Err(fail("unsupported text alignment")),
    }
}
pub(crate) fn apply(
    elements: &mut [Value],
    id: &str,
    source: &str,
    max_width: Option<f64>,
) -> Result<(), SceneError> {
    if source.trim().is_empty() || source.chars().count() > 2048 {
        return Err(fail(
            "requires 1..2048 visible text characters; use remove to delete a label",
        ));
    }
    if source.chars().any(|c| {
        !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
            && crate::font_metrics::ADVANCES
                .binary_search_by_key(&(c as u32), |(cp, _)| *cp)
                .is_err()
    }) {
        return Err(fail(
            "text contains characters outside the pinned scientific font metrics",
        ));
    }
    let index = elements
        .iter()
        .position(|element| element["id"].as_str() == Some(id))
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let original = &elements[index];
    if original["type"] != "text"
        || original["isDeleted"] == true
        || original["locked"] == true
        || original["fontFamily"] != 9
        || number(original, "angle")? != 0.
    {
        return Err(fail(
            "requires live unlocked unrotated scientific-font text (fontFamily 9)",
        ));
    }
    let size = number(original, "fontSize")?;
    let line_height = number(original, "lineHeight")?;
    if !(4. ..=200.).contains(&size) || !(0.5..=3.).contains(&line_height) {
        return Err(fail("font size or line height is out of range"));
    }
    let container_id = match original.get("containerId") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.as_str()),
        _ => return Err(fail("invalid text container identity")),
    };
    let container = container_id
        .map(|cid| {
            elements
                .iter()
                .find(|element| element["id"].as_str() == Some(cid))
                .ok_or_else(|| fail("text container is missing"))
        })
        .transpose()?;
    let mut origin = [number(original, "x")?, number(original, "y")?];
    let mut available_height = None;
    let mut arrow_center = None;
    let limit = if let Some(container) = container {
        if max_width.is_some() {
            return Err(fail(
                "bound text uses its native container width; resize the container explicitly",
            ));
        }
        if container["isDeleted"] == true
            || container["locked"] == true
            || number(container, "angle")? != 0.
        {
            return Err(fail("requires a live unlocked unrotated text container"));
        }
        let cid = container_id.unwrap();
        let owners = container["boundElements"]
            .as_array()
            .ok_or_else(|| fail("container has no reciprocal text binding"))?;
        if owners
            .iter()
            .filter(|entry| entry["type"] == "text")
            .count()
            != 1
            || !owners
                .iter()
                .any(|entry| entry["type"] == "text" && entry["id"] == id)
            || elements
                .iter()
                .filter(|e| {
                    e["type"] == "text" && e["isDeleted"] != true && e["containerId"] == cid
                })
                .count()
                != 1
        {
            return Err(fail("ambiguous or nonreciprocal text binding"));
        }
        let (x, y, w, h) = (
            number(container, "x")?,
            number(container, "y")?,
            number(container, "width")?,
            number(container, "height")?,
        );
        if w < 0. || h < 0. {
            return Err(fail("invalid container dimensions"));
        }
        match container["type"].as_str() {
            Some("rectangle") => {
                origin = [x + 5., y + 5.];
                available_height = Some(h - 10.);
                w - 10.
            }
            Some("ellipse") => {
                origin = [
                    x + 5. + w / 2. * (1. - 2_f64.sqrt() / 2.),
                    y + 5. + h / 2. * (1. - 2_f64.sqrt() / 2.),
                ];
                available_height = Some((h / 2. * 2_f64.sqrt()).round() - 10.);
                (w / 2. * 2_f64.sqrt()).round() - 10.
            }
            Some("diamond") => {
                origin = [x + 5. + w / 4., y + 5. + h / 4.];
                available_height = Some((h / 2.).round() - 10.);
                (w / 2.).round() - 10.
            }
            Some("arrow") => {
                let points = container["points"]
                    .as_array()
                    .ok_or_else(|| fail("arrow points missing"))?;
                if points.len() < 2
                    || container["elbowed"] == true
                    || (points.len() > 2
                        && (!container["roundness"].is_null()
                            || container["roughness"].as_f64() != Some(0.)))
                {
                    return Err(fail(
                        "curved/elbowed caption geometry requires native editing",
                    ));
                }
                let points = points
                    .iter()
                    .map(|p| {
                        let p = p
                            .as_array()
                            .filter(|p| p.len() == 2)
                            .ok_or_else(|| fail("invalid arrow point"))?;
                        Ok([
                            p[0].as_f64()
                                .filter(|v| v.is_finite() && v.abs() <= 1e6)
                                .ok_or_else(|| fail("invalid arrow point"))?,
                            p[1].as_f64()
                                .filter(|v| v.is_finite() && v.abs() <= 1e6)
                                .ok_or_else(|| fail("invalid arrow point"))?,
                        ])
                    })
                    .collect::<Result<Vec<_>, SceneError>>()?;
                arrow_center = Some(scene::path_label_center(&points, x, y));
                (w * 0.7).max(size * 11.)
            }
            _ => return Err(fail("unsupported bound text container")),
        }
    } else {
        max_width.unwrap_or(number(original, "width")?)
    };
    if !limit.is_finite() || !(1. ..=20000.).contains(&limit) {
        return Err(fail("text width is out of range"));
    }
    let wrapped = layout::wrap(source, limit, size);
    if wrapped
        .split('\n')
        .any(|line| layout::text_width(line, size) > limit + 1e-6)
    {
        return Err(fail("a glyph does not fit the requested width"));
    }
    let height = wrapped.split('\n').count() as f64 * size * line_height;
    if height > 20000. || available_height.is_some_and(|available| height > available + 1e-6) {
        return Err(fail(
            "wrapped text does not fit the existing container height; explicitly resize the container or edit the font size",
        ));
    }
    let width = limit;
    let (x, y) = if let Some(center) = arrow_center {
        (center[0] - width / 2., center[1] - height / 2.)
    } else if container.is_some() {
        (
            origin[0]
                + align(
                    original["textAlign"].as_str().unwrap_or("center"),
                    limit,
                    width,
                )?,
            origin[1]
                + align(
                    original["verticalAlign"].as_str().unwrap_or("middle"),
                    available_height.unwrap(),
                    height,
                )?,
        )
    } else {
        (
            origin[0]
                + align(
                    original["textAlign"].as_str().unwrap_or("left"),
                    number(original, "width")?,
                    width,
                )?,
            origin[1],
        )
    };
    if let Some(frame_id) = original["frameId"].as_str() {
        let frame = elements
            .iter()
            .find(|e| {
                e["id"] == frame_id
                    && e["type"] == "frame"
                    && e["isDeleted"] != true
                    && e["locked"] != true
            })
            .ok_or_else(|| fail("frame is missing or locked"))?;
        let (fx, fy, fw, fh) = (
            number(frame, "x")?,
            number(frame, "y")?,
            number(frame, "width")?,
            number(frame, "height")?,
        );
        if number(frame, "angle")? != 0.
            || x < fx
            || y < fy
            || x + width > fx + fw
            || y + height > fy + fh
        {
            return Err(fail("wrapped text would leave its existing frame"));
        }
    }
    if ![x, y, width, height]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 1e6)
    {
        return Err(fail("resulting text geometry is out of range"));
    }
    let target = &mut elements[index];
    target["text"] = json!(wrapped);
    target["originalText"] = json!(source);
    target["width"] = json!(width);
    target["height"] = json!(height);
    target["x"] = json!(x);
    target["y"] = json!(y);
    target["autoResize"] = json!(false);
    scene::touch(target);
    Ok(())
}
