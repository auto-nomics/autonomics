//! Preserve native connector label centers without shrinking arrowheads.
fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}

// Native arrow labels use the middle vertex/segment. Balance harmless collinear
// vertices around a retained label center, rather than moving clear existing text.
fn insert_center(route: &mut Vec<[f64; 2]>, center: [f64; 2]) -> Option<usize> {
    let index = if let Some(i) = route.iter().position(|p| close(*p, center)) {
        i
    } else {
        let i = route.windows(2).position(|p| {
            let [a, b] = [p[0], p[1]];
            ((a[0] - b[0]).abs() < 1e-6
                && (center[0] - a[0]).abs() < 1e-6
                && center[1] >= a[1].min(b[1]) - 1e-6
                && center[1] <= a[1].max(b[1]) + 1e-6)
                || ((a[1] - b[1]).abs() < 1e-6
                    && (center[1] - a[1]).abs() < 1e-6
                    && center[0] >= a[0].min(b[0]) - 1e-6
                    && center[0] <= a[0].max(b[0]) + 1e-6)
        })?;
        route.insert(i + 1, center);
        i + 1
    };
    Some(index)
}
pub(crate) fn preserve_center(route: &mut Vec<[f64; 2]>, center: [f64; 2]) {
    let Some(index) = insert_center(route, center) else {
        return;
    };
    if index == 0 || index + 1 == route.len() {
        return;
    }
    // Earlier balancing must not accumulate tiny terminal pieces across edits.
    // Remove only between-points on the same straight axis, retaining U-turns.
    let mut simple: Vec<[f64; 2]> = Vec::new();
    for &p in route.iter() {
        while simple.len() > 1 {
            let a = simple[simple.len() - 2];
            let b = simple[simple.len() - 1];
            let straight = (a[0] == b[0] && b[0] == p[0]) || (a[1] == b[1] && b[1] == p[1]);
            let between = (0..2)
                .all(|axis| b[axis] >= a[axis].min(p[axis]) && b[axis] <= a[axis].max(p[axis]));
            if straight && between {
                simple.pop();
            } else {
                break;
            }
        }
        simple.push(p);
    }
    *route = simple;
    let Some(index) = insert_center(route, center) else {
        return;
    };
    let left = index;
    let right = route.len() - index - 1;
    let extra = left.abs_diff(right);
    if extra == 0 {
        return;
    }
    let candidates = if left < right {
        0..index
    } else {
        index..route.len() - 1
    };
    let length = |i: usize| (route[i + 1][0] - route[i][0]).hypot(route[i + 1][1] - route[i][1]);
    // Native arrowhead size is capped by the adjacent terminal segment.
    // Prefer an interior segment so keeping a caption fixed cannot erase a head.
    let segment = candidates
        .clone()
        .filter(|&i| i != 0 && i + 2 != route.len())
        .max_by(|&a, &b| length(a).total_cmp(&length(b)))
        .or_else(|| candidates.max_by(|&a, &b| length(a).total_cmp(&length(b))))
        .unwrap();
    let distance = length(segment);
    let reserve = (distance * 0.9).min(50.);
    let (low, high) = if segment == 0 {
        (reserve / distance, 1.)
    } else if segment + 2 == route.len() {
        (0., 1. - reserve / distance)
    } else {
        (0., 1.)
    };
    let a = route[segment];
    let b = route[segment + 1];
    for n in 1..=extra {
        let t = low + (high - low) * n as f64 / (extra + 1) as f64;
        route.insert(
            segment + n,
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
        );
    }
}

