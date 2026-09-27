//! Manifest → dag-core compilations.
//!
//! Each function turns the declarative manifest DTO into the runtime
//! contract the engine consumes. Pure: no I/O, no process state, no shared
//! mutable globals. Same input → same output, always.
//!
//! - [`compile_schema`] — `params: BTreeMap<String, ParamSpec>` → JSON Schema
//!   that agents see when wiring nodes.
//! - [`compile_ports`] (in `node_entry`) — port layout → `NodePorts`.
//! - [`compile_container_spec`] (future) — node entry + param values +
//!   panels → `ContainerCommandSpec` ready for podman.

pub mod error;
pub mod spec_compile;
mod utils;

use std::collections::BTreeMap;

use schemars::Schema;
use schemars::json_schema;
use serde_json::{Map, Value, json};

use crate::node_definition::{ParamSpec, ParamType};

/// Compile the node's parameter DSL into a single JSON Schema of the form
/// `{"type":"object","properties":{...},"required":[...],"additionalProperties":false}`.
///
/// The object is `additionalProperties: false` so the agent cannot smuggle
/// in fields the manifest doesn't declare — this is the same fail-closed
/// posture used for `HfRepoId` and `ManifestDigest`.
///
/// `requires` is currently informational only: each `requires: ["x"]`
/// declaration becomes the description of the parameter, telling the agent
/// to set the gate. Runtime enforcement lands in `compile_container_spec`
/// when it ships.
pub fn compile_schema(params: &BTreeMap<String, ParamSpec>) -> Schema {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for (name, spec) in params {
        properties.insert(name.clone(), param_value(spec));
        if spec.default.is_none() {
            required.push(name.clone());
        }
    }

    let mut root = json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
        "additionalProperties": false,
    });
    if let Value::Object(map) = &mut root {
        map.insert(
            "title".to_string(),
            Value::String("PluginNodeParams".into()),
        );
    }
    Schema::try_from(root).expect("compile_schema always produces a valid schema")
}

/// Render one parameter as a JSON value (the `serde_json::Value` form of a
/// JSON Schema node). The shape mirrors [`ParamType`]: closed vocabulary,
/// no `anyOf` ambiguity, default values pass through as-is.
fn param_value(spec: &ParamSpec) -> Value {
    let mut node = match spec.r#type {
        ParamType::Bool => json!({ "type": "boolean" }),
        ParamType::Int => int_node(spec),
        ParamType::Number => number_node(spec),
        ParamType::String => string_node(spec),
        ParamType::StringArray => string_array_node(spec),
    };

    if let Some(doc) = &spec.doc {
        insert_string(&mut node, "description", doc.clone());
    } else if !spec.requires.is_empty() {
        // Surface gating constraints even when authors forget to document.
        let deps = spec.requires.join(", ");
        insert_string(&mut node, "description", format!("requires: {deps}"));
    }

    if let Some(default) = &spec.default {
        if let Value::Object(map) = &mut node {
            map.insert("default".to_string(), default.clone());
        }
    }

    node
}

fn int_node(spec: &ParamSpec) -> Value {
    let mut node = json!({ "type": "integer" });
    apply_numeric_bounds(&mut node, spec);
    node
}

fn number_node(spec: &ParamSpec) -> Value {
    let mut node = json!({ "type": "number" });
    apply_numeric_bounds(&mut node, spec);
    node
}

fn apply_numeric_bounds(node: &mut Value, spec: &ParamSpec) {
    let Value::Object(map) = node else {
        return;
    };
    if let Some(min) = spec.min {
        map.insert("minimum".into(), json!(min));
    }
    if let Some(max) = spec.max {
        map.insert("maximum".into(), json!(max));
    }
    if let Some(min) = spec.exclusive_min {
        map.insert("exclusiveMinimum".into(), json!(min));
    }
    if let Some(max) = spec.exclusive_max {
        map.insert("exclusiveMaximum".into(), json!(max));
    }
}

fn string_node(spec: &ParamSpec) -> Value {
    let mut node = json!({ "type": "string" });
    if let Some(min) = spec.min_len {
        insert_number(&mut node, "minLength", min as f64);
    }
    if let Some(max) = spec.max_len {
        insert_number(&mut node, "maxLength", max as f64);
    }
    node
}

fn string_array_node(spec: &ParamSpec) -> Value {
    let mut node = json!({
        "type": "array",
        "items": { "type": "string" },
    });
    if let Some(min) = spec.min_len {
        insert_number(&mut node, "minItems", min as f64);
    }
    if let Some(max) = spec.max_len {
        insert_number(&mut node, "maxItems", max as f64);
    }
    node
}

fn insert_string(node: &mut Value, key: &str, value: String) {
    if let Value::Object(map) = node {
        map.insert(key.into(), Value::String(value));
    }
}

