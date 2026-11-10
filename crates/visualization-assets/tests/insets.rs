use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use visualization::{
    Store, image,
    inset::{Region, Spec},
};

#[test]
fn rotated_flipped_crop_is_a_reviewable_immutable_vector_snapshot() {
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg' width='400' height='200'><path d='M0 0h400v200H0z' fill='red'/></svg>";
    let url = format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg));
    let (mut scene, target) = image::insert(&json!({"elements":[]}), image::Image { file_id:"shared", data_url:&url, width:400, height:200, provenance:json!({"source":"fixture","asset":{"license":"cc0"},"user_note":"preserve"}) }, 20.,30.).unwrap();
    let original = &mut scene["elements"][0];
    original["width"] = json!(240.);
    original["height"] = json!(120.);
    original["angle"] = json!(std::f64::consts::FRAC_PI_2);
    original["scale"] = json!([-1.0, 1.0]);
    original["crop"] =
        json!({"x":100,"y":20,"width":200,"height":100,"naturalWidth":400,"naturalHeight":200});
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Study").unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &scene)
        .unwrap();
    let mut spec = Spec {
        element_id: target,
        region: Region {
            x: 0.1,
            y: 0.1,
            width: 0.4,
            height: 0.6,
        },
        x: 800.,
        y: 100.,
        max_size: 320.,
        title: "局部放大".into(),
    };
    let info =
        visualization_assets::image_info(&store, "p", &figure.id, 1, &spec.element_id).unwrap();
    assert_eq!(info["visible_crop"], json!([100., 20., 200., 100.]));
    let proposal = visualization_assets::inset(&store, "p", &figure.id, 1, &spec).unwrap();
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    let next = &proposal.document;
    assert_eq!(next["files"], scene["files"]);
    assert_eq!(next["elements"][0], scene["elements"][0]);
    assert_eq!(next["elements"].as_array().unwrap().len(), 6);
    let roi = &next["elements"][1];
    for (key, expected) in [("x", 104.), ("y", 6.), ("width", 96.), ("height", 72.)] {
        assert!(
            (roi[key].as_f64().unwrap() - expected).abs() < 1e-8,
            "{key}"
        );
    }
    let detail = &next["elements"][4];
    assert_eq!(detail["fileId"], "shared");
    assert_eq!(
        detail["crop"],
        json!({"x":200.,"y":30.,"width":80.,"height":60.,"naturalWidth":400,"naturalHeight":200})
    );
    assert_eq!(detail["customData"]["asset"]["license"], "cc0");
    assert_eq!(detail["customData"]["user_note"], "preserve");
    assert_eq!(detail["customData"]["inset"]["source_revision"], 1);
    assert_eq!(
        next["elements"][2]["startArrowhead"],
        serde_json::Value::Null
    );
    assert_eq!(next["elements"][2]["endArrowhead"], serde_json::Value::Null);
    assert!(visualization_assets::inset(&store, "p", &figure.id, 0, &spec).is_err());
    spec.region.width = f64::MIN_POSITIVE;
    assert!(visualization_assets::inset(&store, "p", &figure.id, 1, &spec).is_err());
    spec.region.width = 1.;
    assert!(visualization_assets::inset(&store, "p", &figure.id, 1, &spec).is_err());
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
}

