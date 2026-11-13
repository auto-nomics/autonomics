use serde_json::{Value, json};
fn original() -> Value {
    json!({"elements":[
 {"id":"frame","type":"frame","x":0,"y":0,"width":400,"height":400,"angle":0},
 {"id":"anchor","type":"rectangle","x":0,"y":0,"width":120,"height":80,"angle":0,"groupIds":["g"],"customData":{"visualAssetElementId":"image","visualLabelIds":["label"],"visualAssetGroupId":"g"}},
 {"id":"image","type":"image","x":0,"y":0,"width":40,"height":80,"angle":0,"groupIds":["g"],"fileId":"asset","customData":{"semanticAnchorId":"anchor"}},
 {"id":"label","type":"text","x":50,"y":20,"width":100,"height":25,"angle":0,"fontFamily":9,"fontSize":20,"lineHeight":1.25,"text":"Full receptor label","originalText":"Full receptor label","groupIds":["g"],"containerId":null,"frameId":"frame","customData":{"evidence":["retained"]}},
 {"id":"other-image","type":"image","x":200,"y":100,"width":100,"height":100,"angle":0,"fileId":"other"},
 {"id":"bound","type":"text","x":20,"y":20,"width":50,"height":25,"angle":0,"containerId":"anchor"},
 {"id":"locked","type":"text","x":20,"y":200,"width":50,"height":25,"angle":0,"locked":true}
 ],"files":{"asset":{"dataURL":"private-image-bytes"},"other":{"dataURL":"other-private-bytes"}}})
}
fn by_id<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[tokio::test]
async fn scalar_text_reflow_preserves_group_members_and_parent_with_exact_scope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = original();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Parent", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path)
        .into_iter()
        .find(|t| t.definition.name == "visualization_reflow_text")
        .unwrap();
    let schema = serde_json::to_value(&tool.definition.input_schema).unwrap();
    for field in ["max_width", "font_size"] {
        assert!(schema["properties"][field].to_string().contains("number"));
    }
    assert!(schema["properties"].get("operations").is_none());
    let input = json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Only label","element_id":"label","text":"Full receptor label","max_width":180,"markup_ids":[]});
    for (field, value) in [
        ("max_width", json!("180")),
        ("font_size", json!("20")),
        ("document_sha256", json!("wrong")),
        ("expected_revision", json!(0)),
        ("element_id", json!("image")),
        ("element_id", json!("bound")),
        ("element_id", json!("locked")),
        ("max_width", json!(0)),
        ("font_size", json!(0)),
    ] {
        let mut bad = input.clone();
        bad[field] = value;
        assert!(tool.implementation.execute(bad).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    }
    let result =
        serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    assert!(!result.to_string().contains("private-image-bytes"));
    let child = store
        .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(
        by_id(&child.document, "label")["width"].as_f64(),
        Some(180.)
    );
    assert_eq!(
        by_id(&child.document, "label")["originalText"],
        "Full receptor label"
    );
    let mut actual = by_id(&child.document, "label").clone();
    let mut expected = by_id(&original, "label").clone();
    for key in [
        "width",
        "height",
        "text",
        "autoResize",
        "version",
        "versionNonce",
        "updated",
    ] {
        actual.as_object_mut().unwrap().remove(key);
        expected.as_object_mut().unwrap().remove(key);
    }
    for key in ["x", "y", "fontSize"] {
        assert_eq!(actual[key].as_f64(), expected[key].as_f64());
        actual.as_object_mut().unwrap().remove(key);
        expected.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(actual, expected);
    for e in original["elements"].as_array().unwrap() {
        if e["id"] != "label" {
            assert_eq!(by_id(&child.document, e["id"].as_str().unwrap()), e);
        }
    }
    assert_eq!(child.document["files"], original["files"]);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        parent.document
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(
        result["content"]["follow_up_inputs"]["read_proposal"]["proposal_id"],
        child.id
    );
    assert!(result["content"].get("scene_geometry_review").is_some());
    // Reusing a successfully revised parent must not silently fork the chain.
    assert!(tool.implementation.execute(input).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 2);
}

#[tokio::test]
async fn formal_reflow_never_uses_unadopted_artwork_and_creates_no_seed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("formal.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = original();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let mut alternate = original.clone();
    alternate["elements"][3]["text"] = json!("Unadopted wording");
    alternate["elements"][3]["originalText"] = json!("Unadopted wording");
    alternate["elements"][3]["x"] = json!(200);
    let pending = store
        .propose("p", &figure.id, 1, "Unadopted", &alternate)
        .unwrap();
    let tool = visualization_tools::registrations(path)
        .into_iter()
        .find(|t| t.definition.name == "visualization_reflow_text")
        .unwrap();
    let schema = serde_json::to_value(&tool.definition.input_schema).unwrap();
    for field in ["proposal_id", "document_sha256"] {
        assert!(
            !schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(field))
        );
    }
    let request = json!({"project_id":"p","figure_id":figure.id,"expected_revision":1,"title":"Wrap formal label","element_id":"label","text":"Full receptor label","max_width":180,"markup_ids":[]});
    for patch in [
        json!({"proposal_id":pending.id}),
        json!({"document_sha256":pending.document_sha256}),
        json!({"proposal_id":"","document_sha256":""}),
        json!({"expected_revision":0}),
        json!({"max_width":"180"}),
        json!({"font_size":0}),
        json!({"element_id":"image"}),
    ] {
        let mut bad = request.clone();
        bad.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(tool.implementation.execute(bad).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    }
    let result = serde_json::to_value(tool.implementation.execute(request).await.unwrap()).unwrap();
    let child = store
        .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(child.parent_proposal_id, None);
    assert_eq!(child.status, "pending");
    let label = by_id(&child.document, "label");
    assert_eq!(label["originalText"], "Full receptor label");
    assert_eq!(label["x"].as_f64(), Some(50.));
    assert_eq!(label["width"].as_f64(), Some(180.));
    assert_eq!(label["fontSize"].as_f64(), Some(20.));
    let mut restored = child.document.clone();
    let actual = restored["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == "label")
        .unwrap();
    for key in [
        "x",
        "y",
        "width",
        "height",
        "text",
        "originalText",
        "fontSize",
        "autoResize",
        "version",
        "versionNonce",
        "updated",
    ] {
        if let Some(value) = by_id(&original, "label").get(key) {
            actual[key] = value.clone();
        } else {
            actual.as_object_mut().unwrap().remove(key);
        }
    }
    assert_eq!(restored, original);
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 2);
    assert_eq!(
        store
            .proposal("p", &figure.id, &pending.id)
            .unwrap()
            .document,
        alternate
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
}
