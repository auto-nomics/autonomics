use serde_json::{Value, json};
use visualization::geometry_review::review;
fn text(id: &str, x: f64) -> Value {
    json!({"id":id,"type":"text","x":x,"y":10,"width":20,"height":20,"angle":0,"opacity":100})
}
#[test]
fn detached_labels_and_bound_captions_are_both_checked_read_only() {
    let doc = json!({"elements":[text("label",10.),text("caption",20.),text("touching",39.)]});
    let before = doc.clone();
    let r = review(&doc).text_rectangle_overlap;
    assert_eq!(r.status, "complete");
    assert_eq!(r.checked_text_count, 3);
    assert_eq!(r.overlap_count, 1);
    assert_eq!(r.pairs[0].first, "label");
    assert_eq!(r.pairs[0].second, "caption");
    assert_eq!(doc, before);
}
#[test]
fn unsupported_rotations_and_invalid_bounds_report_partial_coverage() {
    let mut rotated = text("rotated", 0.);
    rotated["angle"] = json!(0.1);
    let mut invalid = text("invalid", 0.);
    invalid["width"] = Value::Null;
    let r = review(&json!({"elements":[text("ok",0.),rotated,invalid]})).text_rectangle_overlap;
    assert_eq!(r.status, "partial");
    assert_eq!(r.total_text_count, 3);
    assert_eq!(r.skipped_text_count, 2);
    assert_eq!(r.overlap_count, 0);
    assert_eq!(
        review(&json!({})).text_rectangle_overlap.status,
        "unavailable"
    );
}
#[test]
fn pair_and_text_limits_have_explicit_omission_counts() {
    let doc = json!({"elements":(0..12).map(|i|text(&format!("t{i}"),0.)).collect::<Vec<_>>()});
    let r = review(&doc).text_rectangle_overlap;
    assert_eq!(r.overlap_count, 66);
    assert_eq!(r.pairs.len(), 50);
    assert_eq!(r.pair_omitted_count, 16);
    let doc = json!({"elements":(0..1001).map(|i|text(&format!("t{i}"),i as f64*30.)).collect::<Vec<_>>()});
    let r = review(&doc).text_rectangle_overlap;
    assert_eq!(r.checked_text_count, 1000);
    assert_eq!(r.skipped_text_count, 1);
    assert_eq!(r.status, "partial");
}
#[test]
fn optional_real_failed_candidate_reports_the_gui_overlap() {
    let Ok(path) = std::env::var("VISUALIZATION_TEXT_REVIEW_SOURCE") else {
        return;
    };
    let input: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let r = review(&input["document"]).text_rectangle_overlap;
    assert_eq!(r.status, "complete");
    assert_eq!(r.overlap_count, 1);
    assert!(
        r.pairs
            .iter()
            .any(|p| (p.first == "label-6" && p.second == "edge-label-6")
                || (p.second == "label-6" && p.first == "edge-label-6"))
    );
}
