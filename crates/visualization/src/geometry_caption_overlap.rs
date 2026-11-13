//! Bounded rectangle/path overlap hints; not glyph-ink or renderer validation.
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct CaptionOverlap {
    pub scope: &'static str,
    pub checked_count: usize,
    pub skipped_count: usize,
    pub warning_omitted_count: usize,
    pub warnings: Vec<CaptionWarning>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CaptionWarning {
    pub arrow_id: String,
    pub label_id: String,
    pub path_length: f64,
    pub covered_path_length: f64,
    pub uncovered_path_length: f64,
    pub covered_fraction: f64,
}
fn n(v: &Value) -> Option<f64> {
    v.as_f64().filter(|v| v.is_finite() && v.abs() <= 1e6)
}
fn unrotated(e: &Value) -> bool {
    e["angle"].is_null() || n(&e["angle"]) == Some(0.)
}
fn inside_length(a: [f64; 2], b: [f64; 2], rect: [f64; 4]) -> f64 {
    let mut lo: f64 = 0.;
    let mut hi: f64 = 1.;
    for axis in 0..2 {
        let delta = b[axis] - a[axis];
        if delta.abs() < 1e-12 {
            if a[axis] < rect[axis] || a[axis] > rect[axis + 2] {
                return 0.;
            }
        } else {
            let t1 = (rect[axis] - a[axis]) / delta;
            let t2 = (rect[axis + 2] - a[axis]) / delta;
            lo = lo.max(t1.min(t2));
            hi = hi.min(t1.max(t2));
            if hi <= lo {
                return 0.;
            }
        }
    }
    (hi - lo) * (b[0] - a[0]).hypot(b[1] - a[1])
}
fn measure(arrow: &Value, label: &Value) -> Option<(f64, f64)> {
    if !unrotated(arrow)
        || !unrotated(label)
        || !arrow["roundness"].is_null()
        || arrow["elbowed"] == true
        || (!arrow["roughness"].is_null() && n(&arrow["roughness"]) != Some(0.))
    {
        return None;
    }
    let (x, y, w, h) = (
        n(&label["x"])?,
        n(&label["y"])?,
        n(&label["width"])?,
        n(&label["height"])?,
    );
    if w <= 0. || h <= 0. {
        return None;
    }
    let (ax, ay) = (n(&arrow["x"])?, n(&arrow["y"])?);
    let raw = arrow["points"]
        .as_array()
        .filter(|p| (2..=1000).contains(&p.len()))?;
    let points = raw
        .iter()
        .map(|p| {
            let pair = p.as_array().filter(|p| p.len() == 2)?;
            Some([ax + n(&pair[0])?, ay + n(&pair[1])?])
        })
        .collect::<Option<Vec<_>>>()?;
    let mut total = 0.;
    let mut covered = 0.;
    for segment in points.windows(2) {
        total += (segment[1][0] - segment[0][0]).hypot(segment[1][1] - segment[0][1]);
        covered += inside_length(segment[0], segment[1], [x, y, x + w, y + h]);
    }
    (total > 0.).then_some((total, covered))
}
pub fn review(elements: &[&Value]) -> CaptionOverlap {
    let mut result = CaptionOverlap {
        scope: "unrotated_sharp_native_arrow_centerlines_vs_single_reciprocally_bound_text_rectangle_not_glyph_ink_or_renderer_visibility_warn_fraction_at_least_0_8_or_fraction_at_least_0_5_with_uncovered_length_below_32",
        checked_count: 0,
        skipped_count: 0,
        warning_omitted_count: 0,
        warnings: Vec::new(),
    };
    let index: HashMap<_, _> = elements
        .iter()
        .filter_map(|e| e["id"].as_str().map(|id| (id, *e)))
        .collect();
    for label in elements
        .iter()
        .filter(|e| e["type"] == "text" && e["containerId"].is_string())
    {
        let Some(arrow) = index
            .get(label["containerId"].as_str().unwrap())
            .filter(|e| e["type"] == "arrow")
        else {
            continue;
        };
        let bound = arrow["boundElements"].as_array().map(|items| {
            items
                .iter()
                .filter(|e| e["type"] == "text")
                .collect::<Vec<_>>()
        });
        if !bound.is_some_and(|items| items.len() == 1 && items[0]["id"] == label["id"]) {
            result.skipped_count += 1;
            continue;
        }
        let Some((total, covered)) = measure(arrow, label) else {
            result.skipped_count += 1;
            continue;
        };
        result.checked_count += 1;
        let remaining = (total - covered).max(0.);
        let fraction = (covered / total).clamp(0., 1.);
        if fraction >= 0.8 || fraction >= 0.5 && remaining < 32. {
            if result.warnings.len() >= 20 {
                result.warning_omitted_count += 1;
                continue;
            }
            result.warnings.push(CaptionWarning {
                arrow_id: arrow["id"].as_str().unwrap_or_default().into(),
                label_id: label["id"].as_str().unwrap_or_default().into(),
                path_length: total,
                covered_path_length: covered,
                uncovered_path_length: remaining,
                covered_fraction: fraction,
            });
        }
    }
    result
}
