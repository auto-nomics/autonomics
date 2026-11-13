use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../apps/gui/tests/fixtures/scientific-ifng-art-before-clearance.json"
    ))
    .unwrap()
}
fn image(doc: &Value) -> &Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "image")
        .unwrap()
}
fn find<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[tokio::test]
async fn sampled_svg_fit_derives_real_bounds_preserves_parent_sources_and_supports_image_or_anchor()
{
    for select_image in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("figures.sqlite");
        let mut store = visualization::Store::open(&path).unwrap();
        store.create_project("p", "Review").unwrap();
        let original = fixture();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &original)
            .unwrap();
        let parent = store
            .propose("p", &figure.id, 1, "Parent", &original)
            .unwrap();
        let art = image(&original);
        let target = if select_image {
            art["id"].as_str().unwrap()
        } else {
            "node-0"
        };
        let info =
            visualization_assets::image_info_document(&original, art["id"].as_str().unwrap())
                .unwrap();
        let tool = visualization_tools::registrations(path.clone())
            .into_iter()
            .find(|t| t.definition.name == "visualization_fit_svg_preview")
            .unwrap();
        let input = json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Sampled SVG fit","element_id":target,"file_id":art["fileId"],"markup_ids":[]});
        let result =
            serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap())
                .unwrap();
        let content = &result["content"];
        assert_eq!(
            content["svg_preview_fit_basis"],
            info["svg_preview_alpha_bounds"]
        );
        assert!(content["svg_preview_fit_basis"].get("dataURL").is_none());
        let child = store
            .proposal("p", &figure.id, content["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            child.parent_proposal_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(child.document["files"], original["files"]);
        let bounds = &info["svg_preview_alpha_bounds"]["normalized_bounds"];
        assert_eq!(
            find(&child.document, "node-0")["customData"]["visualAnchorFit"]["normalizedBounds"],
            *bounds
        );
        for id in [
            "note_scope_heading",
            "note_scope_body",
            "generic-receptor-note",
        ] {
            assert_eq!(find(&child.document, id), find(&original, id));
        }
        for old in original["elements"].as_array().unwrap() {
            let actual = find(&child.document, old["id"].as_str().unwrap());
            for key in ["originalText", "text", "fontSize", "fontFamily", "frameId"] {
                assert_eq!(actual[key], old[key]);
            }
            for (key, value) in old["customData"].as_object().into_iter().flatten() {
                if !["visualAnchorFit", "visualAssetSlot"].contains(&key.as_str()) {
                    assert_eq!(&actual["customData"][key], value);
                }
            }
        }
        assert_eq!(
            store
                .proposal("p", &figure.id, &parent.id)
                .unwrap()
                .document,
            original
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        let count = store.proposals("p", &figure.id).unwrap().len();
        assert!(tool.implementation.execute(input.clone()).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), count);
        for (key, value) in [
            ("document_sha256", json!("wrong")),
            ("expected_revision", json!(2)),
            ("file_id", json!("wrong")),
            ("project_id", json!("other")),
        ] {
            let mut bad = input.clone();
            bad[key] = value;
            assert!(tool.implementation.execute(bad).await.is_err());
            assert_eq!(store.proposals("p", &figure.id).unwrap().len(), count);
        }
    }
}
#[tokio::test]
async fn cropped_rotated_empty_and_raster_art_reject_without_creating_a_candidate() {
    for variant in ["crop", "rotate", "empty", "raster"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("figures.sqlite");
        let mut store = visualization::Store::open(&path).unwrap();
        store.create_project("p", "Review").unwrap();
        let mut doc = fixture();
        let art_id = image(&doc)["id"].as_str().unwrap().to_owned();
        let file = image(&doc)["fileId"].as_str().unwrap().to_owned();
        match variant {
            "crop" => {
                doc["elements"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|e| e["id"] == art_id)
                    .unwrap()["crop"] =
                    json!({"x":0,"y":0,"width":1,"height":1,"naturalWidth":1,"naturalHeight":1})
            }
            "rotate" => {
                doc["elements"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|e| e["id"] == art_id)
                    .unwrap()["angle"] = json!(1.)
            }
            "empty" => {
                use base64::Engine;
                doc["files"][&file]["dataURL"] = json!(format!(
                    "data:image/svg+xml;base64,{}",
                    base64::engine::general_purpose::STANDARD
                        .encode("<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>")
                ));
            }
            "raster" => {
                doc["files"][&file]["dataURL"] = json!(
                    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aHn8AAAAASUVORK5CYII="
                )
            }
            _ => unreachable!(),
        }
        let figure = store.create("p", "Figure", "excalidraw/v2", &doc).unwrap();
        let parent = store.propose("p", &figure.id, 1, "Parent", &doc).unwrap();
        let tool = visualization_tools::registrations(path)
            .into_iter()
            .find(|t| t.definition.name == "visualization_fit_svg_preview")
            .unwrap();
        assert!(tool.implementation.execute(json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Must reject","element_id":art_id,"file_id":file})).await.is_err(),"{variant}");
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
        assert_eq!(
            store
                .proposal("p", &figure.id, &parent.id)
                .unwrap()
                .document,
            doc
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    }
}

#[tokio::test]
async fn sampled_fit_does_not_accept_silently_ignored_manual_bounds_or_string_review_links(){
    let directory=tempfile::tempdir().unwrap();let path=directory.path().join("absent.sqlite");
    let tool=visualization_tools::registrations(path.clone()).into_iter().find(|t|t.definition.name=="visualization_fit_svg_preview").unwrap();
    let valid=json!({"project_id":"p","figure_id":"f","proposal_id":"candidate","document_sha256":"hash","expected_revision":1,"title":"Fit","element_id":"anchor","file_id":"file"});
    for (key,value) in [("x",json!(0.2)),("normalized_bounds",json!([0.,0.,1.,1.])),("markup_ids",json!(""))]{let mut bad=valid.clone();bad[key]=value;assert!(tool.implementation.execute(bad).await.is_err());assert!(!path.exists());}
}
