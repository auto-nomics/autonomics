use serde_json::{Value, json};
use visualization::scene::{self, IllustrationMember, Operation};
fn fixture() -> Value {
    let a=bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><rect x='10' y='20' width='60' height='50' fill='#112233'/></svg>").unwrap();
    let b=bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><circle cx='50' cy='50' r='20' fill='#445566'/></svg>").unwrap();
    let mut d = json!({"elements":[{"id":"frame","type":"frame","x":0.,"y":0.,"width":300.,"height":300.,"angle":0.},{"id":"anchor","type":"rectangle","x":60.,"y":80.,"width":100.,"height":100.,"angle":0.,"frameId":"frame","boundElements":[{"id":"label","type":"text"}],"customData":{"scientificId":"complex","evidence":["preserved"]}},{"id":"label","type":"text","x":60.,"y":220.,"width":160.,"height":20.,"angle":0.,"frameId":"frame","containerId":"anchor","text":"Complete scientific label","originalText":"Complete scientific label","fontSize":16}],"files":{}});
    for (id, v, x, y, size) in [
        ("primary", a, 60., 80., 100.),
        ("badge", b, 130., 100., 20.),
    ] {
        let (mut next,generated)=visualization::image::insert(&d,visualization::image::Image{file_id:&v.sha256,data_url:&v.data_url,width:v.width,height:v.height,provenance:json!({"source":"Fixture","sha256":v.sha256,"asset":{"author":"Artist","license":"CC0"}})},x,y).unwrap();
        let image = next["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == generated)
            .unwrap();
        image["id"] = json!(id);
        image["frameId"] = json!("frame");
        image["width"] = json!(size);
        image["height"] = json!(size);
        d = next;
    }
    scene::apply(
        &d,
        &[Operation::BindIllustration {
            anchor_id: "anchor".into(),
            primary_id: "primary".into(),
            label_ids: vec!["label".into()],
            members: vec![IllustrationMember {
                id: "badge".into(),
                meaning: "Generic accessory".into(),
            }],
        }],
    )
    .unwrap()
}
#[tokio::test]
async fn group_preview_tool_measures_every_distinct_file_and_saves_only_one_protected_child() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Review").unwrap();
    let doc = fixture();
    let figure = store.create("p", "Figure", "excalidraw/v2", &doc).unwrap();
    let parent = store.propose("p", &figure.id, 1, "Parent", &doc).unwrap();
    let tool = visualization_tools::registrations(path)
        .into_iter()
        .find(|t| t.definition.name == "visualization_fit_svg_group_preview")
        .unwrap();
    let input = json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Fit the complete group","element_id":"badge"});
    let result =
        serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    let value = &result["content"];
    let child = store
        .proposal("p", &figure.id, value["id"].as_str().unwrap())
        .unwrap();
    let anchor = child.document["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "anchor")
        .unwrap();
    assert_eq!(
        anchor["customData"]["visualAnchorFit"]["scope"],
        "illustration_group"
    );
    assert_eq!(
        anchor["customData"]["visualAnchorFit"]["members"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let basis = value["svg_group_preview_fit_basis"].as_array().unwrap();
    assert_eq!(basis.len(), 2);
    assert_ne!(basis[0]["file_id"], basis[1]["file_id"]);
    for item in basis {
        let id = item["element_id"].as_str().unwrap();
        let info = visualization_assets::image_info_document(&doc, id).unwrap();
        assert_eq!(item["bounds"], info["svg_preview_alpha_bounds"]);
        assert!(item["bounds"].get("dataURL").is_none());
    }
    for old in doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != "anchor")
    {
        assert_eq!(
            child.document["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == old["id"])
                .unwrap(),
            old
        );
    }
    assert_eq!(child.document["files"], doc["files"]);
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        doc
    );
    assert!(tool.implementation.execute(input).await.is_err());
}

#[tokio::test]
async fn one_unsupported_member_rejects_the_entire_group_without_a_partial_child() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Review").unwrap();
    let mut doc = fixture();
    doc["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == "badge")
        .unwrap()["crop"] =
        json!({"x":0,"y":0,"width":50,"height":100,"naturalWidth":100,"naturalHeight":100});
    let figure = store.create("p", "Figure", "excalidraw/v2", &doc).unwrap();
    let parent = store.propose("p", &figure.id, 1, "Parent", &doc).unwrap();
    let tool = visualization_tools::registrations(path)
        .into_iter()
        .find(|t| t.definition.name == "visualization_fit_svg_group_preview")
        .unwrap();
    let error=tool.implementation.execute(json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Group fit","element_id":"anchor"})).await.unwrap_err().to_string();
    assert!(error.contains("uncropped"), "{error}");
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
