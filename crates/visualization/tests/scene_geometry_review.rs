use serde_json::json;
use visualization::geometry_review::review;
#[test]
fn full_candidate_metrics_include_the_caption_that_plan_metrics_exclude() {
    let document = json!({"elements":[{"type":"frame","x":0,"y":0,"width":740,"height":1280},{"type":"text","x":40,"y":1310,"width":700,"height":220,"fontSize":16}]});
    let before = document.clone();
    let r = review(&document);
    let b = r.bounds.unwrap();
    assert_eq!(r.status, "available");
    assert_eq!((b.width, b.height), (740., 1530.));
    assert!(r.aspect_ratio.unwrap() < 0.5);
    assert_eq!(r.min_text_font_size, Some(16.));
    assert_eq!(document, before);
}
#[test]
fn negative_path_points_and_rotated_native_rectangles_contribute_real_extents() {
    let r = review(
        &json!({"elements":[{"type":"arrow","x":10,"y":20,"points":[[0,0],[-100,-60]],"roughness":0}]}),
    );
    let b = r.bounds.unwrap();
    assert_eq!((b.x, b.y, b.width, b.height), (-90., -40., 100., 60.));
    let r = review(
        &json!({"elements":[{"type":"text","x":0,"y":0,"width":100,"height":20,"angle":std::f64::consts::FRAC_PI_2,"fontSize":20}]}),
    );
    let b = r.bounds.unwrap();
    assert!((b.width - 20.).abs() < 1e-6 && (b.height - 100.).abs() < 1e-6);
}
#[test]
fn invalid_or_unsupported_geometry_never_claims_complete_coverage() {
    let r = review(
        &json!({"elements":[{"type":"rectangle","x":0,"y":0,"width":100,"height":20},{"type":"text","x":"0","y":0,"width":50,"height":20},{"type":"arrow","x":0,"y":0,"points":[[0,0],[10,20]],"roundness":{"type":2}},{"isDeleted":true,"x":9999}]}),
    );
    assert_eq!(r.status, "partial");
    assert_eq!(
        (
            r.checked_element_count,
            r.skipped_element_count,
            r.unknown_text_font_size_count
        ),
        (1, 2, 1)
    );
    assert_eq!(review(&json!({"elements":[]})).status, "empty");
    assert_eq!(
        review(&json!({"elements":[{"x":"bad"}]})).status,
        "unavailable"
    );
    let r = review(
        &json!({"elements":vec![json!({"type":"rectangle","x":0,"y":0,"width":1,"height":1});5001]}),
    );
    assert_eq!(r.status, "partial");
    assert_eq!(r.omitted_element_count, 1);
}
