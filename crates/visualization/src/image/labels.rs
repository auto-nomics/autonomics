//! Conservative initial caption placement for narrow scientific illustrations.
//! Existing illustration captions retain user positioning on later replacement.
use crate::layout;
use serde_json::{Value, json};
fn number(e: &Value, k: &str) -> Option<f64> {
    e[k].as_f64().filter(|n| n.is_finite() && n.abs() <= 1e6)
}
fn unrotated(e: &Value) -> bool {
    e["angle"].is_null() || number(e, "angle") == Some(0.)
}
fn rect(e: &Value) -> Option<[f64; 4]> {
    let r = [
        number(e, "x")?,
        number(e, "y")?,
        number(e, "width")?,
        number(e, "height")?,
    ];
    (r[2] > 0. && r[3] > 0.).then_some(r)
}
fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[0] + b[2] + 6.
        && a[0] + a[2] + 6. > b[0]
        && a[1] < b[1] + b[3] + 6.
        && a[1] + a[3] + 6. > b[1]
}
fn points(e: &Value) -> Option<Vec<[f64; 2]>> {
    if !unrotated(e) || e["elbowed"] == true || !e["roundness"].is_null() {
        return None;
    }
    let (x, y) = (number(e, "x")?, number(e, "y")?);
    let raw = e["points"]
        .as_array()
        .filter(|p| (2..=1000).contains(&p.len()))?;
    raw.iter()
        .map(|p| {
            let p = p.as_array().filter(|p| p.len() == 2)?;
            Some([
                p[0].as_f64().filter(|n| n.is_finite())? + x,
                p[1].as_f64().filter(|n| n.is_finite())? + y,
            ])
        })
        .collect()
}
fn crosses(a: [f64; 2], b: [f64; 2], r: [f64; 4]) -> bool {
    let mut enter: f64 = 0.;
    let mut leave: f64 = 1.;
    for axis in 0..2 {
        let low = r[axis] - 6.;
        let high = r[axis] + r[axis + 2] + 6.;
        let delta = b[axis] - a[axis];
        if delta.abs() < 1e-9 {
            if a[axis] < low || a[axis] > high {
                return false;
            }
        } else {
            let t1 = (low - a[axis]) / delta;
            let t2 = (high - a[axis]) / delta;
            enter = enter.max(t1.min(t2));
            leave = leave.min(t1.max(t2));
            if leave < enter {
                return false;
            }
        }
    }
    leave >= enter
}
fn blocked(
    elements: &[Value],
    anchor_id: &str,
    image_id: &str,
    label_id: &str,
    candidate: [f64; 4],
) -> bool {
    elements.iter().any(|e| {
        if e["isDeleted"] == true
            || e["opacity"] == 0
            || e["id"] == anchor_id
            || e["id"] == image_id
            || e["id"] == label_id
            || matches!(e["type"].as_str(), Some("frame" | "magicframe"))
        {
            return false;
        }
        if matches!(e["type"].as_str(), Some("arrow" | "line")) {
            return points(e).is_none_or(|p| p.windows(2).any(|p| crosses(p[0], p[1], candidate)));
        }
        if !matches!(
            e["type"].as_str(),
            Some("text" | "rectangle" | "ellipse" | "diamond" | "image")
        ) {
            return true;
        }
        !unrotated(e) || rect(e).is_none_or(|r| overlaps(candidate, r))
    })
}
pub(super) fn place_beside(
    elements: &mut [Value],
    anchor_id: &str,
    image_id: &str,
    labels: &[String],
    slot: [f64; 4],
) {
    if labels.len() != 1 || elements.len() > 500 {
        return;
    }
    let Some(index) = elements.iter().position(|e| e["id"] == labels[0]) else {
        return;
    };
    let label = &elements[index];
    if label["fontFamily"] != 9 || label["locked"] == true || !unrotated(label) {
        return;
    }
    let Some(size) = number(label, "fontSize").filter(|n| (4. ..=200.).contains(n)) else {
        return;
    };
    let Some(line_height) = number(label, "lineHeight").filter(|n| (0.5..=3.).contains(n)) else {
        return;
    };
    let Some(source) = label["originalText"]
        .as_str()
        .or_else(|| label["text"].as_str())
        .filter(|s| !s.is_empty() && s.chars().count() <= 2048)
    else {
        return;
    };
    let Some(image) = elements
        .iter()
        .find(|e| e["id"] == image_id && unrotated(e))
        .and_then(rect)
    else {
        return;
    };
    if image[2] > slot[2] / 2. {
        return;
    }
    let exits_below = elements.iter().any(|e| {
        let start = e["startBinding"]["elementId"] == anchor_id;
        let end = e["endBinding"]["elementId"] == anchor_id;
        if e["type"] != "arrow" || e["isDeleted"] == true || e["opacity"] == 0 || !start && !end {
            return false;
        }
        points(e)
            .and_then(|p| {
                if start {
                    p.first().copied()
                } else {
                    p.last().copied()
                }
            })
            .is_some_and(|p| {
                p[1] >= slot[1] + slot[3]
                    && p[1] <= slot[1] + slot[3] + 100.
                    && p[0] >= slot[0]
                    && p[0] <= slot[0] + slot[2]
            })
    });
    if !exits_below {
        return;
    }
    let frame = elements
        .iter()
        .find(|e| !label["frameId"].is_null() && e["id"] == label["frameId"]);
    let region = if let Some(frame) = frame {
        let Some(r) = rect(frame) else {
            return;
        };
        if !unrotated(frame) || slot[1] < r[1] || slot[1] + slot[3] > r[1] + r[3] {
            return;
        }
        [r[0] + 16., slot[1], r[2] - 32., slot[3]]
    } else {
        slot
    };
    let Some(original_width) = number(label, "width") else {
        return;
    };
    for right in [true, false] {
        let room = if right {
            region[0] + region[2] - image[0] - image[2] - 12.
        } else {
            image[0] - 12. - region[0]
        };
        let width = original_width.min(room);
        if width < size * 3. {
            continue;
        }
        let wrapped = layout::wrap(source, width, size);
        if wrapped
            .lines()
            .any(|line| layout::text_width(line, size) > width + 1e-6)
        {
            continue;
        }
        let height = wrapped.split('\n').count() as f64 * size * line_height;
        if height > region[3] {
            continue;
        }
        let x = if right {
            image[0] + image[2] + 12.
        } else {
            image[0] - 12. - width
        };
        let y = slot[1] + (slot[3] - height) / 2.;
        let candidate = [x, y, width, height];
        let blocked = blocked(elements, anchor_id, image_id, &labels[0], candidate);
        if blocked {
            continue;
        }
        let label = &mut elements[index];
        for (key, value) in ["x", "y", "width", "height"].into_iter().zip(candidate) {
            label[key] = json!(value);
        }
        label["text"] = json!(wrapped);
        label["autoResize"] = json!(false);
        return;
    }
}