/// Explicitly move a native caption along the unchanged rectilinear path.
/// Native Excalidraw labels use point indices, so rebalance collinear vertices.
pub(crate) fn position_caption(
    elements: &mut [serde_json::Value],
    id: &str,
    position: f64,
) -> Result<(), crate::scene::SceneError> {
    use crate::scene::{self, SceneError};
    use serde_json::json;
    let fail =
        |message: &str| SceneError::InvalidUpdate(format!("position_arrow_caption: {message}"));
    if !position.is_finite() || position <= 0. || position >= 1. {
        return Err(fail(
            "position must be a finite fraction strictly between 0 and 1, measured from the start binding along path length",
        ));
    }
    let ti = elements
        .iter()
        .position(|e| e["id"] == id && e["isDeleted"] != true)
        .ok_or_else(|| fail("caption is missing"))?;
    let text = &elements[ti];
    if text["type"] != "text"
        || text["locked"] == true
        || text["angle"].as_f64().unwrap_or(0.) != 0.
    {
        return Err(fail("requires an unlocked unrotated bound text caption"));
    }
    let arrow_id = text["containerId"]
        .as_str()
        .ok_or_else(|| fail("caption has no native connector binding"))?;
    let ai = elements
        .iter()
        .position(|e| e["id"] == arrow_id && e["isDeleted"] != true)
        .ok_or_else(|| fail("bound arrow is missing"))?;
    let arrow = &elements[ai];
    if arrow["type"] != "arrow"
        || arrow["locked"] == true
        || arrow["angle"].as_f64().unwrap_or(0.) != 0.
        || arrow["roughness"].as_f64() != Some(0.)
        || !arrow["roundness"].is_null()
        || arrow["elbowed"] == true
    {
        return Err(fail(
            "requires an unlocked sharp unrotated native arrow without roughness or elbow routing",
        ));
    }
    if elements
        .iter()
        .filter(|e| e["type"] == "text" && e["containerId"] == arrow_id && e["isDeleted"] != true)
        .count()
        != 1
        || !arrow["boundElements"].as_array().is_some_and(|v| {
            v.iter()
                .filter(|b| b["id"] == id && b["type"] == "text")
                .count()
                == 1
        })
    {
        return Err(fail("requires one reciprocal caption binding"));
    }
    let number = |e: &serde_json::Value, key: &str| {
        e[key]
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= 1e6)
            .ok_or_else(|| fail("invalid or out-of-range native geometry"))
    };
    let (x, y, w, h) = (
        number(arrow, "x")?,
        number(arrow, "y")?,
        number(text, "width")?,
        number(text, "height")?,
    );
    if w <= 0. || h <= 0. {
        return Err(fail("caption must have positive dimensions"));
    }
    let points = arrow["points"]
        .as_array()
        .filter(|p| (2..=512).contains(&p.len()))
        .ok_or_else(|| fail("requires 2..512 native points"))?;
    let mut route = points
        .iter()
        .map(|p| {
            let p = p
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or_else(|| fail("invalid point"))?;
            let a = p[0].as_f64().ok_or_else(|| fail("invalid point"))? + x;
            let b = p[1].as_f64().ok_or_else(|| fail("invalid point"))? + y;
            if ![a, b].iter().all(|v| v.is_finite() && v.abs() <= 1e6) {
                return Err(fail("invalid point"));
            }
            Ok([a, b])
        })
        .collect::<Result<Vec<_>, SceneError>>()?;
    let lengths = route
        .windows(2)
        .map(|p| {
            let dx = (p[1][0] - p[0][0]).abs();
            let dy = (p[1][1] - p[0][1]).abs();
            if dx > 1e-6 && dy > 1e-6 {
                return Err(fail("diagonal or curved segments require native editing"));
            }
            Ok(dx + dy)
        })
        .collect::<Result<Vec<_>, SceneError>>()?;
    let total = lengths.iter().sum::<f64>();
    if total <= 1e-6 {
        return Err(fail("arrow path is empty"));
    }
    let mut remaining = total * position;
    let mut center = route[0];
    for (i, length) in lengths.iter().enumerate() {
        if *length > 0. && remaining <= *length {
            let f = remaining / length;
            center = [
                route[i][0] + f * (route[i + 1][0] - route[i][0]),
                route[i][1] + f * (route[i + 1][1] - route[i][1]),
            ];
            break;
        }
        remaining -= length;
    }
    let left = center[0] - w / 2.;
    let top = center[1] - h / 2.;
    if ![left, top, left + w, top + h]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 1e6)
    {
        return Err(fail("caption would leave canvas coordinate range"));
    }
    if let Some(frame_id) = text["frameId"].as_str() {
        let frame = elements
            .iter()
            .find(|e| e["id"] == frame_id && e["isDeleted"] != true)
            .ok_or_else(|| fail("caption frame is missing"))?;
        if frame["type"] != "frame" || frame["angle"].as_f64().unwrap_or(0.) != 0. {
            return Err(fail("caption frame must be unrotated"));
        }
        let (fx, fy, fw, fh) = (
            number(frame, "x")?,
            number(frame, "y")?,
            number(frame, "width")?,
            number(frame, "height")?,
        );
        if left < fx - 1e-6
            || top < fy - 1e-6
            || left + w > fx + fw + 1e-6
            || top + h > fy + fh + 1e-6
        {
            return Err(fail(
                "complete caption would leave its frame; adjust layout explicitly",
            ));
        }
    }
    preserve_center(&mut route, center);
    if route.len() > 512 {
        return Err(fail(
            "native label placement would exceed 512 points; simplify the route explicitly",
        ));
    }
    let native = if route.len() % 2 == 1 {
        route[route.len() / 2]
    } else {
        let i = route.len() / 2;
        [
            (route[i - 1][0] + route[i][0]) / 2.,
            (route[i - 1][1] + route[i][1]) / 2.,
        ]
    };
    if !close(native, center) {
        return Err(fail(
            "requested center cannot be represented safely on the native path",
        ));
    }
    elements[ai]["points"] = json!(
        route
            .iter()
            .map(|p| [p[0] - x, p[1] - y])
            .collect::<Vec<_>>()
    );
    scene::touch(&mut elements[ai]);
    elements[ti]["x"] = json!(left);
    elements[ti]["y"] = json!(top);
    scene::touch(&mut elements[ti]);
    Ok(())
}
