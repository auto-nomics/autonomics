//! Explicit visual bounds for an illustration's semantic anchor and local connectors.
use crate::connector_path::preserve_center;
use crate::scene::{self, SceneError};
use serde_json::{Value, json};
fn fail(message: &str) -> SceneError {
    SceneError::InvalidIllustration(message.into())
}
fn number(e: &Value, key: &str) -> Result<f64, SceneError> {
    e[key]
        .as_f64()
        .filter(|v| v.is_finite() && v.abs() <= 1e6)
        .ok_or_else(|| fail("invalid component geometry"))
}
fn editable(e: &Value) -> Result<(), SceneError> {
    if e["isDeleted"] == true || e["locked"] == true || number(e, "angle")? != 0. {
        return Err(fail("requires live unlocked unrotated objects"));
    }
    Ok(())
}
fn rect(e: &Value) -> Result<[f64; 4], SceneError> {
    let r = [
        number(e, "x")?,
        number(e, "y")?,
        number(e, "width")?,
        number(e, "height")?,
    ];
    if r[2] <= 0. || r[3] <= 0. {
        return Err(fail("requires positive object bounds"));
    }
    Ok(r)
}
/// Layout slots follow anchor translation/scaling without becoming stale absolute boxes.
pub(crate) fn layout_slot(anchor: &Value) -> Result<[f64; 4], SceneError> {
    let r = rect(anchor)?;
    let Some(slot) = anchor["customData"].get("visualAssetSlot") else {
        return Ok(r);
    };
    if anchor["angle"].as_f64().unwrap_or(0.) != 0. {
        return Err(fail("rotated fitted components require native editing"));
    }
    let Some(slot) = slot.as_array().filter(|s| s.len() == 4) else {
        return Err(fail("invalid visual layout slot"));
    };
    let Some(n) = slot.iter().map(Value::as_f64).collect::<Option<Vec<_>>>() else {
        return Err(fail("invalid visual layout slot"));
    };
    if !n.iter().all(|n| n.is_finite()) || n[2] <= 0. || n[3] <= 0. {
        return Err(fail("invalid visual layout slot"));
    }
    let slot = [
        r[0] + n[0] * r[2],
        r[1] + n[1] * r[3],
        n[2] * r[2],
        n[3] * r[3],
    ];
    if slot.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
        return Err(fail("visual layout slot is out of range"));
    }
    Ok(slot)
}

