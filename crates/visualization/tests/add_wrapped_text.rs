use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn add(id: &str, text: &str, width: f64) -> Operation {
    Operation::AddWrappedText {
        id: id.into(),
        text: text.into(),
        x: 12.,
        y: 24.,
        max_width: width,
        font_size: Some(16.),
        frame_id: None,
    }
}
#[test]
fn adds_complete_pinned_text_and_keeps_unrelated_objects_files_and_scientific_metadata() {
    let original = json!({"elements":[{"id":"source","type":"rectangle","x":300,"y":100,"width":50,"height":50,"customData":{"evidence":["verified-source"]}}],"files":{"unused":{"dataURL":"immutable bytes"}}});
    let next = scene::apply(
        &original,
        &[add("scope", "Human EGFR-MAPK scope\nα β γ 中文", 180.)],
    )
    .unwrap();
    assert_eq!(next["elements"][0], original["elements"][0]);
    assert_eq!(next["files"], original["files"]);
    let note = &next["elements"][1];
    assert_eq!(note["fontFamily"], 9);
    assert_eq!(note["fontSize"], 16.);
    assert_eq!(note["originalText"], "Human EGFR-MAPK scope\nα β γ 中文");
    assert_eq!(note["width"], 180.);
    assert_eq!(
        note["height"].as_f64().unwrap(),
        note["text"].as_str().unwrap().lines().count() as f64 * 20.
    );
    for key in [
        "x",
        "y",
        "width",
        "height",
        "fontFamily",
        "fontSize",
        "lineHeight",
        "roughness",
        "opacity",
        "version",
        "seed",
    ] {
        assert!(note[key].is_number(), "{key}");
    }
    assert_eq!(note["groupIds"], json!([]));
    assert_eq!(note["boundElements"], json!([]));
    assert_eq!(note["textAlign"], "left");
    assert_eq!(note["frameId"], Value::Null);
    assert_eq!(note["containerId"], Value::Null);
    let changed = scene::apply(
        &original,
        &[
            add("scope", "First caption", 180.),
            Operation::SetWrappedText {
                id: "scope".into(),
                text: "Revised caption".into(),
                max_width: None,
                font_size: Some(18.),
            },
        ],
    )
    .unwrap();
    assert_eq!(changed["elements"][1]["fontSize"], 18.);
    assert_eq!(changed["elements"][1]["originalText"], "Revised caption");
}
#[test]
fn frame_membership_requires_a_live_unlocked_unrotated_frame_that_contains_the_text() {
    let original = json!({"elements":[{"id":"frame","type":"frame","x":0,"y":0,"width":240,"height":160,"angle":0}],"files":{}});
    let mut note = add("inside", "Caption", 160.);
    if let Operation::AddWrappedText { frame_id, .. } = &mut note {
        *frame_id = Some("frame".into());
    }
    let valid = scene::apply(&original, &[note.clone()]).unwrap();
    assert_eq!(valid["elements"][1]["frameId"], "frame");
    assert_eq!(valid["elements"][0], original["elements"][0]);
    for patch in [
        json!({"locked":true}),
        json!({"isDeleted":true}),
        json!({"angle":0.1}),
        json!({"width":20}),
    ] {
        let mut invalid = original.clone();
        invalid["elements"][0]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(scene::apply(&invalid, &[note.clone()]).is_err());
    }
}
#[test]
fn malformed_ids_geometry_glyphs_or_collisions_reject_without_partial_writes() {
    let original = json!({"elements":[],"files":{}});
    for operation in [
        add("", "Caption", 100.),
        add("too-small", "W", 1.),
        add("unsupported", "🧬", 100.),
        add("blank", " ", 100.),
        Operation::AddWrappedText {
            id: "position".into(),
            text: "Caption".into(),
            x: 1e6 + 1.,
            y: 1e6 + 1.,
            max_width: 100.,
            font_size: Some(16.),
            frame_id: None,
        },
        Operation::AddWrappedText {
            id: "size".into(),
            text: "Caption".into(),
            x: 0.,
            y: 0.,
            max_width: 100.,
            font_size: Some(201.),
            frame_id: None,
        },
    ] {
        assert!(scene::apply(&original, &[add("first", "Valid", 100.), operation]).is_err());
        assert_eq!(original["elements"], json!([]));
    }
    assert!(
        scene::apply(
            &original,
            &[add("same", "One", 100.), add("same", "Two", 100.)]
        )
        .is_err()
    );
    assert!(serde_json::from_value::<Operation>(json!({"op":"add_wrapped_text","id":"strings","text":"Caption","x":"0","y":0,"max_width":100})).is_err(),"never silently coerce opaque string coordinates");
}
