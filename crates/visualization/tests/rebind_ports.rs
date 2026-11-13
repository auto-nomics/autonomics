use serde_json::{Value, json};
use visualization::scene::{self, ConnectorPort, Operation};
fn fixture() -> Value {
    let plan=serde_json::from_value(json!({"message":"Local routing","audience":"researchers","layout":"vertical","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b","label":"activates"}]})).unwrap();
    let mut d = visualization::sketch::compile(&plan).unwrap();
    for e in d["elements"].as_array_mut().unwrap() {
        e["frameId"] = json!("panel");
    }
    d["elements"].as_array_mut().unwrap().push(
        json!({"id":"panel","type":"frame","x":-400,"y":-100,"width":1200,"height":1200,"angle":0}),
    );
    d
}
fn endpoint(e: &Value, start: bool) -> [f64; 2] {
    let p = if start {
        &e["points"][0]
    } else {
        e["points"].as_array().unwrap().last().unwrap()
    };
    [
        e["x"].as_f64().unwrap() + p[0].as_f64().unwrap(),
        e["y"].as_f64().unwrap() + p[1].as_f64().unwrap(),
    ]
}

fn get<'a>(d: &'a Value, id: &str) -> &'a Value {
    d["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn rebind(id: &str) -> Operation {
    Operation::RebindConnectorPorts {
        id: id.into(),
        start_side: ConnectorPort::Bottom,
        end_side: ConnectorPort::Top,
    }
}
#[test]
fn explicit_ports_repair_diagonal_endpoints_and_keep_caption_and_bindings() {
    let mut d = fixture();
    let arrow = d["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == "edge-0")
        .unwrap();
    arrow["points"][1][0] = json!(7.);
    assert!(
        scene::apply(
            &d,
            &[Operation::Reroute {
                id: "edge-0".into()
            }]
        )
        .is_err()
    );
    let next = scene::apply(&d, &[rebind("edge-0")]).unwrap();
    let old = get(&d, "edge-0");
    let new = get(&next, "edge-0");
    for k in [
        "startBinding",
        "endBinding",
        "startArrowhead",
        "endArrowhead",
        "customData",
    ] {
        assert_eq!(new[k], old[k]);
    }
    for e in d["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != "edge-0")
    {
        assert_eq!(get(&next, e["id"].as_str().unwrap()), e);
    }
    let source = get(&next, old["startBinding"]["elementId"].as_str().unwrap());
    let end = get(&next, old["endBinding"]["elementId"].as_str().unwrap());
    assert_eq!(
        endpoint(new, true),
        [
            source["x"].as_f64().unwrap() + source["width"].as_f64().unwrap() / 2.,
            source["y"].as_f64().unwrap()
                + source["height"].as_f64().unwrap()
                + old["startBinding"]["gap"].as_f64().unwrap()
        ]
    );
    assert_eq!(
        endpoint(new, false),
        [
            end["x"].as_f64().unwrap() + end["width"].as_f64().unwrap() / 2.,
            end["y"].as_f64().unwrap() - old["endBinding"]["gap"].as_f64().unwrap()
        ]
    );
}
#[test]
fn unsupported_binding_and_geometry_reject_atomically() {
    for case in 0..4 {
        let mut d = fixture();
        let e = d["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "edge-0")
            .unwrap();
        match case {
            0 => e["locked"] = json!(true),
            1 => e["roundness"] = json!({"type":2}),
            2 => e["startBinding"]["focus"] = json!(0.5),
            _ => e["startBinding"]["fixedPoint"] = json!([0.5, 0.5]),
        };
        let before = d.clone();
        assert!(scene::apply(&d, &[rebind("edge-0")]).is_err());
        assert_eq!(d, before);
    }
}
#[test]
fn optional_saved_smad_repair() {
    let Ok(path) = std::env::var("VISUALIZATION_PORT_SOURCE") else {
        return;
    };
    let input: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let d = &input["document"];
    let label = get(d, "label-6");
    let ops = [
        Operation::PositionArrowCaption { id:"edge-label-5".into(), position:0.5 },
        Operation::SetTextPosition {
            id: "label-6".into(),
            x: label["x"].as_f64().unwrap(),
            y: 337.,
        },
        Operation::Reroute {
            id: "edge-4".into(),
        },
        rebind("edge-5"),
        rebind("edge-6"),
    ];
    let next = scene::apply(d, &ops).unwrap();
    assert_eq!(next["files"], d["files"]);
    for old in d["elements"].as_array().unwrap() {
        let new = get(&next, old["id"].as_str().unwrap());
        for key in [
            "text",
            "originalText",
            "fontSize",
            "fontFamily",
            "startBinding",
            "endBinding",
            "startArrowhead",
            "endArrowhead",
        ] {
            assert_eq!(new[key], old[key]);
        }
        if !["edge-4", "edge-5", "edge-6", "edge-label-4", "edge-label-5", "label-6"]
            .contains(&old["id"].as_str().unwrap())
        {
            assert_eq!(new, old);
        }
    }
    if let Ok(path) = std::env::var("VISUALIZATION_PORT_OUTPUT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"document":next})).unwrap(),
        )
        .unwrap();
    }
}
