use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use visualization::{Store, image};

fn asset<'a>(file: &'a str, url: &'a str, width: u32, height: u32) -> image::Image<'a> {
    image::Image {
        file_id: file,
        data_url: url,
        width,
        height,
        provenance: json!({"source":"Synthetic metadata fixture"}),
    }
}
#[tokio::test]
async fn image_info_reads_exact_candidate_bytes_without_mutation_or_formal_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    store.create_project("other", "Other").unwrap();
    let formal_url=format!("data:image/svg+xml;base64,{}",STANDARD.encode(b"<svg xmlns='http://www.w3.org/2000/svg' width='12' height='8'><rect width='12' height='8' fill='red'/></svg>"));
    let candidate_url=format!("data:image/svg+xml;base64,{}",STANDARD.encode(b"<svg xmlns='http://www.w3.org/2000/svg' width='13' height='9'><rect width='13' height='9' fill='blue'/></svg>"));
    let (formal, target) = image::insert(
        &json!({"elements":[],"files":{}}),
        asset("formal-file", &formal_url, 12, 8),
        0.,
        0.,
    )
    .unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &formal)
        .unwrap();
    let other = store
        .create("p", "Other figure", "excalidraw/v2", &formal)
        .unwrap();
    let replaced = image::replace_file(
        &formal,
        &target,
        asset("candidate-file", &candidate_url, 13, 9),
    )
    .unwrap();
    let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
    let (document, only) =
        image::insert(&replaced, asset("candidate-only-file", png, 1, 1), 100., 0.).unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Candidate", &document)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = json!({"action":"image_info","project_id":"p","figure_id":figure.id,"expected_revision":1,"element_id":target});
    let formal_info: Value =
        serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    assert_eq!(formal_info["content"]["natural_width"], 12);
    let mut candidate = input.clone();
    candidate["proposal_id"] = json!(parent.id);
    candidate["document_sha256"] = json!(parent.document_sha256);
    let info: Value = serde_json::to_value(
        tool.implementation
            .execute(candidate.clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(info["content"]["natural_width"], 13);
    assert_eq!(info["content"]["natural_height"], 9);
    assert_ne!(info["content"]["sha256"], formal_info["content"]["sha256"]);
    assert_eq!(info["content"]["scope"], "proposal_candidate");
    assert_eq!(info["content"]["proposal_id"], parent.id);
    assert_eq!(info["content"]["document_sha256"], parent.document_sha256);
    assert_eq!(info["content"]["revision"], 1);
    assert_eq!(
        info["content"]["candidate_context"]["proposal_id"],
        parent.id
    );
    assert_eq!(
        info["content"]["follow_up_inputs"]["read_proposal"]["proposal_id"],
        parent.id
    );
    assert_eq!(
        info["content"]["follow_up_inputs"]["read_proposal"]["expected_revision"],
        1
    );
    assert!(!info.to_string().contains("data:image"));
    assert!(info["content"].get("dataURL").is_none());
    let mut only_formal = input.clone();
    only_formal["element_id"] = json!(only);
    assert!(
        tool.implementation
            .execute(only_formal.clone())
            .await
            .is_err()
    );
    only_formal["proposal_id"] = json!(parent.id);
    only_formal["document_sha256"] = json!(parent.document_sha256);
    let png_info: Value =
        serde_json::to_value(tool.implementation.execute(only_formal).await.unwrap()).unwrap();
    assert_eq!(png_info["content"]["natural_width"], 1);
    assert_eq!(png_info["content"]["natural_height"], 1);
    assert!(png_info["content"].get("dataURL").is_none());
    for (key, value) in [
        ("document_sha256", json!("wrong")),
        ("proposal_id", json!("missing")),
        ("project_id", json!("other")),
        ("figure_id", json!(other.id)),
        ("expected_revision", json!(2)),
        ("element_id", json!("missing")),
    ] {
        let mut bad = candidate.clone();
        bad[key] = value;
        assert!(tool.implementation.execute(bad).await.is_err(), "{key}");
    }
    for key in ["proposal_id", "document_sha256"] {
        let mut bad = candidate.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert!(tool.implementation.execute(bad).await.is_err());
    }
    let mut inset = candidate.clone();
    inset["action"] = json!("inset");
    assert!(tool.implementation.execute(inset).await.is_err());
    assert_eq!(store.revision("p", &figure.id, 1).unwrap().document, formal);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        document
    );
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let rejected = store
        .propose("p", &figure.id, 1, "Rejected snapshot", &document)
        .unwrap();
    store
        .reject_proposal("p", &figure.id, &rejected.id)
        .unwrap();
    let mut rejected_input = candidate.clone();
    rejected_input["proposal_id"] = json!(rejected.id);
    rejected_input["document_sha256"] = json!(rejected.document_sha256);
    assert!(
        tool.implementation.execute(rejected_input).await.is_err(),
        "rejected candidate at current head cannot be read as pending"
    );
    store
        .save("p", &figure.id, 1, "Advance formal head", &formal)
        .unwrap();
    candidate["expected_revision"] = json!(2);
    assert!(
        tool.implementation
            .execute(candidate.clone())
            .await
            .is_err(),
        "old candidate base rejected even with current head input"
    );
    store.reject_proposal("p", &figure.id, &parent.id).unwrap();
    candidate["expected_revision"] = json!(1);
    assert!(tool.implementation.execute(candidate).await.is_err());
}

#[tokio::test]
async fn local_asset_result_exposes_executable_candidate_image_info_arguments() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let url=format!("data:image/svg+xml;base64,{}",STANDARD.encode(b"<svg xmlns='http://www.w3.org/2000/svg' width='12' height='8'><rect width='12' height='8' fill='#ff0000'/></svg>"));
    let (document, target) = image::insert(
        &json!({"elements":[],"files":{}}),
        asset("original", &url, 12, 8),
        0.,
        0.,
    )
    .unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &document)
        .unwrap();
    let vector = visualization_tools::bioicons_registrations(path.clone()).remove(0);
    let created:Value=serde_json::to_value(vector.implementation.execute(json!({"action":"recolor","project_id":"p","figure_id":figure.id,"expected_revision":1,"element_id":target,"colors":{"#ff0000":"#0088cc"},"markup_ids":[]})).await.unwrap()).unwrap();
    let result = &created["content"];
    let args = &result["follow_up_inputs"]["image_info"];
    assert_eq!(
        args,
        &json!({"action":"image_info","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":result["proposal_id"],"document_sha256":result["document_sha256"],"element_id":target})
    );
    assert!(args.get("markup_ids").is_none());
    assert!(!result.to_string().contains("data:image"));
    let tool = visualization_tools::registrations(path).remove(0);
    let info: Value =
        serde_json::to_value(tool.implementation.execute(args.clone()).await.unwrap()).unwrap();
    assert_eq!(info["content"]["proposal_id"], result["proposal_id"]);
    assert_eq!(
        info["content"]["document_sha256"],
        result["document_sha256"]
    );
    assert_eq!(info["content"]["natural_width"], 12);
    assert_eq!(info["content"]["natural_height"], 8);
    assert!(info["content"].get("dataURL").is_none());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        document
    );
}