/// A fresh illustration's default below-image caption must not be hidden by
/// frame clipping. Try measured above/below positions inside its own frame.
/// Existing user-positioned captions on later replacements are not moved.
pub(super) fn keep_in_frame(
    elements: &mut [Value],
    anchor_id: &str,
    image_id: &str,
    labels: &[String],
) {
    if labels.len() != 1 || elements.len() > 500 {
        return;
    }
    let Some(index) = elements.iter().position(|e| e["id"] == labels[0]) else {
        return;
    };
    let label = &elements[index];
    if label["fontFamily"] != 9 || label["locked"] == true || !unrotated(label) {
        return;
    }
    let Some(frame) = elements
        .iter()
        .find(|e| {
            e["id"] == label["frameId"]
                && e["type"] == "frame"
                && e["isDeleted"] != true
                && unrotated(e)
        })
        .and_then(rect)
    else {
        return;
    };
    let Some(old) = rect(label) else {
        return;
    };
    let inside = |r: [f64; 4], margin: f64| {
        r[0] >= frame[0] + margin
            && r[1] >= frame[1] + margin
            && r[0] + r[2] <= frame[0] + frame[2] - margin
            && r[1] + r[3] <= frame[1] + frame[3] - margin
    };
    if inside(old, 0.) {
        return;
    }
    let Some(image) = elements
        .iter()
        .find(|e| e["id"] == image_id && unrotated(e))
        .and_then(rect)
    else {
        return;
    };
    if !inside(image, 0.) {
        return;
    }
    let (Some(size), Some(line_height), Some(source)) = (
        number(label, "fontSize"),
        number(label, "lineHeight"),
        label["originalText"].as_str(),
    ) else {
        return;
    };
    if !(4. ..=200.).contains(&size)
        || !(0.5..=3.).contains(&line_height)
        || source.is_empty()
        || source.chars().count() > 2048
    {
        return;
    }
    let available = frame[2] - 32.;
    if available < size * 3. {
        return;
    }
    for width in [old[2].min(available), available] {
        let wrapped = layout::wrap(source, width, size);
        if wrapped
            .lines()
            .any(|line| layout::text_width(line, size) > width + 1e-6)
        {
            continue;
        }
        let height = wrapped.split('\n').count() as f64 * size * line_height;
        let x = (image[0] + image[2] / 2. - width / 2.).clamp(
            frame[0] + 16.,
            (frame[0] + frame[2] - 16. - width).max(frame[0] + 16.),
        );
        for y in [image[1] - 12. - height, image[1] + image[3] + 12.] {
            let candidate = [x, y, width, height];
            if !inside(candidate, 8.)
                || blocked(elements, anchor_id, image_id, &labels[0], candidate)
            {
                continue;
            }
            let label = &mut elements[index];
            for (key, value) in ["x", "y", "width", "height"].into_iter().zip(candidate) {
                label[key] = json!(value);
            }
            label["text"] = json!(wrapped);
            label["autoResize"] = json!(false);
            return;
        }
    }
}

pub(super) fn contained_in_frames(elements: &[Value], labels: &[String]) -> bool {
    labels.iter().all(|id| {
        let Some(label) = elements.iter().find(|e| e["id"] == *id) else {
            return false;
        };
        let Some(frame) = elements.iter().find(|e| {
            e["id"] == label["frameId"] && e["type"] == "frame" && e["isDeleted"] != true
        }) else {
            return true;
        };
        let (Some(a), Some(b)) = (
            crate::geometry_review::extents(label),
            crate::geometry_review::extents(frame),
        ) else {
            return false;
        };
        a[0] >= b[0] - 1e-6 && a[1] >= b[1] - 1e-6 && a[2] <= b[2] + 1e-6 && a[3] <= b[3] + 1e-6
    })
}