/// Map source-root fractions through the saved crop, flips and center rotation.
/// The semantic anchor remains an axis-aligned box around the selected artwork.
pub(crate) fn artwork_bounds(visual: &Value, bounds: [f64; 4]) -> Result<[f64; 4], SceneError> {
    let [x, y, width, height] = rect(visual)?;
    let angle = number(visual, "angle")?;
    let scale = visual["scale"]
        .as_array()
        .filter(|s| s.len() == 2)
        .ok_or_else(|| fail("image scale must contain two flips"))?;
    let flips = [scale[0].as_f64(), scale[1].as_f64()];
    if flips.iter().any(|v| !matches!(v, Some(1.0) | Some(-1.0))) {
        return Err(fail("image scale must contain only 1 or -1"));
    }
    let crop = if visual["crop"].is_null() {
        [0., 0., 1., 1.]
    } else {
        let c = &visual["crop"];
        let nw = number(c, "naturalWidth")?;
        let nh = number(c, "naturalHeight")?;
        if nw <= 0. || nh <= 0. {
            return Err(fail("invalid crop source dimensions"));
        }
        [
            number(c, "x")? / nw,
            number(c, "y")? / nh,
            number(c, "width")? / nw,
            number(c, "height")? / nh,
        ]
    };
    if crop[0] < 0.
        || crop[1] < 0.
        || crop[2] <= 0.
        || crop[3] <= 0.
        || crop[0] + crop[2] > 1. + 1e-9
        || crop[1] + crop[3] > 1. + 1e-9
    {
        return Err(fail("saved crop is outside the source image"));
    }
    if bounds[0] < crop[0] - 1e-9
        || bounds[1] < crop[1] - 1e-9
        || bounds[0] + bounds[2] > crop[0] + crop[2] + 1e-9
        || bounds[1] + bounds[3] > crop[1] + crop[3] + 1e-9
    {
        return Err(fail(
            "selected artwork bounds must lie inside the visible source crop",
        ));
    }
    let (sin, cos) = angle.sin_cos();
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for u in [bounds[0], bounds[0] + bounds[2]] {
        for v in [bounds[1], bounds[1] + bounds[3]] {
            let local = [
                ((u - crop[0]) / crop[2] - 0.5) * width * flips[0].unwrap(),
                ((v - crop[1]) / crop[3] - 0.5) * height * flips[1].unwrap(),
            ];
            let point = [
                x + width / 2. + cos * local[0] - sin * local[1],
                y + height / 2. + sin * local[0] + cos * local[1],
            ];
            for axis in 0..2 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
    }
    Ok([low[0], low[1], high[0] - low[0], high[1] - low[1]])
}

fn frame(elements: &[Value], e: &Value, bounds: [f64; 4]) -> Result<(), SceneError> {
    if let Some(id) = e["frameId"].as_str() {
        let f = elements
            .iter()
            .find(|f| f["id"] == id && f["type"] == "frame")
            .ok_or_else(|| fail("missing component frame"))?;
        editable(f)?;
        let [x, y, w, h] = rect(f)?;
        if bounds[0] < x
            || bounds[1] < y
            || bounds[0] + bounds[2] > x + w
            || bounds[1] + bounds[3] > y + h
        {
            return Err(fail("adjustment would leave its frame"));
        }
    }
    Ok(())
}
fn ports([x, y, w, h]: [f64; 4], gap: f64) -> [[f64; 2]; 4] {
    [
        [x - gap, y + h / 2.],
        [x + w + gap, y + h / 2.],
        [x + w / 2., y - gap],
        [x + w / 2., y + h + gap],
    ]
}
fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}
fn bridge(port: [f64; 2], next: [f64; 2], side: usize) -> Vec<[f64; 2]> {
    let candidates = if side < 2 {
        let x = (port[0] + next[0]) / 2.;
        vec![port, [x, port[1]], [x, next[1]], next]
    } else {
        let y = (port[1] + next[1]) / 2.;
        vec![port, [port[0], y], [next[0], y], next]
    };
    let mut route = Vec::new();
    for p in candidates {
        if route.last().is_none_or(|last| !close(*last, p)) {
            route.push(p);
        }
    }
    route
}

/// Uniform center-fixed scaling reuses a current, explicitly reviewed fit.
pub(crate) fn scale(
    elements: &mut [Value],
    id: &str,
    file_id: &str,
    factor: f64,
) -> Result<(), SceneError> {
    if !factor.is_finite() || factor <= 0. {
        return Err(fail(
            "scale factor must be a finite positive number; 1.1 means enlarge by 10%",
        ));
    }
    let selected = elements
        .iter()
        .find(|e| e["id"] == id)
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let (anchor, image) = if selected["type"] == "image" {
        let owner = selected["customData"]["semanticAnchorId"]
            .as_str()
            .ok_or_else(|| fail("requires an owned illustration"))?;
        (
            elements
                .iter()
                .find(|e| e["id"] == owner)
                .ok_or_else(|| fail("missing semantic anchor"))?,
            selected,
        )
    } else {
        let visual = selected["customData"]["visualAssetElementId"]
            .as_str()
            .ok_or_else(|| fail("anchor has no illustration"))?;
        (
            selected,
            elements
                .iter()
                .find(|e| e["id"] == visual && e["type"] == "image")
                .ok_or_else(|| fail("missing illustration"))?,
        )
    };
    let fit = &anchor["customData"]["visualAnchorFit"];
    if anchor["customData"]["visualAssetElementId"] != image["id"]
        || image["customData"]["semanticAnchorId"] != anchor["id"]
        || fit["fileId"] != file_id
        || image["fileId"] != file_id
    {
        return Err(fail(
            "requires the exact reciprocal illustration and its reviewed file fit",
        ));
    }
    if fit["scope"]=="illustration_group" {
        let anchor_id=anchor["id"].as_str().ok_or_else(||fail("missing group identity"))?.to_owned();
        return scale_group(elements,&anchor_id,factor);
    }
    let normalized: [f64; 4] = serde_json::from_value(fit["normalizedBounds"].clone())
        .map_err(|_| fail("review source bounds with fit_illustration_anchor before scaling"))?;
    let expected = artwork_bounds(image, normalized)?;
    if rect(anchor)?
        .iter()
        .zip(expected)
        .any(|(actual, expected)| (actual - expected).abs() > 1e-6)
    {
        return Err(fail(
            "stored illustration fit is stale; review and refit before scaling",
        ));
    }
    let [x, y, width, height] = rect(image)?;
    let w = width * factor;
    let h = height * factor;
    set_geometry(
        elements,
        id,
        file_id,
        [x + (width - w) / 2., y + (height - h) / 2., w, h],
        normalized,
    )
}

