//! Bounded text rectangle hints matching the GUI; not glyph-ink or scientific review.
use serde::Serialize;
use serde_json::Value;
#[derive(Debug, Clone, Serialize)]
pub struct TextRectangleOverlap {
    pub scope: &'static str,
    pub status: &'static str,
    pub total_text_count: usize,
    pub checked_text_count: usize,
    pub skipped_text_count: usize,
    pub overlap_count: usize,
    pub pair_omitted_count: usize,
    pub pairs: Vec<TextPair>,
}
#[derive(Debug, Clone, Serialize)]
pub struct TextPair {
    pub first: String,
    pub second: String,
}
fn number(v: &Value) -> Option<f64> {
    v.as_f64().filter(|v| v.is_finite() && v.abs() <= 1e6)
}
pub fn review(document: &Value) -> TextRectangleOverlap {
    let mut result = TextRectangleOverlap {
        scope: "all_live_unrotated_text_rectangles_excluding_glyph_ink_and_scientific_validation",
        status: "unavailable",
        total_text_count: 0,
        checked_text_count: 0,
        skipped_text_count: 0,
        overlap_count: 0,
        pair_omitted_count: 0,
        pairs: vec![],
    };
    let Some(elements) = document["elements"].as_array() else {
        return result;
    };
    let mut boxes = Vec::new();
    for e in elements
        .iter()
        .filter(|e| e["type"] == "text" && e["isDeleted"] != true)
    {
        result.total_text_count += 1;
        let rect = (|| {
            let id = e["id"]
                .as_str()
                .filter(|s| s.encode_utf16().count() <= 512)?;
            if e["angle"].as_f64() != Some(0.)
                || e["opacity"].as_f64() == Some(0.)
                || boxes.len() >= 1000
            {
                return None;
            }
            let b = [
                number(&e["x"])?,
                number(&e["y"])?,
                number(&e["width"])?,
                number(&e["height"])?,
            ];
            if b[2] <= 0. || b[3] <= 0. {
                return None;
            }
            Some((id, b))
        })();
        if let Some(rect) = rect {
            boxes.push(rect);
        } else {
            result.skipped_text_count += 1;
        }
    }
    result.checked_text_count = boxes.len();
    result.status = if result.skipped_text_count == 0 {
        "complete"
    } else {
        "partial"
    };
    for (i, (id, a)) in boxes.iter().enumerate() {
        for (other, b) in boxes.iter().skip(i + 1) {
            if id != other
                && (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]) > 1.
                && (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]) > 1.
            {
                result.overlap_count += 1;
                if result.pairs.len() < 50 {
                    result.pairs.push(TextPair {
                        first: (*id).into(),
                        second: (*other).into(),
                    });
                }
            }
        }
    }
    result.pair_omitted_count = result.overlap_count - result.pairs.len();
    result
}
