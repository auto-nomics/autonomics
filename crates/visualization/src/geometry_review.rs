//! Read-only native object extents, including captions and artwork viewports.
//! These metrics do not measure glyph/stroke collisions or scientific quality.
#[path = "geometry_caption_overlap.rs"]
mod caption_overlap;
pub use caption_overlap::{CaptionOverlap, CaptionWarning};
#[path = "geometry_text_overlap.rs"]
mod text_overlap;
pub use text_overlap::{TextRectangleOverlap, TextPair};
use serde::Serialize;
use serde_json::Value;
const MAX_ELEMENTS: usize = 5000;
#[derive(Debug, Clone, Serialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Debug, Clone, Serialize)]
pub struct GeometryReview {
    pub text_rectangle_overlap: TextRectangleOverlap,
    pub caption_path_overlap: CaptionOverlap,
    pub status: &'static str,
    pub scope: &'static str,
    pub bounds: Option<Bounds>,
    pub aspect_ratio: Option<f64>,
    pub live_element_count: usize,
    pub checked_element_count: usize,
    pub skipped_element_count: usize,
    pub omitted_element_count: usize,
    pub min_text_font_size: Option<f64>,
    pub unknown_text_font_size_count: usize,
}
fn number(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite() && n.abs() <= 1e6)
}
pub(crate) fn extents(e: &Value) -> Option<[f64; 4]> {
    let x = number(&e["x"])?;
    let y = number(&e["y"])?;
    let angle = if e["angle"].is_null() {
        0.
    } else {
        number(&e["angle"])?
    };
    let mut points = if matches!(e["type"].as_str(), Some("arrow" | "line" | "freedraw")) {
        if !e["roundness"].is_null()
            || e["elbowed"] == true
            || !e["roughness"].is_null() && number(&e["roughness"]) != Some(0.)
        {
            return None;
        }
        let raw = e["points"]
            .as_array()
            .filter(|p| !p.is_empty() && p.len() <= 1000)?;
        raw.iter()
            .map(|p| {
                let p = p.as_array().filter(|p| p.len() == 2)?;
                Some([x + number(&p[0])?, y + number(&p[1])?])
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        if !matches!(
            e["type"].as_str(),
            Some(
                "rectangle"
                    | "ellipse"
                    | "diamond"
                    | "text"
                    | "image"
                    | "frame"
                    | "magicframe"
                    | "embeddable"
                    | "iframe"
            )
        ) {
            return None;
        }
        let w = number(&e["width"])?;
        let h = number(&e["height"])?;
        if w < 0. || h < 0. {
            return None;
        }
        vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
    };
    let bounds = |points: &[[f64; 2]]| {
        points.iter().fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |mut b, p| {
                b[0] = b[0].min(p[0]);
                b[1] = b[1].min(p[1]);
                b[2] = b[2].max(p[0]);
                b[3] = b[3].max(p[1]);
                b
            },
        )
    };
    if angle != 0. {
        let b = bounds(&points);
        let center = [(b[0] + b[2]) / 2., (b[1] + b[3]) / 2.];
        let (s, c) = angle.sin_cos();
        for p in &mut points {
            let dx = p[0] - center[0];
            let dy = p[1] - center[1];
            *p = [center[0] + c * dx - s * dy, center[1] + s * dx + c * dy];
        }
    }
    Some(bounds(&points))
}
pub fn review(document: &Value) -> GeometryReview {
    let empty = Vec::new();
    let elements = document["elements"].as_array().unwrap_or(&empty);
    let live: Vec<_> = elements.iter().filter(|e| e["isDeleted"] != true).collect();
    let mut result = GeometryReview {
        text_rectangle_overlap: text_overlap::review(document),
        caption_path_overlap: caption_overlap::review(
            &live.iter().take(MAX_ELEMENTS).copied().collect::<Vec<_>>(),
        ),
        status: "empty",
        scope: "all_live_native_object_rectangles_and_straight_path_points_including_captions_and_image_viewports_excluding_strokes_glyph_ink_frame_names_and_pixel_collision_review",
        bounds: None,
        aspect_ratio: None,
        live_element_count: live.len(),
        checked_element_count: 0,
        skipped_element_count: 0,
        omitted_element_count: live.len().saturating_sub(MAX_ELEMENTS),
        min_text_font_size: None,
        unknown_text_font_size_count: 0,
    };
    let mut combined = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    if !document["elements"].is_array() {
        result.status = "unavailable";
        return result;
    }
    for e in live.iter().take(MAX_ELEMENTS) {
        if e["type"] == "text" {
            if let Some(size) = number(&e["fontSize"]).filter(|n| *n > 0.) {
                result.min_text_font_size =
                    Some(result.min_text_font_size.map_or(size, |old| old.min(size)));
            } else {
                result.unknown_text_font_size_count += 1;
            }
        }
        let Some(b) = extents(e) else {
            result.skipped_element_count += 1;
            continue;
        };
        for i in 0..2 {
            combined[i] = combined[i].min(b[i]);
            combined[i + 2] = combined[i + 2].max(b[i + 2]);
        }
        result.checked_element_count += 1;
    }
    if result.checked_element_count > 0 {
        let width = combined[2] - combined[0];
        let height = combined[3] - combined[1];
        result.bounds = Some(Bounds {
            x: combined[0],
            y: combined[1],
            width,
            height,
        });
        if width > 0. && height > 0. {
            result.aspect_ratio = Some(width / height);
        }
        result.status = if result.skipped_element_count + result.omitted_element_count > 0 {
            "partial"
        } else {
            "available"
        };
    } else if !live.is_empty() {
        result.status = "unavailable";
    }
    result
}