/// The outer scene edit is atomic: reject unsupported connectors after geometry validation.
pub(crate) fn set_geometry(
    elements: &mut [Value],
    id: &str,
    file_id: &str,
    bounds: [f64; 4],
    normalized: [f64; 4],
) -> Result<(), SceneError> {
    if bounds.iter().any(|n| !n.is_finite() || n.abs() > 1e6) || bounds[2] <= 0. || bounds[3] <= 0.
    {
        return Err(fail(
            "illustration geometry requires finite x/y and positive width/height within 1000000",
        ));
    }
    let selected = elements
        .iter()
        .find(|e| e["id"] == id)
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let visual_id = if selected["type"] == "image" {
        id.to_owned()
    } else {
        selected["customData"]["visualAssetElementId"]
            .as_str()
            .ok_or_else(|| fail("anchor has no illustration"))?
            .to_owned()
    };
    let index = elements
        .iter()
        .position(|e| e["id"] == visual_id && e["type"] == "image")
        .ok_or_else(|| fail("missing illustration"))?;
    let visual = &elements[index];
    if visual["isDeleted"] == true
        || visual["locked"] == true
        || visual["fileId"] != file_id
        || !visual["customData"]["semanticAnchorId"].is_string()
        || visual["customData"]["inset"].is_object()
    {
        return Err(fail(
            "requires the exact live unlocked owned illustration, excluding insets",
        ));
    }
    if elements.iter().any(|e| {
        e["isDeleted"] != true
            && (e["containerId"] == visual_id
                || e["startBinding"]["elementId"] == visual_id
                || e["endBinding"]["elementId"] == visual_id)
    }) {
        return Err(fail(
            "illustration has directly bound objects; use native editing",
        ));
    }
    let mut next = visual.clone();
    for (key, value) in ["x", "y", "width", "height"].into_iter().zip(bounds) {
        next[key] = json!(value);
    }
    // Check the complete displayed image, including rotation, against its frame.
    let crop = &next["crop"];
    let visible = if crop.is_null() {
        [0., 0., 1., 1.]
    } else {
        let nw = number(crop, "naturalWidth")?;
        let nh = number(crop, "naturalHeight")?;
        if nw <= 0. || nh <= 0. {
            return Err(fail("invalid crop source dimensions"));
        }
        [
            number(crop, "x")? / nw,
            number(crop, "y")? / nh,
            number(crop, "width")? / nw,
            number(crop, "height")? / nh,
        ]
    };
    frame(elements, &next, artwork_bounds(&next, visible)?)?;
    scene::touch(&mut next);
    elements[index] = next;
    apply(elements, id, file_id, normalized)
}

