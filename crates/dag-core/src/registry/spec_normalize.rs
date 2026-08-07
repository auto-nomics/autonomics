//! Schema-guided normalization of node specs.
//!
//! Tool-calling LLMs occasionally emit slightly malformed JSON for node specs
//! — e.g. wrapping an array as `{"item": X}` or rendering numbers as strings.
//! Because [`crate::registry::NodeFactory::build`] deserializes the spec
//! into a typed config, these pathologies fail at the tool boundary with an
//! opaque "invalid type" error.
//!
//! [`normalize_against_schema`] walks the spec alongside the factory's own JSON
//! Schema and repairs the common cases. It is *schema-driven*: coercion is only
//! applied where the schema declares the target type, so well-formed specs pass
//! through unchanged and we never guess.

use serde_json::Value;

/// Normalize common LLM-emitted spec pathologies against a node factory's JSON
/// Schema (the `serde_json::Value` form of a `schemars::Schema`).
///
/// Repairs:
/// - `array` field given as `{"item": X}` / `{"items": X}` / `{"value": X}` →
///   `[X]` (or `X` when it is already an array).
/// - `array` field given as a numeric-keyed object `{"0": .., "1": ..}` → array
///   in index order.
/// - `number` / `integer` field given as a numeric string (`"200"`) → number,
///   preserving integer-ness so `usize`/integer targets still deserialize.
/// - `boolean` field given as `"true"` / `"false"` → bool.
///
/// `Option<T>` (`anyOf: [T, null]`) passes `null` through untouched. `$ref` is
/// resolved against the root schema's `$defs` / `definitions`. Anything not
/// matching a known schema type is returned unchanged — serde remains the final
/// authority.
pub fn normalize_against_schema(spec: Value, root_schema: &Value) -> Value {
    normalize(spec, root_schema, root_schema)
}

fn normalize(value: Value, schema: &Value, root: &Value) -> Value {
    let schema = resolve_ref(schema, root);

    // anyOf / oneOf — overwhelmingly `Option<T>` in this codebase.
    if let Some(repaired) = normalize_union(&value, schema, root) {
        return repaired;
    }
    // allOf — apply every subschema in sequence.
    if let Some(all) = schema.get("allOf").and_then(|v| v.as_array()) {
        let mut acc = value;
        for sub in all {
            acc = normalize(acc, sub, root);
        }
        return acc;
    }

    match schema_type(schema) {
        Some("array") => normalize_array(value, schema, root),
        Some("object") => normalize_object(value, schema, root),
        Some("number") | Some("integer") => coerce_number(value),
        Some("boolean") => coerce_bool(value),
        _ => value,
    }
}

/// Handle `anyOf` / `oneOf`. `null` is passed through (every union we emit is an
/// `Option<T>` that admits `null`); otherwise we recurse with the first
/// non-`null` branch.
fn normalize_union(value: &Value, schema: &Value, root: &Value) -> Option<Value> {
    let subs = schema
        .get("anyOf")
        .and_then(|v| v.as_array())
        .or_else(|| schema.get("oneOf").and_then(|v| v.as_array()))?;

    if value.is_null() {
        return Some(Value::Null);
    }
    for sub in subs {
        if !is_null_type(sub) {
            return Some(normalize(value.clone(), sub, root));
        }
    }
    Some(value.clone())
}

fn normalize_array(value: Value, schema: &Value, root: &Value) -> Value {
    let items = schema.get("items");
    match &value {
        Value::Array(_) => {
            let Value::Array(arr) = value else {
                return value;
            };
            Value::Array(
                arr.into_iter()
                    .map(|e| match items {
                        Some(it) => normalize(e, it, root),
                        None => e,
                    })
                    .collect(),
            )
        }
        Value::Object(obj) => {
            // Single-key wrapper: {"item": X} / {"items": X} / {"value": X}.
            if obj.len() == 1 {
                let (key, inner) = obj.iter().next().expect("len == 1");
                if matches!(key.as_str(), "item" | "items" | "value") {
                    let repaired = match items {
                        Some(it) => normalize(inner.clone(), it, root),
                        None => inner.clone(),
                    };
                    return if repaired.is_array() {
                        repaired
                    } else {
                        Value::Array(vec![repaired])
                    };
                }
            }
            // Numeric-keyed object: {"0": .., "1": .., ..}.
            if obj.keys().all(|k| k.parse::<usize>().is_ok()) {
                let mut indexed: Vec<(usize, &Value)> = obj
                    .iter()
                    .map(|(k, v)| (k.parse::<usize>().expect("numeric key"), v))
                    .collect();
                indexed.sort_by_key(|(i, _)| *i);
                return Value::Array(
                    indexed
                        .into_iter()
                        .map(|(_, v)| match items {
                            Some(it) => normalize(v.clone(), it, root),
                            None => v.clone(),
                        })
                        .collect(),
                );
            }
            value
        }
        _ => value,
    }
}

