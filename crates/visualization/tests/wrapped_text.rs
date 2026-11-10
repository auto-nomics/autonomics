use serde_json::{Value, json};
use visualization::{
    scene::{self, Operation},
    sketch::{self, Plan},
};
fn plan() -> Value {
    let plan:Plan=serde_json::from_value(json!({"message":"Text fixture","audience":"Testers","layout":"horizontal","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b","label":"relation","evidence":["fixture-source"]}]})).unwrap();
    sketch::compile(&plan).unwrap()
}
fn element<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn operation(id: &str, text: &str, max_width: Option<f64>) -> Operation {
    Operation::SetWrappedText {
        id: id.into(),
        text: text.into(),
        max_width,
        font_size: None,
    }
}
#[test]
fn bound_shape_text_wraps_without_changing_geometry_evidence_or_bindings() {
    for kind in ["rectangle", "ellipse", "diamond"] {
        let mut original = plan();
        let shape = original["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "node-0")
            .unwrap();
        shape["type"] = json!(kind);
        shape["width"] = json!(320);
        shape["height"] = json!(320);
        original["files"] = json!({"image":{"dataURL":"private-source-bytes"}});
        let source = "Receptor complex and downstream signal 细胞膜受体";
        let edited = scene::apply(&original, &[operation("label-0", source, None)]).unwrap();
        let label = element(&edited, "label-0");
        assert_eq!(label["originalText"], source);
        assert!(label["text"].as_str().unwrap().contains('\n'));
        assert_eq!(
            label["height"].as_f64().unwrap(),
            label["text"].as_str().unwrap().split('\n').count() as f64 * 25.
        );
        assert_eq!(label["containerId"], "node-0");
        assert_eq!(label["autoResize"], false);
        assert_eq!(
            label["strokeColor"],
            element(&original, "label-0")["strokeColor"]
        );
        for e in original["elements"].as_array().unwrap() {
            if e["id"] != "label-0" {
                assert_eq!(element(&edited, e["id"].as_str().unwrap()), e);
            }
        }
        assert_eq!(edited["files"], original["files"]);
        assert_eq!(edited["scientificPlan"], original["scientificPlan"]);
    }
}
#[test]
fn polyline_caption_and_free_text_keep_native_alignment_anchors() {
    let mut original = plan();
    let arrow = original["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == "edge-0")
        .unwrap();
    arrow["x"] = json!(10);
    arrow["y"] = json!(20);
    arrow["width"] = json!(120);
    arrow["height"] = json!(40);
    arrow["points"] = json!([[0, 0], [60, 40], [120, 0]]);
    let edited = scene::apply(
        &original,
        &[operation(
            "edge-label-0",
            "Explicit interaction and selected context",
            None,
        )],
    )
    .unwrap();
    let label = element(&edited, "edge-label-0");
    assert_eq!(
        label["x"].as_f64().unwrap() + label["width"].as_f64().unwrap() / 2.,
        70.
    );
    assert_eq!(
        label["y"].as_f64().unwrap() + label["height"].as_f64().unwrap() / 2.,
        60.
    );
    assert_eq!(element(&edited, "edge-0"), element(&original, "edge-0"));
    let free = json!({"elements":[{"id":"text","type":"text","text":"old","originalText":"old","fontFamily":9,"fontSize":20,"lineHeight":1.25,"angle":0,"x":10,"y":20,"width":200,"height":25,"textAlign":"right","verticalAlign":"top","containerId":null,"autoResize":true,"customData":{"private_note":"keep"}}]});
    let changed = scene::apply(
        &free,
        &[operation(
            "text",
            "Long unbound scientific caption",
            Some(120.),
        )],
    )
    .unwrap();
    let label = element(&changed, "text");
    assert_eq!(label["x"], 90.);
    assert_eq!(label["y"], 20.);
    assert_eq!(label["width"], 120.);
    assert!(label["height"].as_f64().unwrap() > 25.);
    assert_eq!(label["customData"], element(&free, "text")["customData"]);
}
#[test]
fn unsupported_or_nonfitting_labels_fail_atomically() {
    let original = plan();
    for changes in [
        json!({"fontFamily":1}),
        json!({"angle":0.2}),
        json!({"locked":true}),
        json!({"containerId":"missing"}),
    ] {
        let mut invalid = original.clone();
        invalid["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "label-0")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());
        let before = invalid.clone();
        assert!(scene::apply(&invalid, &[operation("label-0", "Scientific label", None)]).is_err());
        assert_eq!(invalid, before);
    }
    let overlong = "Long long long long ".repeat(50);
    for (text, width) in [
        ("", None),
        ("Unsupported 🚀", None),
        (overlong.as_str(), None),
        ("label", Some(90.)),
    ] {
        assert!(scene::apply(&original, &[operation("label-0", text, width)]).is_err());
    }
    let missing =
        serde_json::from_value::<Operation>(json!({"op":"set_wrapped_text","id":"label-0"}));
    assert!(missing.is_err());
}

#[test]
fn explicit_font_size_reflows_atomically_and_preserves_container() {
    let original = plan();
    let source = "Receptor complex and downstream signal";
    let edit = |size| Operation::SetWrappedText {
        id: "label-0".into(),
        text: source.into(),
        max_width: None,
        font_size: Some(size),
    };
    let changed = scene::apply(&original, &[edit(12.)]).unwrap();
    let label = element(&changed, "label-0");
    assert_eq!(label["fontSize"], 12.);
    assert_eq!(label["originalText"], source);
    assert_eq!(
        label["lineHeight"],
        element(&original, "label-0")["lineHeight"]
    );
    assert_eq!(
        label["height"].as_f64().unwrap(),
        label["text"].as_str().unwrap().split('\n').count() as f64 * 12. * 1.25
    );
    for e in original["elements"].as_array().unwrap() {
        if e["id"] != "label-0" {
            assert_eq!(element(&changed, e["id"].as_str().unwrap()), e);
        }
    }
    for size in [0., 3.9, 201., f64::NAN, f64::INFINITY] {
        assert!(scene::apply(&original, &[edit(size)]).is_err());
    }
    assert!(
        scene::apply(&original, &[edit(200.)]).is_err(),
        "large font must not silently expand container"
    );
    assert_eq!(element(&original, "label-0")["fontSize"], 20.);
}