pub(crate) fn apply(
    elements: &mut [Value],
    id: &str,
    file_id: &str,
    normalized: [f64; 4],
) -> Result<(), SceneError> {
    let [u, v, w, h] = normalized;
    if !normalized.iter().all(|v| v.is_finite())
        || u < 0.
        || v < 0.
        || w <= 0.
        || h <= 0.
        || u + w > 1. + 1e-9
        || v + h > 1. + 1e-9
    {
        return Err(fail(
            "normalized_bounds must be a nonempty [x,y,width,height] inside [0,1]",
        ));
    }
    let selected = elements
        .iter()
        .find(|e| e["id"] == id)
        .ok_or_else(|| SceneError::NotFound(id.into()))?;
    let anchor_id = if selected["type"] == "image" {
        selected["customData"]["semanticAnchorId"]
            .as_str()
            .ok_or_else(|| fail("image has no semantic anchor"))?
    } else {
        id
    };
    let index = elements
        .iter()
        .position(|e| e["id"] == anchor_id)
        .ok_or_else(|| fail("missing semantic anchor"))?;
    let anchor = &elements[index];
    editable(anchor)?;
    if anchor["type"] != "rectangle"
        || anchor["customData"]["inset"].is_object()
        || anchor["customData"]["annotationKind"].is_string()
    {
        return Err(fail("requires an illustrated rectangular semantic anchor"));
    }
    let visual_id = anchor["customData"]["visualAssetElementId"]
        .as_str()
        .ok_or_else(|| fail("anchor has no illustration"))?;
    let visual = elements
        .iter()
        .find(|e| e["id"] == visual_id && e["type"] == "image")
        .ok_or_else(|| fail("missing illustration"))?;
    if visual["isDeleted"] == true || visual["locked"] == true {
        return Err(fail("requires a live unlocked illustration"));
    }
    if visual["customData"]["semanticAnchorId"] != anchor_id
        || visual["fileId"] != file_id
        || visual["frameId"] != anchor["frameId"]
    {
        return Err(fail(
            "requires the exact owned image file and matching frame",
        ));
    }
    let old = rect(anchor)?;
    let slot = layout_slot(anchor)?;
    let [ix, iy, iw, ih] = rect(visual)?;
    let fitted = artwork_bounds(visual, normalized)?;
    let image_transform =
        json!({"angle":visual["angle"],"scale":visual["scale"],"crop":visual["crop"]});
    if fitted[2] < 1. || fitted[3] < 1. || fitted.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
        return Err(fail(
            "fitted bounds must be at least one scene unit and remain in range",
        ));
    }
    frame(elements, anchor, fitted)?;
    let anchor_id = anchor_id.to_owned();
    adjust_connected(elements, &anchor_id, old, fitted)?;
    let anchor = &mut elements[index];
    for (key, value) in ["x", "y", "width", "height"].into_iter().zip(fitted) {
        anchor[key] = json!(value);
    }
    anchor["customData"]["visualAssetSlot"] = json!([
        (slot[0] - fitted[0]) / fitted[2],
        (slot[1] - fitted[1]) / fitted[3],
        slot[2] / fitted[2],
        slot[3] / fitted[3]
    ]);
    anchor["customData"]["visualAnchorFit"] = json!({"fileId":file_id,"normalizedBounds":normalized,"imageBounds":[ix,iy,iw,ih],"imageTransform":image_transform});
    scene::touch(anchor);
    Ok(())
}
fn rect_line(e: &Value) -> Result<[f64; 4], SceneError> {
    Ok([
        number(e, "x")?,
        number(e, "y")?,
        number(e, "width")?,
        number(e, "height")?,
    ])
}

