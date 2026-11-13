use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
fn scene(svg: &str) -> (Value, String) {
    let url = format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg));
    visualization::image::insert(
        &json!({"elements":[]}),
        visualization::image::Image {
            file_id: "original-file-id",
            data_url: &url,
            width: 120,
            height: 100,
            provenance: json!({"source":"user fixture"}),
        },
        0.,
        0.,
    )
    .unwrap()
}
#[test]
fn svg_alpha_bounds_measure_paint_not_transparent_margins_and_never_write() {
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='120' height='100'><rect x='30' y='20' width='40' height='50' fill='red'/></svg>";
    let (doc, id) = scene(svg);
    let before = doc.clone();
    let info = visualization_assets::image_info_document(&doc, &id).unwrap();
    let bounds = &info["svg_preview_alpha_bounds"];
    assert_eq!(info["file_id"], "original-file-id");
    assert_eq!(
        info["sha256"],
        format!("{:x}", Sha256::digest(svg.as_bytes()))
    );
    assert_eq!(bounds["file_sha256"], info["sha256"]);
    assert_eq!(bounds["status"], "available");
    assert_eq!(bounds["pixel_bounds"], json!([30, 20, 40, 50]));
    assert_eq!(bounds["painted_pixel_count"], 2000);
    assert_eq!(
        bounds["normalized_bounds"],
        json!([29. / 120., 19. / 100., 42. / 120., 52. / 100.])
    );
    assert_eq!(bounds["padding_preview_pixels"], 1);
    assert_eq!(doc, before);
}
#[test]
fn clipped_preview_is_bounded_and_does_not_claim_native_crop_or_rotation() {
    let (mut doc, id) = scene(
        "<svg xmlns='http://www.w3.org/2000/svg' width='120' height='100'><defs><clipPath id='c'><rect x='40' y='30' width='20' height='20'/></clipPath></defs><rect width='120' height='100' fill='black' clip-path='url(#c)'/></svg>",
    );
    doc["elements"][0]["crop"] =
        json!({"x":20,"y":10,"width":80,"height":70,"naturalWidth":120,"naturalHeight":100});
    doc["elements"][0]["angle"] = json!(1.);
    doc["elements"][0]["scale"] = json!([-1, 1]);
    let before = doc.clone();
    let info = visualization_assets::image_info_document(&doc, &id).unwrap();
    let bounds = &info["svg_preview_alpha_bounds"];
    assert_eq!(bounds["pixel_bounds"], json!([40, 30, 20, 20]));
    assert_eq!(bounds["scope"], "uncropped_file_normalized_svg_preview");
    assert!(
        bounds["limitations"]
            .as_str()
            .unwrap()
            .contains("crop/rotation/flip/element opacity are not applied")
    );
    assert_eq!(info["visible_crop"], json!([20., 10., 80., 70.]));
    assert_eq!(doc, before);
}
#[test]
fn empty_svg_has_no_invented_fit_bounds() {
    let (doc, id) = scene(
        "<svg xmlns='http://www.w3.org/2000/svg' width='120' height='100'><rect width='120' height='100' fill='red' opacity='0'/></svg>",
    );
    let info = visualization_assets::image_info_document(&doc, &id).unwrap();
    let bounds = &info["svg_preview_alpha_bounds"];
    assert_eq!(bounds["status"], "empty");
    assert_eq!(bounds["normalized_bounds"], Value::Null);
    assert_eq!(bounds["painted_pixel_count"], 0);
}

#[test]
fn large_svg_preview_is_bounded_and_opaque_background_is_included() {
    let (doc, id) = scene(
        "<svg xmlns='http://www.w3.org/2000/svg' width='2400' height='1200'><rect width='2400' height='1200' fill='white'/><rect x='1000' y='500' width='100' height='100' fill='red'/></svg>",
    );
    let before = doc.clone();
    let info = visualization_assets::image_info_document(&doc, &id).unwrap();
    let bounds = &info["svg_preview_alpha_bounds"];
    assert_eq!(bounds["preview_width"], 768);
    assert_eq!(bounds["preview_height"], 384);
    assert_eq!(bounds["pixel_bounds"], json!([0, 0, 768, 384]));
    assert_eq!(bounds["normalized_bounds"], json!([0., 0., 1., 1.]));
    assert_eq!(bounds["painted_pixel_count"], 768 * 384);
    assert_eq!(doc, before);
}
