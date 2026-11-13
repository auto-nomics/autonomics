use serde_json::{Value, json};
use visualization::scene::{self, IllustrationMember, Operation};
fn fixture() -> Value {
    json!({"elements":[{"id":"frame","type":"frame","x":0,"y":0,"width":500,"height":500,"angle":0},{"id":"anchor","type":"rectangle","x":100,"y":100,"width":100,"height":100,"angle":0,"frameId":"frame","customData":{"scientificId":"complex","evidence":["retained"]},"boundElements":[{"id":"label","type":"text"}]},{"id":"primary","type":"image","x":110,"y":110,"width":60,"height":80,"angle":0,"frameId":"frame","fileId":"f","customData":{"source":"Fixture","asset":{"author":"Artist","license":"CC0"}}},{"id":"label","type":"text","x":100,"y":220,"width":200,"height":25,"angle":0,"frameId":"frame","containerId":"anchor","text":"Complete complex","originalText":"Complete complex","fontSize":20},{"id":"part","type":"image","x":180,"y":110,"width":60,"height":80,"angle":0,"frameId":"frame","fileId":"f","customData":{"source":"Fixture"}}],"files":{"f":{"dataURL":"data:image/png;base64,fixture"}}})
}
fn bind() -> Operation {
    Operation::BindIllustration {
        anchor_id: "anchor".into(),
        primary_id: "primary".into(),
        label_ids: vec!["label".into()],
        members: vec![IllustrationMember {
            id: "part".into(),
            meaning: "Generic intracellular protein component; structure unverified".into(),
        }],
    }
}
fn find<'a>(d: &'a Value, id: &str) -> &'a Value {
    d["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn binding_keeps_geometry_files_sources_and_science_then_moves_every_member() {
    let before = fixture();
    let after = scene::apply(&before, &[bind()]).unwrap();
    assert_eq!(after["files"], before["files"]);
    for old in before["elements"].as_array().unwrap() {
        let new = find(&after, old["id"].as_str().unwrap());
        for k in [
            "x",
            "y",
            "width",
            "height",
            "frameId",
            "text",
            "originalText",
            "fontSize",
            "fileId",
        ] {
            assert_eq!(new[k], old[k]);
        }
        for k in ["scientificId", "evidence", "source", "asset"] {
            assert_eq!(new["customData"][k], old["customData"][k]);
        }
    }
    assert_eq!(
        find(&after, "anchor")["customData"]["visualBadgeIds"],
        json!(["part"])
    );
    assert_eq!(
        find(&after, "primary")["customData"]["semanticAnchorId"],
        "anchor"
    );
    assert_eq!(find(&after, "label")["containerId"], Value::Null);
    assert_eq!(
        find(&after, "part")["customData"]["semanticBadgeAnchorId"],
        "anchor"
    );
    let moved = visualization::arrange::translate(&after, &["anchor".into()], 20., 10.).unwrap();
    for id in ["anchor", "primary", "label", "part"] {
        assert_eq!(
            find(&moved, id)["x"].as_f64().unwrap(),
            find(&after, id)["x"].as_f64().unwrap() + 20.
        );
        assert_eq!(
            find(&moved, id)["y"].as_f64().unwrap(),
            find(&after, id)["y"].as_f64().unwrap() + 10.
        );
    }
}
#[test]
fn foreign_ownership_groups_frame_escape_and_locks_reject_atomically() {
    for case in 0..5 {
        let mut d = fixture();
        let part = d["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "part")
            .unwrap();
        match case {
            0 => part["locked"] = json!(true),
            1 => part["frameId"] = Value::Null,
            2 => part["x"] = json!(490),
            3 => part["customData"]["semanticAnchorId"] = json!("foreign"),
            _ => {
                part["groupIds"] = json!(["foreign-group"]);
                d["elements"][0]["groupIds"] = json!(["foreign-group"]);
            }
        }
        let before = d.clone();
        assert!(scene::apply(&d, &[bind()]).is_err());
        assert_eq!(d, before);
    }
}
#[test]
fn duplicate_members_and_empty_meaning_reject() {
    let d = fixture();
    for op in [
        Operation::BindIllustration {
            anchor_id: "anchor".into(),
            primary_id: "primary".into(),
            label_ids: vec!["primary".into()],
            members: vec![],
        },
        Operation::BindIllustration {
            anchor_id: "anchor".into(),
            primary_id: "primary".into(),
            label_ids: vec![],
            members: vec![IllustrationMember {
                id: "part".into(),
                meaning: " ".into(),
            }],
        },
    ] {
        assert!(scene::apply(&d, &[op]).is_err());
    }
}
#[test]
fn optional_real_saved_composite_binding() {
    let Ok(source) = std::env::var("VISUALIZATION_BINDING_SOURCE") else {
        return;
    };
    let input: Value = serde_json::from_slice(&std::fs::read(source).unwrap()).unwrap();
    let d = &input["document"];
    let ops=[("node-3","7f687968-b62b-481e-b0ea-3e31301d15d4","label-3",vec![("0fbc506f-5d8b-4fc1-be3c-519ae87fd3e6","Generic intracellular protein symbol for SARA/SMAD components; identities and stoichiometry unverified")]),("node-4","e963a970-06e9-45bf-8334-97104ed65010","label-4",vec![("6a767c5a-fb02-4cf7-8422-1413d12a5bda","Generic intracellular protein symbol for SARA/SMAD components; identities and stoichiometry unverified"),("fe0a0621-6d9e-440b-97cb-c340f6ef8350","Generic phosphorylation marker; no site or stoichiometry evidence")])].into_iter().map(|(a,p,l,m)|Operation::BindIllustration{anchor_id:a.into(),primary_id:p.into(),label_ids:vec![l.into()],members:m.into_iter().map(|(id,meaning)|IllustrationMember{id:id.into(),meaning:meaning.into()}).collect()}).collect::<Vec<_>>();
    let next = scene::apply(d, &ops).unwrap();
    assert_eq!(next["files"], d["files"]);
    for old in d["elements"].as_array().unwrap() {
        let new = find(&next, old["id"].as_str().unwrap());
        for k in [
            "x",
            "y",
            "width",
            "height",
            "text",
            "originalText",
            "fontSize",
            "fileId",
            "startBinding",
            "endBinding",
        ] {
            assert_eq!(new[k], old[k]);
        }
    }
    for id in ["node-3", "node-4"] {
        assert!(find(&next, id)["customData"]["visualAnchorFit"].is_null());
    }
    if let Ok(path) = std::env::var("VISUALIZATION_BINDING_OUTPUT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"document":next})).unwrap(),
        )
        .unwrap();
    }
}