/// Rebind a native component's centered sharp connectors to new bounds.
/// Callers operate on a scene clone, so unsupported geometry remains atomic.
pub(crate) fn adjust_connected(
    elements: &mut [Value],
    anchor_id: &str,
    old: [f64; 4],
    fitted: [f64; 4],
) -> Result<(), SceneError> {
    let connected = elements
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e["isDeleted"] != true
                && (e["startBinding"]["elementId"] == anchor_id
                    || e["endBinding"]["elementId"] == anchor_id)
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if connected.len() > 200 {
        return Err(fail("at most 200 connected arrows per adjustment"));
    }
    // The caller edits a clone; any unsupported connector rejects the entire batch.
    for i in connected {
        let arrow = &elements[i];
        editable(arrow)?;
        if arrow["customData"]["annotationKind"].is_string()
            || arrow["customData"]["inset"].is_object()
            || arrow["type"] != "arrow"
            || arrow["elbowed"] == true
            || !arrow["roundness"].is_null()
            || number(arrow, "roughness")? != 0.
        {
            return Err(fail("requires straight or sharp orthogonal native arrows"));
        }
        let start = arrow["startBinding"]["elementId"] == anchor_id;
        let end = arrow["endBinding"]["elementId"] == anchor_id;
        if start && end {
            return Err(fail("self-loop geometry requires native editing"));
        }
        let binding = &arrow[if start { "startBinding" } else { "endBinding" }];
        let gap = number(binding, "gap")?;
        if !(0. ..=100.).contains(&gap)
            || number(binding, "focus")? != 0.
            || !binding["fixedPoint"].is_null()
        {
            return Err(fail(
                "requires centered bindings with a 0..100 gap and no fixedPoint",
            ));
        }
        let raw = arrow["points"]
            .as_array()
            .filter(|p| (2..=1000).contains(&p.len()))
            .ok_or_else(|| fail("invalid connector points"))?;
        let [ax, ay, _, _] = rect_line(arrow)?;
        let mut points = raw
            .iter()
            .map(|p| {
                let p = p
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .ok_or_else(|| fail("invalid connector point"))?;
                let x = p[0]
                    .as_f64()
                    .filter(|x| x.is_finite() && x.abs() <= 1e6)
                    .ok_or_else(|| fail("invalid connector point"))?;
                let y = p[1]
                    .as_f64()
                    .filter(|y| y.is_finite() && y.abs() <= 1e6)
                    .ok_or_else(|| fail("invalid connector point"))?;
                Ok([x + ax, y + ay])
            })
            .collect::<Result<Vec<_>, SceneError>>()?;
        if points
            .windows(2)
            .any(|p| (p[0][0] - p[1][0]).abs() > 1e-6 && (p[0][1] - p[1][1]).abs() > 1e-6)
        {
            return Err(fail("diagonal connectors require native editing"));
        }
        let endpoint = if start {
            points[0]
        } else {
            *points.last().unwrap()
        };
        let side = ports(old, gap)
            .iter()
            .position(|port| {
                // Saved/native coordinates may be rounded to hundredths. Accept
                // only sub-hundredth drift at the existing centered port; the
                // new endpoint still uses the exact fitted port and same binding.
                (port[0] - endpoint[0]).abs() <= 0.01
                    && (port[1] - endpoint[1]).abs() <= 0.01
            })
            .ok_or_else(|| fail("connector endpoint does not match the existing anchor port"))?;
        let port = ports(fitted, gap)[side];
        if close(port, endpoint) {
            continue;
        }
        let before = scene::path_label_center(&points, 0., 0.);
        let axis = if side < 2 { 0 } else { 1 };
        let sign = if side == 0 || side == 2 { -1. } else { 1. };
        // A later replacement/refit may expand the anchor again. Discard only
        // obsolete near-end stubs; preserve the far endpoint and retained trunk.
        loop {
            let neighbor = if start {
                points[1]
            } else {
                points[points.len() - 2]
            };
            if (neighbor[axis] - port[axis]) * sign >= -1e-6 {
                break;
            }
            if points.len() == 2 {
                return Err(fail(
                    "fitted port would pass the far connector endpoint; use native editing",
                ));
            }
            if start {
                points.remove(1);
            } else {
                let n = points.len();
                points.remove(n - 2);
            }
        }
        let mut next = if start {
            let mut p = bridge(port, points[1], side);
            p.extend_from_slice(&points[2..]);
            p
        } else {
            let n = points.len();
            let mut tail = bridge(port, points[n - 2], side);
            tail.reverse();
            let mut p = points[..n - 2].to_vec();
            p.extend(tail);
            p
        };
        next.dedup_by(|a, b| close(*a, *b));
        if next.len() < 2
            || next
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || v.abs() > 1e6)
        {
            return Err(fail("invalid adjusted connector geometry"));
        }
        if next == points {
            continue;
        }
        let arrow_id = arrow["id"]
            .as_str()
            .ok_or_else(|| fail("missing connector identity"))?
            .to_owned();
        let labels = elements
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e["type"] == "text" && e["isDeleted"] != true && e["containerId"] == arrow_id
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if labels.len() > 1 {
            return Err(fail("ambiguous connector label ownership"));
        }
        if !labels.is_empty() {
            preserve_center(&mut next, before);
        }
        let after = scene::path_label_center(&next, 0., 0.);
        for label in labels {
            editable(&elements[label])?;
            let mut r = rect(&elements[label])?;
            r[0] += after[0] - before[0];
            r[1] += after[1] - before[1];
            frame(elements, &elements[label], r)?;
            if close(before, after) {
                continue;
            }
            elements[label]["x"] = json!(r[0]);
            elements[label]["y"] = json!(r[1]);
            scene::touch(&mut elements[label]);
        }
        let origin = next[0];
        let arrow = &mut elements[i];
        arrow["x"] = json!(origin[0]);
        arrow["y"] = json!(origin[1]);
        arrow["width"] = json!(
            next.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max)
                - next.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min)
        );
        arrow["height"] = json!(
            next.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max)
                - next.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)
        );
        arrow["points"] = json!(
            next.iter()
                .map(|p| [p[0] - origin[0], p[1] - origin[1]])
                .collect::<Vec<_>>()
        );
        scene::touch(arrow);
    }
    Ok(())
}

