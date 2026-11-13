use serde_json::{Value, json};
use visualization::Store;
#[tokio::test]
async fn registered_candidate_reads_and_mutations_report_the_exact_scene_without_writing_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let formal = json!({"elements":[],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &formal)
        .unwrap();
    let document = json!({"elements":[{"id":"frame","type":"frame","x":0,"y":0,"width":740,"height":1280},{"id":"caption","type":"text","x":40,"y":1310,"width":700,"height":220,"fontSize":16,"text":"Caption"}],"files":{"asset":{"dataURL":"private-image-bytes"}}});
    let parent = store
        .propose("p", &figure.id, 1, "Parent", &document)
        .unwrap();
    let tools = visualization_tools::registrations(path);
    let tool = &tools[0];
    let read:Value=serde_json::to_value(tool.implementation.execute(json!({"action":"read_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"expected_revision":1})).await.unwrap()).unwrap();
    assert_eq!(
        read["content"]["scene_geometry_review"]["bounds"]["height"],
        1530.
    );
    assert_eq!(
        read["content"]["scene_geometry_review"]["status"],
        "available"
    );
    assert_eq!(
        read["content"]["proposal"]["document_sha256"],
        parent.document_sha256
    );
    assert!(!read.to_string().contains("private-image-bytes"));
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let child:Value=serde_json::to_value(tool.implementation.execute(json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Wider caption","operations":[{"op":"update","id":"caption","changes":{"width":1000}}]})).await.unwrap()).unwrap();
    assert_eq!(
        child["content"]["scene_geometry_review"]["bounds"]["width"],
        1040.
    );
    assert!(child["content"].get("document").is_none());
    assert_eq!(
        serde_json::to_value(store.proposal("p", &figure.id, &parent.id).unwrap()).unwrap(),
        serde_json::to_value(&parent).unwrap()
    );
    assert_eq!(store.revision("p", &figure.id, 1).unwrap().document, formal);
    let add = tools
        .iter()
        .find(|t| t.definition.name == "visualization_add_text")
        .unwrap();
    let result:Value=serde_json::to_value(add.implementation.execute(json!({"project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":child["content"]["id"],"document_sha256":child["content"]["document_sha256"],"title":"Add note","element_id":"extra-caption","text":"Additional caption","x":0,"y":1600,"max_width":1000,"font_size":16,"markup_ids":[]})).await.unwrap()).unwrap();
    assert!(
        result["content"]["scene_geometry_review"]["bounds"]["height"]
            .as_f64()
            .unwrap()
            > 1600.
    );
    assert!(!result.to_string().contains("private-image-bytes"));
}

#[tokio::test]
async fn registered_reads_and_candidate_writes_deliver_caption_path_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let formal = json!({"elements":[]});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &formal)
        .unwrap();
    let document = json!({"elements":[{"id":"edge","type":"arrow","x":0,"y":0,"angle":0,"points":[[0,0],[200,0]],"boundElements":[{"id":"label","type":"text"}]},{"id":"label","type":"text","containerId":"edge","x":10,"y":-10,"width":180,"height":20,"fontSize":16,"text":"Explicit relation"}]});
    let parent = store
        .propose("p", &figure.id, 1, "Parent", &document)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let read=serde_json::to_value(tool.implementation.execute(json!({"action":"read_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"expected_revision":1})).await.unwrap()).unwrap();
    let review = &read["content"]["scene_geometry_review"]["caption_path_overlap"];
    assert_eq!(review["checked_count"], 1);
    assert_eq!(review["skipped_count"], 0);
    assert_eq!(review["warnings"][0]["arrow_id"], "edge");
    assert_eq!(review["warnings"][0]["label_id"], "label");
    assert!(
        (review["warnings"][0]["covered_path_length"]
            .as_f64()
            .unwrap()
            - 180.)
            .abs()
            < 1e-6
    );
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let child=serde_json::to_value(tool.implementation.execute(json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Color relation","operations":[{"op":"update","id":"edge","changes":{"strokeColor":"#336699"}}]})).await.unwrap()).unwrap();
    assert_eq!(
        &child["content"]["scene_geometry_review"]["caption_path_overlap"],
        review
    );
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        document
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(store.revision("p", &figure.id, 1).unwrap().document, formal);
}
