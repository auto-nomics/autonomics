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
        .ok_or_else(|| {
            fail(&format!(
                "{}.{} must be a finite JSON number with absolute value at most 1000000; explicitly repair this field before reflowing text",
                element["id"].as_str().unwrap_or("<unknown element>"),
                key
            ))
        })
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
    font_size: Option<f64>,
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
    let size = font_size.unwrap_or(number(original, "fontSize")?);
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
        if max_width.is_some() && container["type"] != "arrow" {
            return Err(fail(&format!(
                "bound text {id} uses its native container width from {}; omit max_width for shape-bound text and retry with text/font_size only. For a different node width, resize the container explicitly before reflowing",
                container_id.unwrap()
            )));
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
                // Arrow route bounds are independent of the caption budget.
                // Preserve that budget by default; only an explicit max_width
                // authorizes widening/narrowing the label, never its connector.
                max_width.unwrap_or(number(original, "width")?)
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
    target["fontSize"] = json!(size);
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

/// Construct a complete native element; never coerce opaque native JSON fields.
pub(crate) fn add(
    elements: &mut Vec<Value>,
    id: &str,
    source: &str,
    origin: [f64; 2],
    max_width: f64,
    font_size: Option<f64>,
    frame_id: Option<&str>,
) -> Result<(), SceneError> {
    if id.trim().is_empty() || id.len() > 512 {
        return Err(fail("text ID requires 1..512 bytes"));
    }
    let mut text = crate::sketch::text(id, source, origin[0], origin[1], max_width);
    text["textAlign"] = json!("left");
    text["verticalAlign"] = json!("top");
    text["frameId"] = json!(frame_id);
    let mut scratch = frame_id
        .and_then(|frame| elements.iter().find(|e| e["id"].as_str() == Some(frame)))
        .cloned()
        .into_iter()
        .collect::<Vec<_>>();
    scratch.push(text);
    apply(
        &mut scratch,
        id,
        source,
        Some(max_width),
        Some(font_size.unwrap_or(16.)),
    )?;
    elements.push(scratch.pop().unwrap());
    Ok(())
}

/// Explicitly tighten a current text box without rebuilding text or ownership.
pub(crate) fn fit_width(elements: &mut [Value], id: &str) -> Result<(), SceneError> {
    let index = elements
        .iter()
        .position(|e| e["id"].as_str() == Some(id))
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let original = &elements[index];
    if original["type"] != "text"
        || original["isDeleted"] == true
        || original["locked"] == true
        || original["fontFamily"] != 9
        || number(original, "angle")? != 0.
        || !original["containerId"].is_null()
        || elements.iter().any(|e| {
            e["isDeleted"] != true
                && (e["startBinding"]["elementId"] == id || e["endBinding"]["elementId"] == id)
        })
        || match &original["boundElements"] {
            Value::Null => false,
            Value::Array(items) => !items.is_empty(),
            _ => true,
        }
    {
        return Err(fail(
            "fit_text_width requires live unlocked unrotated unbound scientific text without attached connectors",
        ));
    }
    let text = original["text"]
        .as_str()
        .ok_or_else(|| fail("text must be a string"))?;
    let size = number(original, "fontSize")?;
    let line_height = number(original, "lineHeight")?;
    if text.trim().is_empty()
        || text.chars().count() > 2048
        || !(4. ..=200.).contains(&size)
        || !(0.5..=3.).contains(&line_height)
        || text.chars().any(|c| {
            !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
                && crate::font_metrics::ADVANCES
                    .binary_search_by_key(&(c as u32), |(cp, _)| *cp)
                    .is_err()
        })
    {
        return Err(fail(
            "fit_text_width requires supported scientific text and valid typography",
        ));
    }
    let width = number(original, "width")?;
    let height = number(original, "height")?;
    let y = number(original, "y")?;
    if !(1. ..=20000.).contains(&width) || height <= 0. {
        return Err(fail("invalid text box dimensions"));
    }
    let measured = layout::text_width(text, size);
    if measured > width + 1e-6 {
        return Err(fail(
            "visible text exceeds its current width; explicitly reflow it first",
        ));
    }
    let next_width = measured.ceil().max(1.).min(width);
    let alignment = match &original["textAlign"] {
        Value::Null => "left",
        Value::String(value) if matches!(value.as_str(), "left" | "center" | "right") => value,
        _ => return Err(fail("unsupported text alignment")),
    };
    let x = number(original, "x")? + align(alignment, width, next_width)?;
    if !x.is_finite() || x.abs() > 1e6 || x + next_width > 1e6 {
        return Err(fail("fitted text geometry is out of range"));
    }
    if let Some(frame_id) = original["frameId"].as_str() {
        let frame = elements
            .iter()
            .find(|e| {
                e["id"] == frame_id
                    && e["type"] == "frame"
                    && e["isDeleted"] != true
                    && e["locked"] != true
            })
            .ok_or_else(|| fail("text frame is missing or locked"))?;
        let (fx, fy, fw, fh) = (
            number(frame, "x")?,
            number(frame, "y")?,
            number(frame, "width")?,
            number(frame, "height")?,
        );
        if number(frame, "angle")? != 0.
            || x < fx
            || y < fy
            || x + next_width > fx + fw
            || y + height > fy + fh
        {
            return Err(fail("fitted text would leave its existing frame"));
        }
    } else if !original["frameId"].is_null() {
        return Err(fail("invalid frame identity"));
    }
    if next_width == width {
        return Ok(());
    }
    let moved = x != number(original, "x")?;
    let target = &mut elements[index];
    target["width"] = json!(next_width);
    if moved {
        target["x"] = json!(x);
    }
    scene::touch(target);
    Ok(())
}

/// Explicit label-only movement; grouping does not expand the edit target.
pub(crate) fn set_position(
    elements: &mut [Value],
    id: &str,
    x: f64,
    y: f64,
) -> Result<(), SceneError> {
    if ![x, y]
        .iter()
        .all(|value| value.is_finite() && value.abs() <= 1e6)
    {
        return Err(fail("text position must be finite and within scene limits"));
    }
    let index = elements
        .iter()
        .position(|element| element["id"] == id)
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let text = &elements[index];
    if text["type"] != "text"
        || text["isDeleted"] == true
        || text["locked"] == true
        || !text["containerId"].is_null()
        || (!text["angle"].is_null() && number(text, "angle")? != 0.)
    {
        return Err(fail(
            "requires live unlocked unbound unrotated text; bound labels move with their container",
        ));
    }
    if text["boundElements"]
        .as_array()
        .is_some_and(|entries| entries.iter().any(|entry| entry["type"] == "arrow"))
        || elements.iter().any(|element| {
            element["isDeleted"] != true
                && element["type"] == "arrow"
                && (element["startBinding"]["elementId"] == id
                    || element["endBinding"]["elementId"] == id)
        })
    {
        return Err(fail(
            "text with attached arrows requires native connector editing",
        ));
    }
    let width = number(text, "width")?;
    let height = number(text, "height")?;
    if width <= 0. || height <= 0. {
        return Err(fail("text dimensions must be positive"));
    }
    if let Some(frame_id) = text["frameId"].as_str() {
        let frame = elements
            .iter()
            .find(|element| element["id"] == frame_id && element["isDeleted"] != true)
            .ok_or_else(|| fail("text frame is missing"))?;
        if frame["locked"] == true
            || !matches!(frame["type"].as_str(), Some("frame" | "magicframe"))
            || (!frame["angle"].is_null() && number(frame, "angle")? != 0.)
        {
            return Err(fail("requires a live unlocked unrotated text frame"));
        }
        let fx = number(frame, "x")?;
        let fy = number(frame, "y")?;
        let fw = number(frame, "width")?;
        let fh = number(frame, "height")?;
        if fw <= 0.
            || fh <= 0.
            || x < fx - 1e-6
            || y < fy - 1e-6
            || x + width > fx + fw + 1e-6
            || y + height > fy + fh + 1e-6
        {
            return Err(fail("text movement must stay inside its current frame"));
        }
    } else if !text["frameId"].is_null() {
        return Err(fail("text frame identity is invalid"));
    }
    if elements[index]["x"].as_f64() != Some(x) {
        elements[index]["x"] = json!(x);
    }
    if elements[index]["y"].as_f64() != Some(y) {
        elements[index]["y"] = json!(y);
    }
    scene::touch(&mut elements[index]);
    Ok(())
}
