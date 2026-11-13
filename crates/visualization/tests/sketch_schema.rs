use serde_json::json;
use visualization::sketch::{self, Plan};

#[test]
fn sketch_schema_exposes_existing_compile_limits_without_defaulting_missing_intent() {
    let schema = serde_json::to_value(schemars::schema_for!(Plan)).unwrap();
    assert_eq!(schema["properties"]["message"]["minLength"], 1);
    assert_eq!(schema["properties"]["audience"]["minLength"], 1);
    assert_eq!(schema["properties"]["nodes"]["minItems"], 1);
    assert_eq!(schema["properties"]["nodes"]["maxItems"], 100);
    assert_eq!(schema["properties"]["edges"]["maxItems"], 200);
    for field in ["message", "audience"] {
        let mut spec = json!({"message":"Explicit conceptual flow","audience":"Researchers","nodes":[{"id":"a","label":"Collect"}]});
        spec[field] = json!(" ");
        let plan: Plan = serde_json::from_value(spec).unwrap();
        assert!(sketch::compile(&plan).is_err());
    }
}