fn normalize_object(value: Value, schema: &Value, root: &Value) -> Value {
    let Some(Value::Object(prop_schemas)) = schema.get("properties") else {
        return value;
    };
    let Value::Object(mut obj) = value else {
        return value;
    };
    for (key, sub) in prop_schemas {
        if let Some(slot) = obj.get_mut(key) {
            let owned = std::mem::take(slot);
            *slot = normalize(owned, sub, root);
        }
    }
    Value::Object(obj)
}

fn coerce_number(value: Value) -> Value {
    let Value::String(s) = &value else {
        return value;
    };
    // Preserve integer-ness: a float-stored whole number won't deserialize
    // into `usize`/integer targets, so try integer parses first.
    if let Ok(i) = s.parse::<i64>() {
        return Value::Number(serde_json::Number::from(i));
    }
    if let Ok(u) = s.parse::<u64>() {
        return Value::Number(serde_json::Number::from(u));
    }
    if let Ok(f) = s.parse::<f64>()
        && let Some(n) = serde_json::Number::from_f64(f)
    {
        return Value::Number(n);
    }
    value
}

fn coerce_bool(value: Value) -> Value {
    if let Value::String(s) = &value {
        return match s.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => value,
        };
    }
    value
}

/// Resolve a `$ref` (`"#/$defs/Foo"` / `"#/definitions/Foo"`) against the root
/// schema. Returns the original schema if the reference cannot be resolved, so
/// a dangling ref never crashes normalization.
fn resolve_ref<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    let Some(ref_path) = schema.get("$ref").and_then(|v| v.as_str()) else {
        return schema;
    };
    let stripped = ref_path.strip_prefix('#').unwrap_or(ref_path);
    let stripped = stripped.strip_prefix('/').unwrap_or(stripped);
    if stripped.is_empty() {
        return root;
    }
    let mut cur = root;
    for part in stripped.split('/') {
        let next = match part.parse::<usize>() {
            Ok(i) => cur.get(i),
            Err(_) => cur.get(part),
        };
        cur = match next {
            Some(v) => v,
            None => return schema,
        };
    }
    // The target may itself be a `$ref`.
    resolve_ref(cur, root)
}

/// The schema's declared JSON type, preferring the non-`null` member when the
/// schema lists several (e.g. `["number", "null"]`).
fn schema_type(schema: &Value) -> Option<&str> {
    match schema.get("type")? {
        Value::String(s) => Some(s.as_str()),
        Value::Array(arr) => arr.iter().filter_map(|v| v.as_str()).find(|t| *t != "null"),
        _ => None,
    }
}

