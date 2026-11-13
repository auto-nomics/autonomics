use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn add(id: &str, text: &str, width: f64) -> Operation {
    Operation::AddWrappedText {
        id: id.into(),
        text: text.into(),
        x: 12.,
        y: 24.,
        max_width: width,
        fit_width: false,
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
            fit_width: false,
            font_size: Some(16.),
            frame_id: None,
        },
        Operation::AddWrappedText {
            id: "size".into(),
            text: "Caption".into(),
            x: 0.,
            y: 0.,
            max_width: 100.,
            fit_width: false,
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

#[test]
fn enclosed_figure_markers_are_measured_and_reflowed_without_substitution() {
    let original = json!({"elements":[],"files":{}});
    let text = "① 范围：人类 IFN-γ。② JAK 归属未确定。③ 仅示意。⑳ ⓐ ⓩ";
    let next = scene::apply(&original, &[add("scope", text, 180.)]).unwrap();
    assert_eq!(next["elements"][0]["originalText"], text);
    assert_eq!(next["elements"][0]["fontFamily"], 9);
    assert!(next["elements"][0]["height"].as_f64().unwrap() > 20.);
    let revised = "① 核对来源。② 保留不确定性。";
    let changed = scene::apply(
        &next,
        &[Operation::SetWrappedText {
            id: "scope".into(),
            text: revised.into(),
            max_width: None,
            font_size: None,
        }],
    )
    .unwrap();
    assert_eq!(changed["elements"][0]["originalText"], revised);
    assert_eq!(
        changed["elements"][0]["width"],
        next["elements"][0]["width"]
    );
    assert_eq!(changed["files"], next["files"]);
    let invalid = scene::apply(
        &changed,
        &[Operation::SetWrappedText {
            id: "scope".into(),
            text: "Unsupported emoji 🧬".into(),
            max_width: None,
            font_size: None,
        }],
    );
    assert!(
        invalid.is_err(),
        "unsupported glyphs still reject; no system-font fallback or text replacement"
    );
}

#[test]
fn explicit_compact_width_only_shrinks_the_box_and_preserves_complete_wrapped_text() {
    let original = json!({"elements":[{"id":"kept","type":"rectangle","x":0,"y":0,"width":50,"height":50,"customData":{"evidence":["retained"]}}],"files":{"source":{"dataURL":"unchanged"}}});
    for (source, budget) in [
        (
            "Artwork is a generic membrane receptor illustration, not an IFNGR-specific molecular structure.",
            1180.,
        ),
        (
            "人类 IFN-γ；激酶归属仍不确定。\nPreserve Y701 and the complete source wording.",
            220.,
        ),
    ] {
        let legacy = scene::apply(&original, &[add("note", source, budget)]).unwrap();
        let mut operation = add("note", source, budget);
        if let Operation::AddWrappedText { fit_width, .. } = &mut operation {
            *fit_width = true;
        }
        let compact = scene::apply(&original, &[operation]).unwrap();
        let old = &legacy["elements"][1];
        let new = &compact["elements"][1];
        assert!(new["width"].as_f64().unwrap() < budget);
        assert!(new["width"].as_f64().unwrap() > 1.);
        for key in [
            "text",
            "originalText",
            "x",
            "y",
            "height",
            "fontFamily",
            "fontSize",
            "lineHeight",
            "textAlign",
            "verticalAlign",
            "autoResize",
            "containerId",
            "frameId",
        ] {
            assert_eq!(new[key], old[key], "{key}");
        }
        assert_eq!(compact["elements"][0], original["elements"][0]);
        assert_eq!(compact["files"], original["files"]);
    }
    assert!(serde_json::from_value::<Operation>(json!({"op":"add_wrapped_text","id":"invalid","text":"Note","x":0,"y":0,"max_width":100,"fit_width":"true"})).is_err());
}