fn insert_number(node: &mut Value, key: &str, value: f64) {
    if let Value::Object(map) = node {
        map.insert(
            key.into(),
            // JSON Schema expects integer bounds for `*Length`/`*Items`
            // properties but `serde_json::Number::from_f64` keeps them as
            // floats; a Number serde-json value preserves either.
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_definition::ParamSpec;
    use serde_json::json;

    fn compile(params: &[(&str, ParamSpec)]) -> Value {
        let map = params
            .iter()
            .map(|(name, spec)| (name.to_string(), spec.clone()))
            .collect();
        serde_json::to_value(compile_schema(&map)).unwrap()
    }

    #[test]
    fn object_root_with_required_and_additional_properties_false() {
        let schema = compile(&[
            (
                "with_default",
                ParamSpec {
                    r#type: ParamType::Bool,
                    default: Some(json!(false)),
                    doc: None,
                    min: None,
                    max: None,
                    exclusive_min: None,
                    exclusive_max: None,
                    min_len: None,
                    max_len: None,
                    requires: Vec::new(),
                },
            ),
            (
                "required_param",
                ParamSpec {
                    r#type: ParamType::String,
                    default: None,
                    doc: None,
                    min: None,
                    max: None,
                    exclusive_min: None,
                    exclusive_max: None,
                    min_len: None,
                    max_len: None,
                    requires: Vec::new(),
                },
            ),
        ]);

        assert_eq!(schema["title"], "PluginNodeParams");
        assert_eq!(schema["additionalProperties"], false);
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required, &vec![json!("required_param")]);
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn boolean_carries_default_and_description() {
        let schema = compile(&[(
            "force",
            ParamSpec {
                r#type: ParamType::Bool,
                default: Some(json!(false)),
                doc: Some("Continue on mean-chi-square rejection".into()),
                min: None,
                max: None,
                exclusive_min: None,
                exclusive_max: None,
                min_len: None,
                max_len: None,
                requires: Vec::new(),
            },
        )]);
        let prop = &schema["properties"]["force"];
        assert_eq!(prop["type"], "boolean");
        assert_eq!(prop["default"], false);
        assert_eq!(prop["description"], "Continue on mean-chi-square rejection");
    }

    #[test]
    fn numeric_bounds_emit_minimum_maximum_exclusiveMinimum_exclusiveMaximum() {
        let schema = compile(&[(
            "time_limit_hours",
            ParamSpec {
                r#type: ParamType::Number,
                default: Some(json!(1.0)),
                doc: None,
                min: Some(0.0),
                max: Some(24.0),
                exclusive_min: None,
                exclusive_max: Some(72.0),
                min_len: None,
                max_len: None,
                requires: Vec::new(),
            },
        )]);
        let prop = &schema["properties"]["time_limit_hours"];
        assert_eq!(prop["type"], "number");
        assert_eq!(prop["minimum"], 0.0);
        assert_eq!(prop["maximum"], 24.0);
        assert_eq!(prop["exclusiveMaximum"], 72.0);
        assert!(prop.get("exclusiveMinimum").is_none());
    }

    #[test]
    fn integer_uses_integer_not_number() {
        let schema = compile(&[(
            "nb_distribution",
            ParamSpec {
                r#type: ParamType::Int,
                default: None,
                doc: None,
                min: Some(1000.0),
                max: None,
                exclusive_min: None,
                exclusive_max: None,
                min_len: None,
                max_len: None,
                requires: Vec::new(),
            },
        )]);
        let prop = &schema["properties"]["nb_distribution"];
        assert_eq!(prop["type"], "integer");
        assert_eq!(prop["minimum"], 1000.0);
    }

    #[test]
    fn string_array_uses_array_of_string() {
        let schema = compile(&[(
            "beta_exposure",
            ParamSpec {
                r#type: ParamType::StringArray,
                default: None,
                doc: None,
                min: None,
                max: None,
                exclusive_min: None,
                exclusive_max: None,
                min_len: Some(1),
                max_len: Some(8),
                requires: Vec::new(),
            },
        )]);
        let prop = &schema["properties"]["beta_exposure"];
        assert_eq!(prop["type"], "array");
        assert_eq!(prop["items"]["type"], "string");
        assert_eq!(prop["minItems"], 1.0);
        assert_eq!(prop["maxItems"], 8.0);
    }

    #[test]
    fn requires_surfaces_in_description_when_doc_is_missing() {
        let schema = compile(&[(
            "equal_h2",
            ParamSpec {
                r#type: ParamType::Bool,
                default: Some(json!(false)),
                doc: None,
                min: None,
                max: None,
                exclusive_min: None,
                exclusive_max: None,
                min_len: None,
                max_len: None,
                requires: vec!["force".into()],
            },
        )]);
        let prop = &schema["properties"]["equal_h2"];
        assert_eq!(prop["description"], "requires: force");
    }

    #[test]
    fn mtag_style_fixture_renders_a_sound_schema() {
        // A representative cross-section: required string, optional bool
        // (with description), optional number (with bounds), optional bool
        // gated by `requires`. Verifies all branches compose correctly.
        let schema = compile(&[
            (
                "time_limit_hours",
                ParamSpec {
                    r#type: ParamType::Number,
                    default: Some(json!(1.0)),
                    doc: Some("Optimizer time limit".into()),
                    min: None,
                    max: None,
                    exclusive_min: Some(0.0),
                    exclusive_max: None,
                    min_len: None,
                    max_len: None,
                    requires: Vec::new(),
                },
            ),
            (
                "force",
                ParamSpec {
                    r#type: ParamType::Bool,
                    default: Some(json!(false)),
                    doc: None,
                    min: None,
                    max: None,
                    exclusive_min: None,
                    exclusive_max: None,
                    min_len: None,
                    max_len: None,
                    requires: Vec::new(),
                },
            ),
            (
                "equal_h2",
                ParamSpec {
                    r#type: ParamType::Bool,
                    default: Some(json!(false)),
                    doc: None,
                    min: None,
                    max: None,
                    exclusive_min: None,
                    exclusive_max: None,
                    min_len: None,
                    max_len: None,
                    requires: vec!["force".into()],
                },
            ),
        ]);

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!([])); // all three have defaults
        assert_eq!(schema["properties"]["time_limit_hours"]["type"], "number");
        assert_eq!(schema["properties"]["force"]["type"], "boolean");
        assert_eq!(
            schema["properties"]["equal_h2"]["description"],
            "requires: force"
        );
    }
}