fn scale_group(elements: &mut [Value], anchor_id: &str, factor: f64) -> Result<(), SceneError> {
    let anchor = elements
        .iter()
        .find(|e| e["id"] == anchor_id)
        .ok_or_else(|| fail("missing group anchor"))?;
    editable(anchor)?;
    let old = rect(anchor)?;
    let fit = &anchor["customData"]["visualAnchorFit"];
    let recorded: [f64; 4] = serde_json::from_value(fit["anchorBounds"].clone())
        .map_err(|_| fail("invalid group bounds snapshot"))?;
    if old
        .iter()
        .zip(recorded)
        .any(|(a, b)| !b.is_finite() || (a - b).abs() > 1e-6)
    {
        return Err(fail("group fit is stale; review and refit before scaling"));
    }
    let members = fit["members"]
        .as_array()
        .filter(|m| !m.is_empty() && m.len() <= 33)
        .ok_or_else(|| fail("missing complete group fit"))?;
    let mut samples = Vec::new();
    let mut changes = Vec::new();
    let mut union = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let center = [old[0] + old[2] / 2., old[1] + old[3] / 2.];
    for member in members {
        let id = member["elementId"]
            .as_str()
            .ok_or_else(|| fail("invalid group member identity"))?;
        let matching = elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e["id"] == id && e["isDeleted"] != true)
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(fail("missing or ambiguous group image"));
        }
        let (i, image) = matching[0];
        editable(image)?;
        let bounds = rect(image)?;
        let recorded: [f64; 4] = serde_json::from_value(member["imageBounds"].clone())
            .map_err(|_| fail("invalid member viewport snapshot"))?;
        let file = member["fileId"]
            .as_str()
            .ok_or_else(|| fail("missing member file"))?;
        if image["fileId"] != file
            || bounds
                .iter()
                .zip(recorded)
                .any(|(a, b)| !b.is_finite() || (a - b).abs() > 1e-6)
            || member["imageTransform"]["angle"] != image["angle"]
            || member["imageTransform"]["scale"] != image["scale"]
            || member["imageTransform"]["crop"] != image["crop"]
        {
            return Err(fail(
                "group member fit is stale; review and refit before scaling",
            ));
        }
        let normalized: [f64; 4] = serde_json::from_value(member["normalizedBounds"].clone())
            .map_err(|_| fail("invalid sampled member bounds"))?;
        let b = artwork_bounds(image, normalized)?;
        union[0] = union[0].min(b[0]);
        union[1] = union[1].min(b[1]);
        union[2] = union[2].max(b[0] + b[2]);
        union[3] = union[3].max(b[1] + b[3]);
        samples.push(crate::scene::IllustrationSample {
            id: id.into(),
            file_id: file.into(),
            normalized_bounds: normalized,
        });
        let next = [
            center[0] + (bounds[0] - center[0]) * factor,
            center[1] + (bounds[1] - center[1]) * factor,
            bounds[2] * factor,
            bounds[3] * factor,
        ];
        if next.iter().any(|v| !v.is_finite() || v.abs() > 1e6) || next[2] <= 0. || next[3] <= 0. {
            return Err(fail("scaled group geometry is outside canvas range"));
        }
        changes.push((i, next));
    }
    let union = [union[0], union[1], union[2] - union[0], union[3] - union[1]];
    if old
        .iter()
        .zip(union)
        .any(|(a, b)| !b.is_finite() || (a - b).abs() > 1e-6)
    {
        return Err(fail(
            "sampled group footprint is stale; refit before scaling",
        ));
    }
    for (i, bounds) in changes {
        let image = &mut elements[i];
        for (k, v) in ["x", "y", "width", "height"].into_iter().zip(bounds) {
            image[k] = json!(v);
        }
        scene::touch(image);
    }
    apply_group(elements, anchor_id, &samples)
}

pub(crate) fn translate_group_record(record: &mut Value, dx: f64, dy: f64) {
    fn shift(value: &mut Value, dx: f64, dy: f64) {
        if let Some(b) = value
            .as_array_mut()
            .filter(|b| b.len() == 4 && b.iter().all(|v| v.as_f64().is_some_and(f64::is_finite)))
        {
            b[0] = json!(b[0].as_f64().unwrap() + dx);
            b[1] = json!(b[1].as_f64().unwrap() + dy);
        }
    }
    if record["scope"] != "illustration_group" {
        return;
    }
    shift(&mut record["anchorBounds"], dx, dy);
    if let Some(members) = record["members"].as_array_mut() {
        for m in members {
            shift(&mut m["imageBounds"], dx, dy);
        }
    }
}

