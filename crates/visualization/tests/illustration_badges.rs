use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn scene() -> Value {
    serde_json::from_str(include_str!("fixtures/illustration-badge-component.json")).unwrap()
}
fn attach() -> Operation {
    Operation::AttachIllustrationBadge {
        id: "badge".into(),
        anchor_id: "node".into(),
        meaning: "phosphorylation state (user-declared)".into(),
    }
}
fn e<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn edit<'a>(doc: &'a mut Value, id: &str) -> &'a mut Value {
    doc["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn attachment_is_explicit_reciprocal_and_moves_as_one_component_from_either_member() {
    let before = scene();
    let attached = scene::apply(&before, &[attach()]).unwrap();
    assert_eq!(
        e(&attached, "node")["customData"]["visualBadgeIds"],
        json!(["badge"])
    );
    assert_eq!(
        e(&attached, "badge")["customData"]["semanticBadgeAnchorId"],
        "node"
    );
    assert_eq!(e(&attached, "badge")["groupIds"], json!(["g"]));
    assert_eq!(attached["files"], before["files"]);
    assert_eq!(e(&attached, "badge")["customData"]["sha256"], "retained");
    for selected in ["node", "badge"] {
        let moved = scene::apply(
            &attached,
            &[Operation::Translate {
                element_ids: vec![selected.into()],
                dx: 10.,
                dy: 20.,
            }],
        )
        .unwrap();
        for id in ["node", "art", "label", "badge"] {
            assert_eq!(
                e(&moved, id)["x"].as_f64().unwrap(),
                e(&attached, id)["x"].as_f64().unwrap() + 10.
            );
            assert_eq!(
                e(&moved, id)["y"].as_f64().unwrap(),
                e(&attached, id)["y"].as_f64().unwrap() + 20.
            );
        }
        assert_eq!(e(&moved, "unrelated"), e(&attached, "unrelated"));
        assert_eq!(moved["files"], attached["files"]);
    }
    let ungrouped = {
        let mut doc = attached.clone();
        for id in ["node", "art", "label", "badge"] {
            edit(&mut doc, id)["groupIds"] = json!([]);
        }
        doc
    };
    let moved = visualization::arrange::translate(&ungrouped, &["badge".into()], 10., 20.).unwrap();
    assert_eq!(e(&moved, "node")["x"], 60.);
    assert_eq!(e(&moved, "art")["x"], 60.);
}
#[test]
fn detach_preserves_image_meaning_and_sources_and_remove_cleans_references() {
    let attached = scene::apply(&scene(), &[attach()]).unwrap();
    let detached = scene::apply(
        &attached,
        &[Operation::DetachIllustrationBadge {
            id: "badge".into(),
            anchor_id: "node".into(),
        }],
    )
    .unwrap();
    assert!(e(&detached, "badge")["customData"]["semanticBadgeAnchorId"].is_null());
    assert!(
        e(&detached, "node")["customData"]["visualBadgeIds"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        e(&detached, "badge")["customData"]["badgeMeaning"],
        e(&attached, "badge")["customData"]["badgeMeaning"]
    );
    assert_eq!(detached["files"], attached["files"]);
    let moved = visualization::arrange::translate(&detached, &["node".into()], 10., 20.).unwrap();
    assert_eq!(e(&moved, "badge"), e(&detached, "badge"));
    let removed = scene::apply(&attached, &[Operation::Remove { id: "badge".into() }]).unwrap();
    assert_eq!(
        e(&removed, "node")["customData"]["visualBadgeIds"],
        json!([])
    );
    let removed = scene::apply(&attached, &[Operation::Remove { id: "node".into() }]).unwrap();
    assert!(
        !removed["elements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| ["node", "art", "label", "badge"].contains(&e["id"].as_str().unwrap()))
    );
    assert_eq!(removed["files"], attached["files"]);
}
#[test]
fn duplicate_remaps_both_directions_and_group_and_replacement_retains_badges() {
    let attached = scene::apply(&scene(), &[attach()]).unwrap();
    let copied = scene::apply(
        &attached,
        &[Operation::Duplicate {
            element_ids: vec!["badge".into()],
            id_prefix: "copy".into(),
            dx: 10.,
            dy: 20.,
        }],
    )
    .unwrap();
    let nodes = copied["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "rectangle" && e["customData"]["visualAssetElementId"].is_string())
        .collect::<Vec<_>>();
    assert_eq!(nodes.len(), 2);
    let clone = *nodes.iter().find(|e| e["id"] != "node").unwrap();
    let badge_id = clone["customData"]["visualBadgeIds"][0].as_str().unwrap();
    let badge = e(&copied, badge_id);
    assert_eq!(badge["customData"]["semanticBadgeAnchorId"], clone["id"]);
    assert_eq!(
        badge["customData"]["visualBadgeGroupId"],
        clone["customData"]["visualAssetGroupId"]
    );
    assert_eq!(
        clone["customData"]["visualAnchorFit"]["imageBounds"],
        json!([60., 70., 50, 70])
    );
    let replacement = visualization::image::Image {
        file_id: "new",
        data_url: "data:image/svg+xml;base64,PHN2Zy8+",
        width: 50,
        height: 70,
        provenance: json!({"source":"new primary fixture"}),
    };
    let (replaced, _) = visualization::image::illustrate(&attached, "node", replacement).unwrap();
    assert_eq!(e(&replaced, "badge"), e(&attached, "badge"));
    assert_eq!(
        e(&replaced, "node")["customData"]["visualBadgeIds"],
        json!(["badge"])
    );
    assert_eq!(
        e(&replaced, "node")["customData"]["evidence"],
        e(&attached, "node")["customData"]["evidence"]
    );
}
#[test]
fn invalid_attachment_and_later_batch_failure_never_mutate_source() {
    for variant in [
        "badge locked",
        "owner locked",
        "grouped",
        "wrong frame",
        "outside",
        "already primary",
        "stale list",
        "rotated",
    ] {
        let mut doc = scene();
        match variant {
            "badge locked" => edit(&mut doc, "badge")["locked"] = json!(true),
            "owner locked" => edit(&mut doc, "node")["locked"] = json!(true),
            "grouped" => edit(&mut doc, "badge")["groupIds"] = json!(["unrelated"]),
            "wrong frame" => edit(&mut doc, "badge")["frameId"] = Value::Null,
            "outside" => edit(&mut doc, "badge")["x"] = json!(-50),
            "already primary" => {
                edit(&mut doc, "badge")["customData"]["semanticAnchorId"] = json!("node")
            }
            "stale list" => {
                edit(&mut doc, "node")["customData"]["visualBadgeIds"] = json!(["missing"])
            }
            "rotated" => edit(&mut doc, "badge")["angle"] = json!(1.),
            _ => unreachable!(),
        }
        let before = doc.clone();
        assert!(scene::apply(&doc, &[attach()]).is_err(), "{variant}");
        assert_eq!(doc, before);
    }
    let doc = scene();
    assert!(scene::apply(&doc, &[attach(), attach()]).is_err());
    assert_eq!(e(&doc, "badge")["groupIds"], json!([]));
}

#[test]
fn explicit_detach_still_works_after_native_ungrouping() {
    let mut attached = scene::apply(&scene(), &[attach()]).unwrap();
    for id in ["node", "art", "label", "badge"] {
        edit(&mut attached, id)["groupIds"] = json!([]);
    }
    let detached = scene::apply(
        &attached,
        &[Operation::DetachIllustrationBadge {
            id: "badge".into(),
            anchor_id: "node".into(),
        }],
    )
    .unwrap();
    assert!(e(&detached, "badge")["customData"]["semanticBadgeAnchorId"].is_null());
}
#[test]
fn replacing_a_badge_keeps_owner_meaning_and_native_group() {
    let attached = scene::apply(&scene(), &[attach()]).unwrap();
    let (replaced, _) = visualization::image::illustrate(
        &attached,
        "badge",
        visualization::image::Image {
            file_id: "new-badge",
            data_url: "data:image/svg+xml;base64,PHN2Zy8+",
            width: 20,
            height: 20,
            provenance: json!({"source":"new badge fixture"}),
        },
    )
    .unwrap();
    for key in [
        "semanticBadgeAnchorId",
        "badgeMeaning",
        "visualBadgeGroupId",
    ] {
        assert_eq!(
            e(&replaced, "badge")["customData"][key],
            e(&attached, "badge")["customData"][key]
        );
    }
    assert_eq!(e(&replaced, "badge")["groupIds"], json!(["g"]));
    assert_eq!(e(&replaced, "node"), e(&attached, "node"));
}
