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

#[test]
fn path_style_proposals_preserve_shared_instances_and_source_history() {
    let (mut store, figure, target, other) = fixture();
    let original = store.revision("p", &figure, 1).unwrap().document;
    let read = visualization_assets::parts(&store, "p", &figure, 1, "node-0").unwrap();
    let edit = bioicons::parts::Edit {
        part_id: read["parts"][0]["part_id"].as_str().unwrap().into(),
        fill: Some("#7744aa".into()),
        stroke: None,
        stroke_width: None,
        opacity: None,
        translate: Some([5., -4.]),
        points: None,
    };
    let sha = read["sha256"].as_str().unwrap();
    let proposal = visualization_assets::edit_parts(
        &store,
        "p",
        &figure,
        1,
        "node-0",
        sha,
        std::slice::from_ref(&edit),
    )
    .unwrap();
    assert_eq!(store.get("p", &figure).unwrap().revision, 1);
    for old in original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != target)
    {
        assert_eq!(
            element(&proposal.document, old["id"].as_str().unwrap()),
            old
        );
    }
    assert_eq!(
        element(&proposal.document, &other),
        element(&original, &other)
    );
    let after = element(&proposal.document, &target);
    for (key, value) in element(&original, &target).as_object().unwrap() {
        if !["fileId", "customData", "updated", "version", "versionNonce"].contains(&key.as_str()) {
            assert_eq!(&after[key], value);
        }
    }
    assert_eq!(
        after["customData"]["asset"],
        element(&original, &target)["customData"]["asset"]
    );
    assert_eq!(after["customData"]["user_note"], "keep this note");
    assert_eq!(
        after["customData"]["source_sha256"],
        "original-upstream-hash"
    );
    assert_eq!(after["customData"]["modifications"][0]["kind"], "path_edit");
    assert_eq!(
        after["customData"]["modifications"][0]["edits"][0]["part_id"],
        edit.part_id
    );
    assert!(
        visualization_assets::edit_parts(
            &store,
            "p",
            &figure,
            1,
            &target,
            "stale",
            std::slice::from_ref(&edit)
        )
        .is_err()
    );
    assert_eq!(store.proposals("p", &figure).unwrap().len(), 1);
    store.accept_proposal("p", &figure, &proposal.id).unwrap();
    assert_eq!(
        visualization_assets::parts(&store, "p", &figure, 2, &target).unwrap()["parts"][0]["fill"],
        "#7744aa"
    );
    let moved = visualization_assets::parts(&store, "p", &figure, 2, &target).unwrap();
    assert_eq!(moved["parts"][0]["bounds"][0], 5.);
    assert_eq!(moved["parts"][0]["bounds"][1], -4.);
    assert_eq!(
        after["customData"]["modifications"][0]["edits"][0]["translate"],
        json!([5., -4.])
    );
    assert_eq!(store.revision("p", &figure, 1).unwrap().document, original);
    assert!(visualization_assets::parts(&store, "p", &figure, 1, &target).is_err());
}

#[test]
fn point_edits_only_change_the_target_svg_and_remain_reviewable() {
    let (mut store, figure, target, other) = fixture();
    let original = store.revision("p", &figure, 1).unwrap().document;
    let list = visualization_assets::parts(&store, "p", &figure, 1, &target).unwrap();
    let sha = list["sha256"].as_str().unwrap();
    let part = list["parts"][0]["part_id"].as_str().unwrap();
    let geometry =
        visualization_assets::geometry(&store, "p", &figure, 1, "node-0", sha, part).unwrap();
    let edit = bioicons::parts::Edit {
        part_id: part.into(),
        fill: None,
        stroke: None,
        stroke_width: None,
        opacity: None,
        translate: None,
        points: Some(vec![bioicons::geometry::PointEdit {
            index: 0,
            position: [7., 9.],
        }]),
    };
    let proposal = visualization_assets::edit_parts(
        &store,
        "p",
        &figure,
        1,
        &target,
        sha,
        std::slice::from_ref(&edit),
    )
    .unwrap();
    assert_eq!(store.get("p", &figure).unwrap().revision, 1);
    for old in original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != target)
    {
        assert_eq!(
            element(&proposal.document, old["id"].as_str().unwrap()),
            old
        );
    }
    assert_eq!(
        element(&proposal.document, &other),
        element(&original, &other)
    );
    let after = element(&proposal.document, &target);
    assert_eq!(
        after["customData"]["asset"],
        element(&original, &target)["customData"]["asset"]
    );
    assert_eq!(after["customData"]["modifications"][0]["kind"], "path_edit");
    assert_eq!(
        after["customData"]["modifications"][0]["edits"][0]["points"][0]["position"],
        serde_json::json!([7., 9.])
    );
    store.accept_proposal("p", &figure, &proposal.id).unwrap();
    let next = visualization_assets::parts(&store, "p", &figure, 2, &target).unwrap();
    let changed = visualization_assets::geometry(
        &store,
        "p",
        &figure,
        2,
        &target,
        next["sha256"].as_str().unwrap(),
        next["parts"][0]["part_id"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        changed["points"][0]["position"],
        serde_json::json!([7., 9.])
    );
    for index in 1..geometry["points"].as_array().unwrap().len() {
        assert_eq!(changed["points"][index], geometry["points"][index]);
    }
    assert_eq!(store.revision("p", &figure, 1).unwrap().document, original);
    assert!(visualization_assets::geometry(&store, "p", &figure, 2, &target, sha, part).is_err());
}

#[test]
fn stroke_width_edit_creates_reviewable_child_and_preserves_shared_file_and_source() {
    let (store,figure,target,other)=fixture();
    let original=store.revision("p",&figure,1).unwrap().document;
    let read=visualization_assets::parts(&store,"p",&figure,1,"node-0").unwrap();
    let edit:bioicons::parts::Edit=serde_json::from_value(serde_json::json!({"part_id":read["parts"][0]["part_id"],"stroke":"#000000","stroke_width":3.})).unwrap();
    let proposal=visualization_assets::edit_parts(&store,"p",&figure,1,"node-0",read["sha256"].as_str().unwrap(),&[edit]).unwrap();
    assert_eq!(store.revision("p",&figure,1).unwrap().document,original);
    assert_eq!(element(&proposal.document,&other),element(&original,&other));
    let before=element(&original,&target);let after=element(&proposal.document,&target);
    assert_ne!(before["fileId"],after["fileId"]);
    assert_eq!(before["customData"]["asset"],after["customData"]["asset"]);
    assert_eq!(before["customData"]["source_sha256"],after["customData"]["source_sha256"]);
    for key in ["x","y","width","height","scale","frameId","groupIds"]{assert_eq!(before[key],after[key]);}
    for (id,file) in original["files"].as_object().unwrap(){assert_eq!(proposal.document["files"][id],*file);}
    assert_eq!(store.get("p",&figure).unwrap().revision,1);
}