/// Fit a component to the union of explicitly sampled primary and accessory images.
pub(crate) fn apply_group(
    elements: &mut [Value],
    anchor_id: &str,
    samples: &[crate::scene::IllustrationSample],
) -> Result<(), SceneError> {
    use std::collections::HashSet;
    let ai = elements
        .iter()
        .position(|e| e["id"] == anchor_id && e["isDeleted"] != true)
        .ok_or_else(|| fail("missing group anchor"))?;
    let anchor = &elements[ai];
    editable(anchor)?;
    if anchor["type"] != "rectangle"
        || !anchor["customData"]["inset"].is_null()
        || !anchor["customData"]["annotationKind"].is_null()
    {
        return Err(fail("requires an illustrated rectangular group anchor"));
    }
    let primary = anchor["customData"]["visualAssetElementId"]
        .as_str()
        .ok_or_else(|| fail("missing group primary"))?;
    let group = anchor["customData"]["visualAssetGroupId"]
        .as_str()
        .ok_or_else(|| fail("missing illustration group"))?;
    if group.is_empty()
        || !anchor["groupIds"]
            .as_array()
            .is_some_and(|ids| ids.contains(&json!(group)))
        || (!anchor["customData"]["visualBadgeIds"].is_null()
            && !anchor["customData"]["visualBadgeIds"].is_array())
    {
        return Err(fail("invalid group ownership record"));
    }
    let mut expected = HashSet::from([primary]);
    if let Some(ids) = anchor["customData"]["visualBadgeIds"].as_array() {
        for id in ids {
            if !expected.insert(
                id.as_str()
                    .ok_or_else(|| fail("invalid accessory identity"))?,
            ) {
                return Err(fail("duplicate group image"));
            }
        }
    }
    if expected.len() > 33 || samples.len() != expected.len() {
        return Err(fail(
            "samples must cover the complete primary/accessory group (at most 33 images)",
        ));
    }
    let mut seen = HashSet::new();
    let mut records = Vec::new();
    let mut union = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for sample in samples {
        if !expected.contains(sample.id.as_str()) || !seen.insert(sample.id.as_str()) {
            return Err(fail(
                "sample identities must exactly match group membership",
            ));
        }
        let matches = elements
            .iter()
            .filter(|e| e["id"] == sample.id && e["isDeleted"] != true)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(fail("ambiguous group image"));
        }
        let image = matches[0];
        editable(image)?;
        let data = &image["customData"];
        let owned = if sample.id == primary {
            data["semanticAnchorId"] == anchor_id
        } else {
            data["semanticBadgeAnchorId"] == anchor_id
                && data["visualBadgeGroupId"] == group
                && data["badgeMeaning"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
        };
        if image["type"] != "image"
            || !owned
            || image["fileId"] != sample.file_id
            || image["frameId"] != anchor["frameId"]
            || !image["groupIds"]
                .as_array()
                .is_some_and(|ids| ids.contains(&json!(group)))
            || image["opacity"].as_f64().unwrap_or(100.) != 100.
        {
            return Err(fail(
                "requires exact current, opaque, reciprocally owned group images in the same frame",
            ));
        }
        let [x, y, w, h] = sample.normalized_bounds;
        if !sample.normalized_bounds.iter().all(|v| v.is_finite())
            || x < 0.
            || y < 0.
            || w <= 0.
            || h <= 0.
            || x + w > 1. + 1e-9
            || y + h > 1. + 1e-9
        {
            return Err(fail(
                "each normalized sample must be nonempty and inside [0,1]",
            ));
        }
        let b = artwork_bounds(image, sample.normalized_bounds)?;
        union[0] = union[0].min(b[0]);
        union[1] = union[1].min(b[1]);
        union[2] = union[2].max(b[0] + b[2]);
        union[3] = union[3].max(b[1] + b[3]);
        records.push(json!({"elementId":sample.id,"fileId":sample.file_id,"normalizedBounds":sample.normalized_bounds,"imageBounds":rect(image)?,"imageTransform":{"angle":image["angle"],"scale":image["scale"],"crop":image["crop"]}}));
    }
    if elements.iter().any(|e| {
        e["isDeleted"] != true
            && e["type"] == "image"
            && (e["customData"]["semanticAnchorId"] == anchor_id
                || e["customData"]["semanticBadgeAnchorId"] == anchor_id)
            && !expected.contains(e["id"].as_str().unwrap_or(""))
    }) {
        return Err(fail("unlisted image claims group ownership"));
    }
    let fitted = [union[0], union[1], union[2] - union[0], union[3] - union[1]];
    if fitted[2] < 1. || fitted[3] < 1. || fitted.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
        return Err(fail("invalid combined visible bounds"));
    }
    let old = rect(anchor)?;
    let slot = layout_slot(anchor)?;
    frame(elements, anchor, fitted)?;
    let mut record = records
        .iter()
        .find(|r| r["elementId"] == primary)
        .unwrap()
        .clone();
    record["scope"] = json!("illustration_group");
    record["anchorBounds"] = json!(fitted);
    record["members"] = json!(records);
    adjust_connected(elements, anchor_id, old, fitted)?;
    let anchor = &mut elements[ai];
    for (k, v) in ["x", "y", "width", "height"].into_iter().zip(fitted) {
        anchor[k] = json!(v);
    }
    anchor["customData"]["visualAssetSlot"] = json!([
        (slot[0] - fitted[0]) / fitted[2],
        (slot[1] - fitted[1]) / fitted[3],
        slot[2] / fitted[2],
        slot[3] / fitted[3]
    ]);
    anchor["customData"]["visualAnchorFit"] = record;
    scene::touch(anchor);
    Ok(())
}

