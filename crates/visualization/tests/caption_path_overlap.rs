use serde_json::{Value, json};
use visualization::geometry_review::review;
fn scene() -> Value {
    json!({"elements":[{"id":"edge","type":"arrow","x":0,"y":0,"angle":0,"points":[[0,0],[200,0]],"boundElements":[{"id":"caption","type":"text"}]},{"id":"caption","type":"text","containerId":"edge","x":10,"y":-10,"width":180,"height":20,"angle":0,"fontSize":16,"text":"Explicit relation"}]})
}
#[test]
fn detects_caption_rectangles_covering_most_of_a_sharp_arrow_without_writing_scene() {
    let original = scene();
    let document = original.clone();
    let result = review(&document).caption_path_overlap;
    assert_eq!(document, original);
    assert_eq!(result.checked_count, 1);
    assert_eq!(result.skipped_count, 0);
    let warning = &result.warnings[0];
    assert_eq!(warning.arrow_id, "edge");
    assert_eq!(warning.label_id, "caption");
    assert!((warning.path_length - 200.).abs() < 1e-6);
    assert!((warning.covered_path_length - 180.).abs() < 1e-6);
    assert!((warning.uncovered_path_length - 20.).abs() < 1e-6);
    assert!((warning.covered_fraction - 0.9).abs() < 1e-6);
    for (x, y, w, h) in [(250., -10., 180., 20.), (80., -10., 40., 20.)] {
        let mut doc = scene();
        let label = &mut doc["elements"][1];
        label["x"] = json!(x);
        label["y"] = json!(y);
        label["width"] = json!(w);
        label["height"] = json!(h);
        assert!(review(&doc).caption_path_overlap.warnings.is_empty());
    }
}
#[test]
fn handles_diagonal_world_coordinates_and_reports_unsupported_coverage() {
    let mut doc = scene();
    doc["elements"][0]["x"] = json!(30);
    doc["elements"][0]["y"] = json!(40);
    doc["elements"][0]["points"] = json!([[0, 0], [100, 100]]);
    let label = &mut doc["elements"][1];
    label["x"] = json!(40);
    label["y"] = json!(50);
    label["width"] = json!(80);
    label["height"] = json!(80);
    let result = review(&doc).caption_path_overlap;
    assert_eq!(result.warnings.len(), 1);
    assert!((result.warnings[0].covered_fraction - 0.8).abs() < 1e-6);
    for (field, value) in [
        ("roundness", json!({"type":2})),
        ("angle", json!(0.5)),
        ("points", json!([[0, 0], ["100", 100]])),
        ("elbowed", json!(true)),
        ("boundElements", json!([])),
    ] {
        let mut invalid = doc.clone();
        invalid["elements"][0][field] = value;
        let result = review(&invalid).caption_path_overlap;
        assert_eq!(result.checked_count, 0);
        assert_eq!(result.skipped_count, 1);
        assert!(result.warnings.is_empty());
    }
    let mut deleted = doc;
    deleted["elements"][1]["isDeleted"] = json!(true);
    assert_eq!(review(&deleted).caption_path_overlap.checked_count, 0);
}

#[test]
fn warning_output_is_bounded_and_reports_omissions() {
    let mut elements = Vec::new();
    for i in 0..25 {
        let mut doc = scene();
        let arrow = format!("edge-{i}");
        let label = format!("caption-{i}");
        doc["elements"][0]["id"] = json!(arrow);
        doc["elements"][0]["boundElements"][0]["id"] = json!(label);
        doc["elements"][1]["id"] = json!(label);
        doc["elements"][1]["containerId"] = json!(arrow);
        elements.extend(doc["elements"].as_array().unwrap().iter().cloned());
    }
    let result = review(&json!({"elements":elements})).caption_path_overlap;
    assert_eq!(result.checked_count, 25);
    assert_eq!(result.warnings.len(), 20);
    assert_eq!(result.warning_omitted_count, 5);
    assert_eq!(result.skipped_count, 0);
}
