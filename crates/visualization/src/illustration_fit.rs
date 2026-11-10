//! Explicit visual bounds for an illustration's semantic anchor and local connectors.
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

// Native arrow labels use the middle vertex/segment. Balance harmless collinear
// vertices around a retained label center, rather than moving clear existing text.
fn preserve_center(route: &mut Vec<[f64; 2]>, center: [f64; 2]) {
    let index = if let Some(i) = route.iter().position(|p| close(*p, center)) {
        i
    } else {
        let Some(i) = route.windows(2).position(|p| {
            let [a, b] = [p[0], p[1]];
            ((a[0] - b[0]).abs() < 1e-6
                && (center[0] - a[0]).abs() < 1e-6
                && center[1] >= a[1].min(b[1]) - 1e-6
                && center[1] <= a[1].max(b[1]) + 1e-6)
                || ((a[1] - b[1]).abs() < 1e-6
                    && (center[1] - a[1]).abs() < 1e-6
                    && center[0] >= a[0].min(b[0]) - 1e-6
                    && center[0] <= a[0].max(b[0]) + 1e-6)
        }) else {
            return;
        };
        route.insert(i + 1, center);
        i + 1
    };
    if index == 0 || index + 1 == route.len() {
        return;
    }
    let left = index;
    let right = route.len() - index - 1;
    let extra = left.abs_diff(right);
    if extra == 0 {
        return;
    }
    let segment = if left < right { 0 } else { route.len() - 2 };
    let a = route[segment];
    let b = route[segment + 1];
    for n in 1..=extra {
        let t = n as f64 / (extra + 1) as f64;
        route.insert(
            segment + n,
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
        );
    }
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
    editable(visual)?;
    if visual["customData"]["semanticAnchorId"] != anchor_id
        || visual["fileId"] != file_id
        || !visual["crop"].is_null()
        || !visual["scale"]
            .as_array()
            .is_some_and(|scale| scale.len() == 2 && scale.iter().all(|n| n.as_f64() == Some(1.)))
        || visual["frameId"] != anchor["frameId"]
    {
        return Err(fail(
            "requires the exact uncropped unflipped owned image file and matching frame",
        ));
    }
    let old = rect(anchor)?;
    let slot = layout_slot(anchor)?;
    let [ix, iy, iw, ih] = rect(visual)?;
    let fitted = [ix + u * iw, iy + v * ih, w * iw, h * ih];
    if fitted[2] < 1. || fitted[3] < 1. || fitted.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
        return Err(fail(
            "fitted bounds must be at least one scene unit and remain in range",
        ));
    }
    frame(elements, anchor, fitted)?;
    let anchor_id = anchor_id.to_owned();
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
            .position(|port| close(*port, endpoint))
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
    anchor["customData"]["visualAnchorFit"] =
        json!({"fileId":file_id,"normalizedBounds":normalized,"imageBounds":[ix,iy,iw,ih]});
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
