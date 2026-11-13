//! Conservative read-only placement suggestions, not a packing or pixel solver.
use crate::geometry_review::{self, Bounds, GeometryReview};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, schemars::JsonSchema, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub relative_to: String,
    #[schemars(range(min = 1.0, max = 4096.0))]
    pub width: f64,
    #[schemars(range(min = 1.0, max = 4096.0))]
    pub height: f64,
    #[schemars(range(min = 4.0, max = 256.0))]
    pub gap: f64,
}
#[derive(Debug, Serialize)]
pub struct Candidate {
    pub position: &'static str,
    pub bounds: Bounds,
    pub added_bounding_area: f64,
    pub resulting_bounds: Bounds,
    pub resulting_aspect_ratio: f64,
}
#[derive(Debug, Serialize)]
pub struct Review {
    pub status: &'static str,
    pub scope: &'static str,
    pub relative_to: String,
    pub gap: f64,
    pub candidates: Vec<Candidate>,
    pub scene_geometry_review: GeometryReview,
}
pub fn validate(spec: &Spec) -> Result<(), &'static str> {
    if spec.relative_to.trim().is_empty() || spec.relative_to.len() > 512 {
        return Err("relative_to requires a verbatim nonblank object ID up to 512 bytes");
    }
    if ![spec.width, spec.height]
        .iter()
        .all(|n| n.is_finite() && (1.0..=4096.0).contains(n))
        || !spec.gap.is_finite()
        || !(4.0..=256.0).contains(&spec.gap)
    {
        return Err("width/height require finite numbers in 1..4096; gap requires 4..256");
    }
    Ok(())
}
fn intersects(rect: &Bounds, other: &[f64; 4], gap: f64) -> bool {
    rect.x < other[2] + gap
        && rect.x + rect.width > other[0] - gap
        && rect.y < other[3] + gap
        && rect.y + rect.height > other[1] - gap
}
fn internal_axis(mut values: Vec<f64>, low: f64, high: f64, reference: f64) -> Vec<f64> {
    values.retain(|v| v.is_finite() && *v >= low && *v <= high && v.abs() <= 1e6);
    values.sort_by(|a, b| {
        (a - reference)
            .abs()
            .total_cmp(&(b - reference).abs())
            .then(a.total_cmp(b))
    });
    values.dedup_by(|a, b| *a == *b);
    values.truncate(32);
    values
}
pub fn find(document: &Value, spec: &Spec) -> Result<Review, &'static str> {
    validate(spec)?;
    let elements = document["elements"]
        .as_array()
        .ok_or("scene elements are required")?;
    let matching: Vec<_> = elements
        .iter()
        .filter(|e| e["id"].as_str() == Some(&spec.relative_to))
        .collect();
    if matching.len() != 1 || matching[0]["isDeleted"] == true {
        return Err("relative_to is missing, deleted or ambiguous");
    }
    let geometry = geometry_review::review(document);
    let mut review = Review {
        status: "unavailable",
        scope: "bounded adjacent, internal obstacle-edge (at most 32 coordinates per axis), and outside-scene rectangle candidates; at most 8 returned ranked by added bounding area then reference proximity; complete native extents required; includes frames/text/image viewports and sharp path bounds, excludes glyph/stroke ink and scientific suitability; not an exhaustive packing search",
        relative_to: spec.relative_to.clone(),
        gap: spec.gap,
        candidates: vec![],
        scene_geometry_review: geometry,
    };
    if review.scene_geometry_review.skipped_element_count != 0
        || review.scene_geometry_review.omitted_element_count != 0
    {
        return Ok(review);
    }
    let reference =
        geometry_review::extents(matching[0]).ok_or("unsupported reference geometry")?;
    let scene = review
        .scene_geometry_review
        .bounds
        .as_ref()
        .ok_or("scene bounds unavailable")?;
    let obstacles: Vec<_> = elements
        .iter()
        .filter(|e| e["isDeleted"] != true)
        .map(geometry_review::extents)
        .collect::<Option<_>>()
        .ok_or("incomplete geometry coverage")?;
    let cx = (reference[0] + reference[2] - spec.width) / 2.0;
    let cy = (reference[1] + reference[3] - spec.height) / 2.0;
    let mut positions = vec![
        ("below", cx, reference[3] + spec.gap),
        ("right", reference[2] + spec.gap, cy),
        ("left", reference[0] - spec.gap - spec.width, cy),
        ("above", cx, reference[1] - spec.gap - spec.height),
        ("scene_below", cx, scene.y + scene.height + spec.gap),
        ("scene_right", scene.x + scene.width + spec.gap, cy),
        ("scene_left", scene.x - spec.gap - spec.width, cy),
        ("scene_above", cx, scene.y - spec.gap - spec.height),
    ];
    let mut xs = vec![scene.x, scene.x + scene.width - spec.width, cx];
    let mut ys = vec![scene.y, scene.y + scene.height - spec.height, cy];
    for other in &obstacles {
        xs.extend([other[2] + spec.gap, other[0] - spec.gap - spec.width]);
        ys.extend([other[3] + spec.gap, other[1] - spec.gap - spec.height]);
    }
    let xs = internal_axis(xs, scene.x, scene.x + scene.width - spec.width, cx);
    let ys = internal_axis(ys, scene.y, scene.y + scene.height - spec.height, cy);
    for x in xs {
        for y in &ys {
            positions.push(("internal_gap", x, *y));
        }
    }
    for (position, x, y) in positions {
        let bounds = Bounds {
            x,
            y,
            width: spec.width,
            height: spec.height,
        };
        if [x, y, x + spec.width, y + spec.height]
            .iter()
            .any(|n| !n.is_finite() || n.abs() > 1e6)
            || obstacles
                .iter()
                .any(|other| intersects(&bounds, other, spec.gap))
            || review
                .candidates
                .iter()
                .any(|old| old.bounds.x == x && old.bounds.y == y)
        {
            continue;
        }
        let area = (scene.x + scene.width).max(x + spec.width) - scene.x.min(x);
        let height = (scene.y + scene.height).max(y + spec.height) - scene.y.min(y);
        review.candidates.push(Candidate {
            position,
            bounds,
            added_bounding_area: (area * height - scene.width * scene.height).max(0.0),
            resulting_bounds: Bounds {
                x: scene.x.min(x),
                y: scene.y.min(y),
                width: area,
                height,
            },
            resulting_aspect_ratio: area / height,
        });
    }
    review.candidates.sort_by(|a, b| {
        a.added_bounding_area
            .total_cmp(&b.added_bounding_area)
            .then_with(|| {
                let distance =
                    |c: &Candidate| (c.bounds.x - cx).powi(2) + (c.bounds.y - cy).powi(2);
                distance(a).total_cmp(&distance(b))
            })
    });
    review.candidates.truncate(8);
    review.status = if review.candidates.is_empty() {
        "no_candidate"
    } else {
        "available"
    };
    Ok(review)
}