#[test]
fn removing_one_inset_cleans_only_reciprocal_decorations() {
    use visualization::{
        inset,
        scene::{self, Operation},
    };
    let url = format!("data:image/svg+xml;base64,{}",STANDARD.encode(b"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><path d='M0 0h100v100H0z'/></svg>"));
    let (source, id) = image::insert(
        &json!({"elements":[]}),
        image::Image {
            file_id: "source",
            data_url: &url,
            width: 100,
            height: 100,
            provenance: json!({"asset":{"license":"cc0"}}),
        },
        0.,
        0.,
    )
    .unwrap();
    let spec = Spec {
        element_id: id,
        region: Region {
            x: 0.2,
            y: 0.2,
            width: 0.5,
            height: 0.5,
        },
        x: 500.,
        y: 0.,
        max_size: 320.,
        title: "Detail".into(),
    };
    let first = inset::compile(&source, &spec, 100, 100, 1, "hash").unwrap();
    let both = inset::compile(&first, &Spec { x: 1000., ..spec }, 100, 100, 1, "hash").unwrap();
    let anchor = first["elements"][3]["id"].as_str().unwrap();
    let removed = scene::apply(&both, &[Operation::Remove { id: anchor.into() }]).unwrap();
    assert_eq!(removed["files"], both["files"]);
    assert_eq!(removed["elements"].as_array().unwrap().len(), 6);
    assert_eq!(removed["elements"][0], source["elements"][0]);
    assert_eq!(
        &removed["elements"].as_array().unwrap()[1..],
        &both["elements"].as_array().unwrap()[6..]
    );
    // A copied or independently edited annotation is not owned by this anchor.
    let mut independent = both.clone();
    independent["elements"][1]["customData"]["inset"]["anchor_id"] = json!("different-inset");
    let retained = scene::apply(&independent, &[Operation::Remove { id: anchor.into() }]).unwrap();
    assert_eq!(
        retained["elements"][1]["id"],
        independent["elements"][1]["id"]
    );
    assert!(
        retained["elements"][1]["boundElements"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // A duplicated inset's stale customData must never delete the original's notes.
    let mut duplicated = both.clone();
    duplicated["elements"][3]["customData"]["inset"]["anchor_id"] = json!("old-anchor");
    let safe = scene::apply(&duplicated, &[Operation::Remove { id: anchor.into() }]).unwrap();
    assert!(
        safe["elements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == first["elements"][1]["id"])
    );
}

fn refresh_fixture() -> (Store, String, String, String) {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Study").unwrap();
    let svg=bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><path d='M0 0h100v100H0z' fill='red'/></svg>").unwrap();
    let (scene,source)=image::insert(&json!({"elements":[]}),image::Image{file_id:&svg.sha256,data_url:&svg.data_url,width:100,height:100,provenance:json!({"source":"fixture","asset":{"author":"Original","license":"cc0"},"sha256":svg.sha256})},0.,0.).unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &scene)
        .unwrap();
    let proposal = visualization_assets::inset(
        &store,
        "p",
        &figure.id,
        1,
        &Spec {
            element_id: source.clone(),
            region: Region {
                x: 0.2,
                y: 0.2,
                width: 0.5,
                height: 0.5,
            },
            x: 500.,
            y: 0.,
            max_size: 320.,
            title: "Detail".into(),
        },
    )
    .unwrap();
    let anchor = proposal.document["elements"][3]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    store
        .accept_proposal("p", &figure.id, &proposal.id)
        .unwrap();
    (store, figure.id, source, anchor)
}
fn element_mut<'a>(document: &'a mut serde_json::Value, id: &str) -> &'a mut serde_json::Value {
    document["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn explicit_refresh_keeps_ids_panel_title_styles_other_insets_and_history() {
    use visualization_assets::{inset_status, refresh_inset};
    let (mut store, figure, source, anchor) = refresh_fixture();
    let status = inset_status(&store, "p", &figure, 2, &anchor).unwrap();
    assert_eq!(
        status["changed"], false,
        "a newer figure revision alone is not stale"
    );
    let doc = store.revision("p", &figure, 2).unwrap().document;
    let second = visualization::inset::compile(
        &doc,
        &Spec {
            element_id: source.clone(),
            region: Region {
                x: 0.1,
                y: 0.1,
                width: 0.4,
                height: 0.4,
            },
            x: 1000.,
            y: 100.,
            max_size: 200.,
            title: "Other inset".into(),
        },
        100,
        100,
        2,
        status["source_sha256"].as_str().unwrap(),
    )
    .unwrap();
    store.save("p", &figure, 2, "Figure", &second).unwrap();
    assert_eq!(
        inset_status(&store, "p", &figure, 3, &anchor).unwrap()["changed"],
        false
    );
    let updated=bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' width='200' height='100'><path d='M0 0h200v100H0z' fill='blue'/></svg>").unwrap();
    let mut changed=image::replace_file(&second,&source,image::Image{file_id:&updated.sha256,data_url:&updated.data_url,width:200,height:100,provenance:json!({"source":"new fixture","asset":{"author":"New author","license":"cc-by-4.0"},"sha256":updated.sha256})}).unwrap();
    let source_image = element_mut(&mut changed, &source);
    source_image.as_object_mut().unwrap().extend(json!({"x":30,"y":40,"width":200,"height":100,"angle":std::f64::consts::FRAC_PI_2,"scale":[-1,1],"crop":{"x":20,"y":10,"width":100,"height":60,"naturalWidth":200,"naturalHeight":100}}).as_object().unwrap().clone());
    let image_id = status["image_id"].as_str().unwrap();
    element_mut(&mut changed, image_id)["customData"]["user_note"] = json!("My detail note");
    element_mut(&mut changed, &anchor)["strokeColor"] = json!("#884422");
    store.save("p", &figure, 3, "Figure", &changed).unwrap();
    let stale = inset_status(&store, "p", &figure, 4, &anchor).unwrap();
    for key in [
        "changed",
        "geometry_changed",
        "file_changed",
        "metadata_changed",
    ] {
        assert_eq!(stale[key], true, "{key}");
    }
    assert_eq!(stale["detail_edited"], false);
    let p = refresh_inset(&store, "p", &figure, 4, &anchor, false).unwrap();
    assert_eq!(store.get("p", &figure).unwrap().revision, 4);
    let old_members = visualization::inset_refresh::members(&changed, &anchor).unwrap();
    let members = visualization::inset_refresh::members(&p.document, &anchor).unwrap();
    assert_eq!(members.image["id"], old_members.image["id"]);
    assert_eq!(members.image["fileId"], updated.sha256);
    assert_eq!(
        members.image["crop"],
        json!({"x":50.,"y":22.,"width":50.,"height":30.,"naturalWidth":200,"naturalHeight":100})
    );
    assert_eq!(members.image["customData"]["asset"]["author"], "New author");
    assert_eq!(members.image["customData"]["asset"]["license"], "cc-by-4.0");
    assert_eq!(members.image["customData"]["user_note"], "My detail note");
    assert_eq!(
        members.image["customData"]["modifications"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["kind"],
        "inset_refresh"
    );
    for key in ["x", "y", "width", "height", "strokeColor", "groupIds"] {
        assert_eq!(members.anchor[key], old_members.anchor[key]);
    }
    assert_eq!(members.title, old_members.title);
    for (key, value) in [("x", 85.), ("y", 55.), ("width", 100.), ("height", 50.)] {
        assert!(
            (members.region[key].as_f64().unwrap() - value).abs() < 1e-8,
            "{key}"
        );
    }
    let updated_ids = [
        members.anchor["id"].clone(),
        members.image["id"].clone(),
        members.region["id"].clone(),
        members.callout["id"].clone(),
    ];
    for element in changed["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| !updated_ids.contains(&e["id"]))
    {
        assert_eq!(
            p.document["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == element["id"])
                .unwrap(),
            element
        );
    }
    assert_eq!(p.document["files"], changed["files"]);
    assert!(refresh_inset(&store, "p", &figure, 3, &anchor, false).is_err());
    store.accept_proposal("p", &figure, &p.id).unwrap();
    assert_eq!(
        inset_status(&store, "p", &figure, 5, &anchor).unwrap()["changed"],
        false
    );
    assert_eq!(store.revision("p", &figure, 4).unwrap().document, changed);
    assert_eq!(store.revision("p", &figure, 2).unwrap().document, doc);
}
#[test]
fn refresh_checks_local_edits_missing_sources_copied_members_and_rotated_panels() {
    use visualization_assets::{inset_status, refresh_inset};
    let (mut store, figure, source, anchor) = refresh_fixture();
    let mut doc = store.revision("p", &figure, 2).unwrap().document;
    let status = inset_status(&store, "p", &figure, 2, &anchor).unwrap();
    let id = status["image_id"].as_str().unwrap();
    element_mut(&mut doc, id)["crop"]["x"] = json!(10.);
    store.save("p", &figure, 2, "Figure", &doc).unwrap();
    assert_eq!(
        inset_status(&store, "p", &figure, 3, &anchor).unwrap()["detail_edited"],
        true
    );
    assert!(refresh_inset(&store, "p", &figure, 3, &anchor, false).is_err());
    let proposed = refresh_inset(&store, "p", &figure, 3, &anchor, true).unwrap();
    assert_eq!(store.get("p", &figure).unwrap().revision, 3);
    let mut copied = doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
        .clone();
    copied["id"] = json!("copied-detail");
    doc["elements"].as_array_mut().unwrap().push(copied);
    assert!(visualization::inset_refresh::members(&doc, "copied-detail").is_err());
    element_mut(&mut doc, &anchor)["angle"] = json!(0.5);
    assert!(
        visualization::inset_refresh::compile(&doc, &anchor, 100, 100, 3, "hash", "previous")
            .is_err()
    );
    let mut missing = proposed.document;
    missing["elements"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["id"] != source);
    store.save("p", &figure, 3, "Figure", &missing).unwrap();
    assert_eq!(
        inset_status(&store, "p", &figure, 4, &anchor).unwrap()["source_available"],
        false
    );
    assert!(refresh_inset(&store, "p", &figure, 4, &anchor, true).is_err());
}