#[cfg(test)]
mod label_center_arrowheads {
    use super::*;
    fn length(points: &[[f64; 2]]) -> f64 {
        points
            .windows(2)
            .map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]))
            .sum()
    }
    #[test]
    fn balancing_only_terminal_leg_retains_most_of_its_length_and_does_not_erase_backtracking() {
        for (mut route, center, minimum) in [
            (
                vec![
                    [0., 0.],
                    [0., 100.],
                    [100., 100.],
                    [100., 0.],
                    [200., 0.],
                    [226., 0.],
                ],
                [200., 0.],
                23.4,
            ),
            (
                vec![
                    [0., 0.],
                    [20., 0.],
                    [10., 0.],
                    [10., 20.],
                    [100., 20.],
                    [100., 50.],
                ],
                [10., 20.],
                30.,
            ),
        ] {
            let total = length(&route);
            let first = route[0];
            let last = *route.last().unwrap();
            preserve_center(&mut route, center);
            let n = route.len();
            assert!(
                (route[n - 1][0] - route[n - 2][0]).hypot(route[n - 1][1] - route[n - 2][1])
                    >= minimum - 1e-6
            );
            assert_eq!(route[0], first);
            assert_eq!(*route.last().unwrap(), last);
            assert!((length(&route) - total).abs() < 1e-6);
            assert!(close(scene::path_label_center(&route, 0., 0.), center));
        }
    }
    #[test]
    fn preserving_caption_center_does_not_shrink_terminal_arrowhead_segments() {
        let mut route = vec![
            [0., 0.],
            [20., 0.],
            [20., 0.5],
            [40., 0.5],
            [40., 100.],
            [40., 200.],
            [-500., 200.],
            [-500., 226.],
        ];
        let original = route.clone();
        let total = length(&route);
        let center = [40., 100.];
        for _ in 0..8 {
            let a = route[0];
            let b = route[1];
            route.insert(1, [a[0] + (b[0] - a[0]) / 3., a[1] + (b[1] - a[1]) / 3.]);
            route.insert(
                2,
                [
                    a[0] + 2. * (b[0] - a[0]) / 3.,
                    a[1] + 2. * (b[1] - a[1]) / 3.,
                ],
            );
            preserve_center(&mut route, center);
            let n = route.len();
            let terminal =
                (route[n - 1][0] - route[n - 2][0]).hypot(route[n - 1][1] - route[n - 2][1]);
            assert!(
                terminal >= 24.,
                "terminal segment shrank to {terminal}; Excalidraw limits arrowhead size to half this segment"
            );
            assert_eq!(route.first(), original.first());
            assert_eq!(route.last(), original.last());
            assert!(close(scene::path_label_center(&route, 0., 0.), center));
            assert!((length(&route) - total).abs() < 1e-6);
        }
    }
}
