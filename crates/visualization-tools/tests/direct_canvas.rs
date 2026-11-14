use serde_json::{Value, json};
use visualization::{Store, sketch};

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Store, String, Value) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("canvas.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let plan:sketch::Plan=serde_json::from_value(json!({"message":"Explicit relation","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Source"},{"id":"b","label":"Target"}],"edges":[{"from":"a","to":"b","label":"activates"}]})).unwrap();
    let scene = sketch::compile(&plan).unwrap();
    let record = store
        .create("p", "Figure", "excalidraw/v2", &scene)
        .unwrap();
    (dir, path, store, record.id, scene)
}
async fn call(path: &std::path::Path, name: &str, input: Value) -> Result<Value, String> {
    let tool = visualization_tools::registrations(path.into())
        .into_iter()
        .chain(visualization_tools::bioicons_registrations(path.into()))
        .find(|t| t.definition.name == name)
        .unwrap();
    let result = tool
        .implementation
        .execute(input)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(result).unwrap()["content"].clone())
}
fn element<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[tokio::test]
async fn edits_commit_immediately_compose_by_revision_and_preserve_history() {
    let (_dir, path, store, id, original) = fixture();
    let first=call(&path,"visualization",json!({"action":"apply","project_id":"p","figure_id":id,"expected_revision":1,"operations":[{"op":"set_wrapped_text","id":"label-0","text":"New source"}]})).await.unwrap();
    assert_eq!(first["revision"], 2);
    assert_eq!(store.get("p", &id).unwrap().revision, 2);
    assert_eq!(
        element(&store.revision("p", &id, 2).unwrap().document, "label-0")["originalText"],
        "New source"
    );
    let second=call(&path,"visualization_reflow_text",json!({"project_id":"p","figure_id":id,"expected_revision":2,"element_id":"label-1","text":"New target"})).await.unwrap();
    assert_eq!(second["revision"], 3);
    let current = store.revision("p", &id, 3).unwrap().document;
    assert_eq!(element(&current, "label-0")["originalText"], "New source");
    assert_eq!(element(&current, "label-1")["originalText"], "New target");
    assert_eq!(store.revision("p", &id, 1).unwrap().document, original);
    let read = call(
        &path,
        "visualization",
        second["follow_up_inputs"]["read"].clone(),
    )
    .await
    .unwrap();
    assert_eq!(read["metadata"]["revision"], 3);
    assert!(read.get("proposal_summary").is_none());
}
#[tokio::test]
async fn stale_writers_and_failed_batches_never_partially_save() {
    let (_dir, path, store, id, original) = fixture();
    let request = |ops| json!({"action":"apply","project_id":"p","figure_id":id,"expected_revision":1,"operations":ops});
    assert!(call(&path,"visualization",request(json!([{"op":"set_wrapped_text","id":"label-0","text":"Changed"},{"op":"remove","id":"missing"}]))).await.is_err());
    assert_eq!(store.get("p", &id).unwrap().revision, 1);
    assert_eq!(store.revision("p", &id, 1).unwrap().document, original);
    let edit = request(json!([{"op":"set_wrapped_text","id":"label-0","text":"Changed"}]));
    let (a, b) = tokio::join!(
        call(&path, "visualization", edit.clone()),
        call(&path, "visualization", edit)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(store.get("p", &id).unwrap().revision, 2);
    assert!(call(&path,"visualization",json!({"action":"apply","project_id":"other","figure_id":id,"expected_revision":2,"operations":[]})).await.is_err());
}
#[tokio::test]
async fn typed_text_translation_and_scoped_reads_use_live_canvas() {
    let (_dir, path, store, id, _) = fixture();
    let added=call(&path,"visualization_add_text",json!({"project_id":"p","figure_id":id,"expected_revision":1,"element_id":"note","text":"Scope note","x":10,"y":600,"max_width":250,"font_size":18})).await.unwrap();
    assert_eq!(added["revision"], 2);
    call(&path,"visualization_move_text",json!({"project_id":"p","figure_id":id,"expected_revision":2,"element_id":"note","dx":25,"dy":10})).await.unwrap();
    let read=call(&path,"visualization",json!({"action":"read","project_id":"p","figure_id":id,"expected_revision":3,"element_ids":["note"]})).await.unwrap();
    assert!(!read.to_string().contains("data:image"));
    let doc = store.revision("p", &id, 3).unwrap().document;
    assert_eq!(element(&doc, "note")["x"], 35.);
    assert_eq!(element(&doc, "note")["y"], 610.);
    assert_eq!(element(&doc, "note")["originalText"], "Scope note");
    call(&path,"visualization_translate",json!({"project_id":"p","figure_id":id,"expected_revision":3,"element_ids":["note"],"dx":10,"dy":0})).await.unwrap();
    assert_eq!(store.get("p", &id).unwrap().revision, 4);
}
#[tokio::test]
async fn node_geometry_updates_connectors_and_preserves_other_nodes() {
    let (_dir, path, store, id, original) = fixture();
    let input = |op| json!({"action":"apply","project_id":"p","figure_id":id,"expected_revision":1,"operations":[op]});
    let op = json!({"op":"set_node_geometry","id":"node-0","x":50,"y":45,"width":250,"height":120});
    let mut bad = op.clone();
    bad["width"] = json!("250");
    assert!(
        call(&path, "visualization", input(bad))
            .await
            .unwrap_err()
            .contains("operations[0].width")
    );
    call(&path, "visualization", input(op)).await.unwrap();
    let next = store.revision("p", &id, 2).unwrap().document;
    assert_eq!(element(&next, "node-1"), element(&original, "node-1"));
    assert_eq!(
        element(&next, "edge-0")["startBinding"]["elementId"],
        "node-0"
    );
    assert_eq!(next["scientificPlan"], original["scientificPlan"]);
}
#[tokio::test]
async fn svg_recolor_commits_preserving_other_instances_and_sources() {
    let (_dir, path, store, id, original) = fixture();
    let svg=bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><path d='M0 0h100v100H0z' fill='#ff0000'/></svg>").unwrap();
    let image = || visualization::image::Image {
        file_id: &svg.sha256,
        data_url: &svg.data_url,
        width: 100,
        height: 100,
        provenance: json!({"source":"fixture","asset":{"author":"Artist","license":"cc0"}}),
    };
    let (doc, target) = visualization::image::insert(&original, image(), 0., 500.).unwrap();
    let (doc, other) = visualization::image::insert(&doc, image(), 200., 500.).unwrap();
    store.save("p", &id, 1, "Figure", &doc).unwrap();
    let result=call(&path,"bioicons",json!({"action":"recolor","project_id":"p","figure_id":id,"expected_revision":2,"element_id":target,"colors":{"#ff0000":"#00aa88"}})).await.unwrap();
    assert_eq!(result["revision"], 3);
    let next = store.revision("p", &id, 3).unwrap().document;
    assert_eq!(element(&next, &other), element(&doc, &other));
    assert_eq!(
        element(&next, &target)["customData"]["asset"]["author"],
        "Artist"
    );
    assert_ne!(
        element(&next, &target)["fileId"],
        element(&doc, &target)["fileId"]
    );
    assert_eq!(store.revision("p", &id, 2).unwrap().document, doc);
}
#[tokio::test]
async fn removed_protocol_is_rejected_and_schemas_do_not_advertise_it() {
    let (_dir, path, store, id, _) = fixture();
    for tool in visualization_tools::registrations(path.clone())
        .into_iter()
        .chain(visualization_tools::bioart_registrations(path.clone()))
        .chain(visualization_tools::bioicons_registrations(path.clone()))
    {
        let schema = serde_json::to_string(&tool.definition).unwrap();
        for retired in [
            "proposal_id",
            "document_sha256",
            "revise_proposal",
            "allow_branch",
        ] {
            assert!(!schema.contains(retired), "{retired}");
        }
    }
    assert!(call(&path,"visualization",json!({"action":"propose","project_id":"p","figure_id":id,"expected_revision":1,"operations":[]})).await.is_err());
    assert!(call(&path,"visualization",json!({"action":"apply","project_id":"p","figure_id":id,"expected_revision":1,"proposal_id":"old","operations":[]})).await.is_err());
    assert_eq!(store.get("p", &id).unwrap().revision, 1);
}

#[tokio::test]
async fn svg_fit_scale_crop_and_preview_use_committed_revision() {
    let (_dir, path, store, id, original) = fixture();
    let svg=bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><path d='M10 10h80v80H10z' fill='#ff0000'/></svg>").unwrap();
    let (doc, image_id) = visualization::image::illustrate(
        &original,
        "node-0",
        visualization::image::Image {
            file_id: &svg.sha256,
            data_url: &svg.data_url,
            width: 100,
            height: 100,
            provenance: json!({"source":"fixture","asset":{"license":"cc0"}}),
        },
    )
    .unwrap();
    store.save("p", &id, 1, "Figure", &doc).unwrap();
    let fit=call(&path,"visualization_fit_svg_preview",json!({"project_id":"p","figure_id":id,"expected_revision":2,"element_id":image_id,"file_id":svg.sha256})).await.unwrap();
    assert_eq!(fit["revision"], 3);
    assert_eq!(fit["svg_preview_fit_basis"]["status"], "available");
    call(&path,"visualization_scale_illustration",json!({"project_id":"p","figure_id":id,"expected_revision":3,"element_id":"node-0","file_id":svg.sha256,"factor":0.9})).await.unwrap();
    let crop=call(&path,"visualization_crop_image",json!({"project_id":"p","figure_id":id,"expected_revision":4,"element_id":image_id,"file_id":svg.sha256,"x":0.1,"y":0.1,"width":0.8,"height":0.8,"fit_to_crop":true})).await.unwrap();
    assert_eq!(crop["revision"], 5);
    assert_eq!(
        store.revision("p", &id, 5).unwrap().document["files"],
        doc["files"]
    );
    const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
    store.save_preview("p", &id, 5, PNG).unwrap();
    let image = call(
        &path,
        "visualization",
        json!({"action":"read_preview","project_id":"p","figure_id":id,"expected_revision":5}),
    )
    .await
    .unwrap();
    assert_eq!(image[1]["source"]["media_type"], "image/png");
    assert!(
        call(
            &path,
            "visualization",
            json!({"action":"read_preview","project_id":"p","figure_id":id,"expected_revision":4})
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn sketch_appends_without_replacing_existing_content_and_whole_replacement_is_explicit() {
    let (_dir, path, store, id, original) = fixture();
    let plan = json!({"message":"Added structure","audience":"Researchers","layout":"horizontal","nodes":[{"id":"c","label":"Extra"}]});
    assert!(call(&path,"visualization",json!({"action":"sketch","project_id":"p","figure_id":id,"expected_revision":1,"title":"Replacement","document":plan})).await.is_err());
    call(&path,"visualization",json!({"action":"apply","project_id":"p","figure_id":id,"expected_revision":1,"operations":[{"op":"insert_sketch","plan":plan,"id_prefix":"extra","x":0,"y":600}]})).await.unwrap();
    let next = store.revision("p", &id, 2).unwrap().document;
    for old in original["elements"].as_array().unwrap() {
        assert_eq!(element(&next, old["id"].as_str().unwrap()), old);
    }
    let replaced=call(&path,"visualization",json!({"action":"sketch","project_id":"p","figure_id":id,"expected_revision":2,"title":"Replacement","document":plan,"replace_existing":true})).await.unwrap();
    assert_eq!(replaced["revision"], 3);
    assert_eq!(store.revision("p", &id, 1).unwrap().document, original);
}
