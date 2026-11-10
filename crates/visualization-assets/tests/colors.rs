use serde_json::{Value, json};
use std::collections::BTreeMap;
use visualization::{Store, image};
use visualization_assets::{palette, recolor};
fn fixture() -> (Store, String, String, String) {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Study").unwrap();
    store.create_project("other", "Other").unwrap();
    let plan=serde_json::from_value(json!({"message":"Conceptual example","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Receptor"},{"id":"b","label":"Response"}],"edges":[{"from":"a","to":"b","label":"explicit"}]})).unwrap();
    let scene = visualization::sketch::compile(&plan).unwrap();
    let svg=bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><path d='M0 0h100v100H0z' fill='#ff0000'/></svg>").unwrap();
    let asset = || image::Image {
        file_id: &svg.sha256,
        data_url: &svg.data_url,
        width: svg.width,
        height: svg.height,
        provenance: json!({"source":"BioIcons","asset":{"author":"Fixture Author","license":"cc-by-4.0","source_url":"https://example.org/asset"},"source_sha256":"original-upstream-hash","sha256":svg.sha256,"colors":svg.colors,"user_note":"keep this note"}),
    };
    let (scene, target) = image::illustrate(&scene, "node-0", asset()).unwrap();
    let (scene, other) = image::insert(&scene, asset(), 900., 300.).unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &scene)
        .unwrap();
    (store, figure.id, target, other)
}
fn element<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn colour_proposals_change_only_one_shared_file_instance_and_keep_history() {
    let (mut store, figure, target, other) = fixture();
    let original = store.revision("p", &figure, 1).unwrap().document;
    let colors = BTreeMap::from([("#ff0000".into(), "#00aa88".into())]);
    let p = palette(&store, "p", &figure, 1, "node-0").unwrap();
    assert_eq!(p["element_id"], target);
    assert_eq!(p["colors"], json!(["#ff0000"]));
    let proposal = recolor(&store, "p", &figure, 1, "node-0", &colors).unwrap();
    assert_eq!(store.get("p", &figure).unwrap().revision, 1);
    let next = &proposal.document;
    let before = element(&original, &target);
    let after = element(next, &target);
    for old in original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != target)
    {
        assert_eq!(element(next, old["id"].as_str().unwrap()), old);
    }
    for (key, value) in before.as_object().unwrap() {
        if !["fileId", "customData", "updated", "version", "versionNonce"].contains(&key.as_str()) {
            assert_eq!(&after[key], value);
        }
    }
    for (id, file) in original["files"].as_object().unwrap() {
        assert_eq!(&next["files"][id], file);
    }
    assert_ne!(before["fileId"], after["fileId"]);
    assert_eq!(element(next, &other)["fileId"], before["fileId"]);
    assert_eq!(
        after["customData"]["source_sha256"],
        "original-upstream-hash"
    );
    assert_eq!(after["customData"]["asset"], before["customData"]["asset"]);
    assert_eq!(after["customData"]["user_note"], "keep this note");
    assert_eq!(
        after["customData"]["modifications"][0]["based_on_file_id"],
        before["fileId"]
    );
    assert_eq!(
        after["customData"]["modifications"][0]["based_on_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    store.accept_proposal("p", &figure, &proposal.id).unwrap();
    assert_eq!(store.revision("p", &figure, 1).unwrap().document, original);
    assert_eq!(
        palette(&store, "p", &figure, 2, &other).unwrap()["colors"],
        json!(["#ff0000"])
    );
    assert_eq!(
        palette(&store, "p", &figure, 2, &target).unwrap()["colors"],
        json!(["#00aa88"])
    );
}
#[test]
fn stale_invalid_and_wrong_project_requests_do_not_create_proposals() {
    let (store, figure, target, _) = fixture();
    let colors = BTreeMap::from([("#123456".into(), "#000000".into())]);
    assert!(recolor(&store, "p", &figure, 1, &target, &colors).is_err());
    assert!(recolor(&store, "p", &figure, 2, &target, &colors).is_err());
    assert!(palette(&store, "other", &figure, 1, &target).is_err());
    assert!(store.proposals("p", &figure).unwrap().is_empty());
    assert_eq!(store.get("p", &figure).unwrap().revision, 1);
}
