use serde_json::json;
use visualization::scene::{self, Operation};
#[test]
fn native_numeric_fields_reject_quoted_null_and_container_values_atomically() {
    let original = json!({"elements":[{"id":"node","type":"rectangle","x":0,"y":0,"width":220,"height":100,"angle":0}],"files":{}});
    for field in scene::NUMERIC_UPDATE_FIELDS {
        for value in [
            json!("240"),
            json!(null),
            json!({"value":240}),
            json!([240]),
        ] {
            let mut changes = serde_json::Map::new();
            changes.insert(field.into(), value);
            let operations = [
                Operation::Update {
                    id: "node".into(),
                    changes: serde_json::from_value(json!({"backgroundColor":"red"})).unwrap(),
                },
                Operation::Update {
                    id: "node".into(),
                    changes,
                },
            ];
            let error = scene::apply(&original, &operations)
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("node.{field}")), "{error}");
            assert_eq!(original["elements"][0]["width"], 220);
            assert!(original["elements"][0].get("backgroundColor").is_none());
        }
    }
    let changed = scene::apply(
        &original,
        &[Operation::Update {
            id: "node".into(),
            changes: serde_json::from_value(
                json!({"width":240,"opacity":75,"backgroundColor":"red"}),
            )
            .unwrap(),
        }],
    )
    .unwrap();
    assert_eq!(changed["elements"][0]["width"], 240);
    assert_eq!(changed["elements"][0]["opacity"], 75);
}

#[test]
fn update_schema_exposes_numeric_types_while_preserving_other_native_fields() {
    let schema = serde_json::to_value(schemars::schema_for!(Operation)).unwrap();
    let update = schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|variant| variant["properties"]["op"]["const"] == "update")
        .unwrap();
    let changes = &update["properties"]["changes"];
    for field in scene::NUMERIC_UPDATE_FIELDS {
        assert_eq!(changes["properties"][field]["type"], "number");
    }
    assert_eq!(changes["additionalProperties"], true);
}
