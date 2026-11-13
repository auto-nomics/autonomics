use serde_json::{Value, json};
#[tokio::test]
async fn crop_tool_reads_real_dimensions_and_preserves_exact_candidate_scope() {
    for fit in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = visualization::Store::open(&path).unwrap();
        store.create_project("p", "Review").unwrap();
        let original: Value = serde_json::from_str(include_str!(
            "../../../apps/gui/tests/fixtures/scientific-ifng-art-before-clearance.json"
        ))
        .unwrap();
        let image = original["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "image")
            .unwrap();
        let id = image["id"].as_str().unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &original)
            .unwrap();
        let parent = store
            .propose("p", &figure.id, 1, "Parent", &original)
            .unwrap();
        let tool = visualization_tools::registrations(path.clone())
            .into_iter()
            .find(|t| t.definition.name == "visualization_crop_image")
            .unwrap();
        let input = json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Review crop","element_id":id,"file_id":image["fileId"],"x":0.25,"y":0.25,"width":0.5,"height":0.5,"fit_to_crop":fit});
        let mut wrong_file = input.clone();
        wrong_file["file_id"] = json!("stale-file");
        assert!(tool.implementation.execute(wrong_file).await.is_err());
        let result = tool.implementation.execute(input.clone()).await.unwrap();
        let result = serde_json::to_value(result).unwrap();
        let child = store
            .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
            .unwrap();
        let next = child.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap();
        let info = visualization_assets::image_info_document(&original, id).unwrap();
        assert_eq!(
            next["crop"]["naturalWidth"],
            info["natural_width"].as_f64().unwrap()
        );
        assert_eq!(
            next["crop"]["naturalHeight"],
            info["natural_height"].as_f64().unwrap()
        );
        assert_eq!(
            next["width"].as_f64().unwrap(),
            image["width"].as_f64().unwrap() * 0.5
        );
        assert_eq!(
            next["height"].as_f64().unwrap(),
            image["height"].as_f64().unwrap() * 0.5
        );
        let owner = image["customData"]["semanticAnchorId"].as_str().unwrap();
        let anchor = child.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == owner)
            .unwrap();
        assert_eq!(anchor["customData"].get("visualAnchorFit").is_some(), fit);
        if fit {
            assert_eq!(
                anchor["customData"]["visualAnchorFit"]["normalizedBounds"],
                json!([0.25, 0.25, 0.5, 0.5])
            );
        }
        assert_eq!(next["customData"], image["customData"]);
        assert_eq!(child.document["files"], original["files"]);
        assert_eq!(
            store
                .proposal("p", &figure.id, &parent.id)
                .unwrap()
                .document,
            parent.document
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        assert!(
            tool.implementation.execute(input).await.is_err(),
            "stale parent cannot create a second pending child"
        );
    }
}
