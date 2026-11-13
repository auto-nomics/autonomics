use serde_json::{Value, json};
use visualization::image::{self, Image};
fn asset(id: &'static str, width: u32, height: u32) -> Image<'static> {
    Image {
        file_id: id,
        data_url: if id == "original" {
            "data:image/png;base64,b2xk"
        } else {
            "data:image/png;base64,bmV3"
        },
        width,
        height,
        provenance: json!({"source":"Fixture","asset":{"author":id,"license":"cc-0"},"source_sha256":id}),
    }
}
fn fixture() -> (Value, String) {
    let (mut doc, id) = image::insert(
        &json!({"elements":[],"files":{}}),
        asset("original", 200, 100),
        10.,
        20.,
    )
    .unwrap();
    let e = &mut doc["elements"][0];
    e["width"] = json!(350);
    e["height"] = json!(140);
    e["angle"] = json!(0.6);
    e["scale"] = json!([-1, 1]);
    e["opacity"] = json!(75);
    e["groupIds"] = json!(["manual-group"]);
    e["frameId"] = json!("panel");
    e["crop"] =
        json!({"x":20,"y":10,"width":100,"height":60,"naturalWidth":200,"naturalHeight":100});
    e["customData"]["userNote"] = json!("Preserve human context");
    e["customData"]["licenses"] = json!(["previous-source-license"]);
    e["customData"]["modifications"] = json!([{"kind":"recolor","colors":{"#ff0000":"#00ff00"}}]);
    e["boundElements"] = json!([{"type":"arrow","id":"arrow"}]);
    doc["elements"].as_array_mut().unwrap().push(json!({"id":"arrow","type":"arrow","startBinding":{"elementId":id},"endBinding":{"elementId":"other"},"points":[[0,0],[100,0]],"customData":{"evidence":["Supplied reference"]}}));
    (doc, id)
}
#[test]
fn standalone_replacement_keeps_identity_crop_fraction_transforms_bindings_and_user_context() {
    let (doc, id) = fixture();
    let (result, new_id) = image::illustrate(&doc, &id, asset("replacement", 400, 300)).unwrap();
    assert_eq!(new_id, id);
    assert_eq!(result["elements"].as_array().unwrap().len(), 2);
    let before = &doc["elements"][0];
    let next = &result["elements"][0];
    assert_eq!(next["type"], "image");
    assert_eq!(next["fileId"], "replacement");
    for (key, value) in before.as_object().unwrap() {
        if ![
            "fileId",
            "crop",
            "customData",
            "version",
            "versionNonce",
            "updated",
        ]
        .contains(&key.as_str())
        {
            assert_eq!(&next[key], value, "preserve {key}");
        }
    }
    assert_eq!(
        next["crop"],
        json!({"x":40.,"y":30.,"width":200.,"height":180.,"naturalWidth":400,"naturalHeight":300})
    );
    assert_eq!(
        next["customData"]["userNote"],
        before["customData"]["userNote"]
    );
    assert_eq!(next["customData"]["asset"]["author"], "replacement");
    assert_eq!(next["customData"]["source_sha256"], "replacement");
    assert!(next["customData"].get("modifications").is_none());
    assert!(
        next["customData"].get("licenses").is_none(),
        "old top-level source licences must not describe new art"
    );
    assert!(next["customData"].get("semanticAnchorId").is_none());
    let history = &next["customData"]["asset_replacements"][0];
    assert_eq!(history["previous_file_id"], "original");
    assert_eq!(
        history["previous_provenance"]["licenses"],
        json!(["previous-source-license"])
    );
    assert_eq!(
        history["previous_provenance"]["asset"]["author"],
        "original"
    );
    assert_eq!(
        history["previous_provenance"]["modifications"],
        before["customData"]["modifications"]
    );
    assert_eq!(history["crop_policy"], "normalized_source_region");
    assert_eq!(result["elements"][1], doc["elements"][1]);
    assert_eq!(result["files"]["original"], doc["files"]["original"]);
    assert_eq!(doc["elements"][0]["type"], "image");
    let (again, _) = image::illustrate(&result, &id, asset("third", 100, 50)).unwrap();
    let history = again["elements"][0]["customData"]["asset_replacements"]
        .as_array()
        .unwrap();
    assert_eq!(history.len(), 2);
    assert!(
        history[1]["previous_provenance"]
            .get("asset_replacements")
            .is_none()
    );
    assert_eq!(
        again["elements"][0]["crop"],
        json!({"x":10.,"y":5.,"width":50.,"height":30.,"naturalWidth":100,"naturalHeight":50})
    );
}
#[test]
fn uncropped_image_stays_uncropped_and_does_not_become_a_placeholder() {
    let (mut doc, id) = fixture();
    doc["elements"][0]["crop"] = Value::Null;
    let (result, new_id) = image::illustrate(&doc, &id, asset("replacement", 10, 1000)).unwrap();
    assert_eq!(new_id, id);
    assert_eq!(result["elements"][0]["crop"], Value::Null);
    assert_eq!(result["elements"][0]["width"], 350);
    assert_eq!(result["elements"][0]["height"], 140);
    assert_eq!(result["elements"][0]["type"], "image");
    assert_eq!(
        result["elements"][0]["customData"]["asset_replacements"][0]["crop_policy"],
        "uncropped"
    );
}
#[test]
fn invalid_crops_locked_images_opaque_history_and_ambiguous_ownership_reject_atomically() {
    for (key, value) in [
        ("locked", json!(true)),
        ("crop", json!({"x":20,"y":10,"width":100,"height":60})),
        (
            "crop",
            json!({"x":190,"y":10,"width":100,"height":60,"naturalWidth":200,"naturalHeight":100}),
        ),
        ("customData", json!({"asset_replacements":{}})),
        ("customData", json!({"semanticAnchorId":"missing-owner"})),
    ] {
        let (mut doc, id) = fixture();
        doc["elements"][0][key] = value;
        let before = doc.clone();
        assert!(image::illustrate(&doc, &id, asset("replacement", 400, 300)).is_err());
        assert_eq!(doc, before);
    }
    let (mut doc, id) = fixture();
    doc["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"owner","type":"rectangle","customData":{"visualAssetElementId":id}}));
    assert!(image::illustrate(&doc, &id, asset("replacement", 400, 300)).is_err());
}

fn compact_protein_frame() -> Value {
    json!({"elements":[
        {"id":"frame","type":"frame","x":0.,"y":0.,"width":304.,"height":254.,"angle":0.},
        {"id":"heading","type":"text","x":40.,"y":30.,"width":224.,"height":50.,"angle":0.,"frameId":"frame","text":"C · RAS family"},
        {"id":"node","type":"rectangle","x":40.,"y":130.,"width":220.,"height":100.,"angle":0.,"frameId":"frame","boundElements":[{"id":"caption","type":"text"}],"customData":{"scientificId":"ras"}},
        {"id":"caption","type":"text","x":50.,"y":155.,"width":200.,"height":50.,"angle":0.,"frameId":"frame","containerId":"node","fontFamily":9,"fontSize":20.,"lineHeight":1.25,"text":"RAS-GTP（p21 RAS 家族）","originalText":"RAS-GTP（p21 RAS 家族）"}
    ],"files":{},"scientificPlan":{"message":"Concept only"}})
}
#[test]
fn fresh_compact_frame_artwork_keeps_its_complete_label_visible_above_when_below_would_clip() {
    let old=compact_protein_frame();
    let (new,image_id)=image::illustrate(&old,"node",asset("replacement",100,220)).unwrap();
    let get=|id:&str|new["elements"].as_array().unwrap().iter().find(|e|e["id"]==id).unwrap();
    let label=get("caption");let artwork=get(&image_id);
    assert_eq!(label["originalText"],old["elements"][3]["originalText"]);
    assert_eq!(label["fontFamily"],9);assert_eq!(label["fontSize"],20.);
    assert!(label["x"].as_f64().unwrap()>=0.);
    assert!(label["x"].as_f64().unwrap()+label["width"].as_f64().unwrap()<=304.);
    assert!(label["y"].as_f64().unwrap()+label["height"].as_f64().unwrap()<=artwork["y"].as_f64().unwrap()-8.);
    assert_eq!(get("frame"),&old["elements"][0]);assert_eq!(get("heading"),&old["elements"][1]);
    assert_eq!(new["scientificPlan"],old["scientificPlan"]);
}
#[test]
fn fresh_artwork_with_no_visible_caption_space_rejects_without_modifying_the_source() {
    let mut old=compact_protein_frame();old["elements"].as_array_mut().unwrap().push(json!({"id":"blocker","type":"rectangle","x":0.,"y":0.,"width":304.,"height":130.,"angle":0.,"frameId":"frame"}));let before=old.clone();
    let error=image::illustrate(&old,"node",asset("replacement",100,220)).unwrap_err();
    assert!(error.0.contains("label cannot fit"));assert_eq!(old,before);
}
