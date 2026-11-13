use serde_json::{Value, json};
use visualization::Store;
#[tokio::test]
async fn scoped_and_full_tool_reads_report_all_text_and_do_not_mutate_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let figure = store
        .create(
            "p",
            "Figure",
            "excalidraw/v2",
            &json!({"elements":[],"files":{}}),
        )
        .unwrap();
    let document = json!({"elements":[{"id":"label","type":"text","x":10,"y":10,"width":100,"height":30,"angle":0,"text":"Protein"},{"id":"caption","type":"text","x":20,"y":20,"width":100,"height":30,"angle":0,"text":"Activation","containerId":"arrow"}],"files":{}});
    let parent = store
        .propose("p", &figure.id, 1, "Overlap review", &document)
        .unwrap();
    let tools = visualization_tools::registrations(path);
    let tool = &tools[0];
    for scoped in [false, true] {
        let mut input = json!({"action":"read_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"expected_revision":1});
        if scoped {
            input["element_ids"] = json!(["label"]);
        }
        let result: Value =
            serde_json::to_value(tool.implementation.execute(input).await.unwrap()).unwrap();
        let review = &result["content"]["scene_geometry_review"]["text_rectangle_overlap"];
        assert_eq!(review["status"], "complete");
        assert_eq!(review["checked_text_count"], 2);
        assert_eq!(review["overlap_count"], 1);
        assert_eq!(review["pairs"][0]["first"], "label");
        assert_eq!(review["pairs"][0]["second"], "caption");
        if scoped {
            assert_eq!(
                result["content"]["selected_elements"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
    }
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        document
    );
}