fn is_null_type(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(s)) => s == "null",
        Some(Value::Array(arr)) => arr.iter().all(|v| v.as_str() == Some("null")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_against_schema as norm;
    use serde_json::json;

    fn array_number_schema() -> serde_json::Value {
        json!({ "type": "object", "properties": {
            "m": { "type": "array", "items": { "type": "number" } }
        }})
    }

    /// Compare a JSON array of numbers by f64 value (the normalizer preserves
    /// integer-ness, so `1` and `1.0` must compare equal — both deserialize to
    /// the same `f64`).
    fn assert_num_array(val: &serde_json::Value, expected: &[f64]) {
        let arr = val
            .as_array()
            .unwrap_or_else(|| panic!("not an array: {val}"));
        assert_eq!(arr.len(), expected.len(), "length mismatch for {val}");
        for (got, want) in arr.iter().zip(expected) {
            assert_eq!(
                got.as_f64()
                    .unwrap_or_else(|| panic!("not a number: {got}")),
                *want
            );
        }
    }

    #[test]
    fn single_key_item_wrapper_unwraps_to_array() {
        let schema = array_number_schema();
        let spec = json!({ "m": { "item": "23960350" } });
        let out = norm(spec, &schema);
        assert_num_array(&out["m"], &[23960350.0]);
    }

    #[test]
    fn single_key_items_wrapper_with_array_inner() {
        let schema = array_number_schema();
        let spec = json!({ "m": { "items": [1, 2] } });
        let out = norm(spec, &schema);
        assert_num_array(&out["m"], &[1.0, 2.0]);
    }

    #[test]
    fn numeric_keyed_object_becomes_array() {
        let schema = array_number_schema();
        let spec = json!({ "m": { "0": "1", "1": "2", "2": "3" } });
        let out = norm(spec, &schema);
        assert_num_array(&out["m"], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn numeric_string_for_integer_preserves_interness() {
        // usize target: a float-stored 200 would fail to deserialize, so the
        // normalizer must keep it an integer.
        let schema = json!({ "type": "object", "properties": {
            "n_blocks": { "type": "integer" }
        }});
        let out = norm(json!({ "n_blocks": "200" }), &schema);
        assert_eq!(out["n_blocks"], json!(200));
        assert!(out["n_blocks"].as_u64() == Some(200));
        assert!(out["n_blocks"].as_f64().is_some());
    }

    #[test]
    fn numeric_string_for_number_float() {
        let schema = json!({ "type": "object", "properties": {
            "intercept": { "type": "number" }
        }});
        let out = norm(json!({ "intercept": "1.5" }), &schema);
        assert_eq!(out["intercept"], json!(1.5));
    }

    #[test]
    fn boolean_string_coerced() {
        let schema = json!({ "type": "object", "properties": {
            "flag": { "type": "boolean" }
        }});
        let out = norm(json!({ "flag": "true" }), &schema);
        assert_eq!(out["flag"], json!(true));
    }

    #[test]
    fn option_null_passes_through() {
        let schema = json!({ "type": "object", "properties": {
            "opt": { "anyOf": [{ "type": "number" }, { "type": "null" }] }
        }});
        let out = norm(json!({ "opt": null }), &schema);
        assert_eq!(out["opt"], json!(null));
    }

    #[test]
    fn option_numeric_string_coerced_via_non_null_branch() {
        let schema = json!({ "type": "object", "properties": {
            "opt": { "anyOf": [{ "type": "number" }, { "type": "null" }] }
        }});
        let out = norm(json!({ "opt": "42" }), &schema);
        assert_eq!(out["opt"], json!(42));
    }

    #[test]
    fn nested_object_recurses() {
        let schema = json!({ "type": "object", "properties": {
            "outer": { "type": "object", "properties": {
                "inner": { "type": "array", "items": { "type": "number" } }
            }}
        }});
        let out = norm(json!({ "outer": { "inner": { "item": "7" } } }), &schema);
        assert_num_array(&out["outer"]["inner"], &[7.0]);
    }

    #[test]
    fn ref_resolved_against_defs() {
        let schema = json!({
            "$defs": { "Nums": { "type": "array", "items": { "type": "number" } } },
            "type": "object",
            "properties": { "m": { "$ref": "#/$defs/Nums" } }
        });
        let out = norm(json!({ "m": { "item": "9" } }), &schema);
        assert_num_array(&out["m"], &[9.0]);
    }

    #[test]
    fn well_formed_spec_unchanged() {
        let schema = json!({ "type": "object", "properties": {
            "m": { "type": "array", "items": { "type": "number" } },
            "n_blocks": { "type": "integer" }
        }});
        let spec = json!({ "m": [1000000.0], "n_blocks": 200 });
        let out = norm(spec.clone(), &schema);
        assert_eq!(out, spec);
    }

    #[test]
    fn unknown_keys_left_untouched() {
        let schema = json!({ "type": "object", "properties": {
            "m": { "type": "array", "items": { "type": "number" } }
        }});
        let spec = json!({ "m": { "item": "1" }, "extra": "leave-me" });
        let out = norm(spec, &schema);
        assert_eq!(out["extra"], json!("leave-me"));
        assert_num_array(&out["m"], &[1.0]);
    }
}
