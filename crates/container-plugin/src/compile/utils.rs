use std::collections::BTreeMap;

use super::error::{Error, Result};
use crate::node_definition::{ParamSpec, ParamType};

/// Validate that a resolved param value's JSON shape matches the declared
/// [`ParamType`]. Strict on purpose: `normalize_against_schema` repairs the
/// common LLM pathologies upstream; anything still mismatched here is
/// genuinely wrong and fails closed. Floats with integral values (`5.0`)
/// do not satisfy `Int`.
///
/// # Arguments
///
/// * `kind` — node kind, copied verbatim into the error so failures
///   surfacing through the loader or `nodedev validate` name the node.
/// * `name` — param name, carried into the error alongside `kind`.
/// * `spec` — the declaration being checked against; supplies `r#type`.
/// * `value` — the resolved value (user-submitted or default). Borrowed:
///   the offending value is cloned only on the error path, so the success
///   path costs nothing and the caller keeps the value for later use.
pub fn check_type(
    kind: &str,
    name: &str,
    spec: &ParamSpec,
    value: &serde_json::Value,
) -> Result<()> {
    let ok = match spec.r#type {
        ParamType::Bool => value.is_boolean(),
        // `as_i64` rejects f64-backed numbers; the u64 arm admits integers
        // above i64::MAX.
        ParamType::Int => value.as_i64().is_some() || value.as_u64().is_some(),
        ParamType::Number => value.is_number(),
        ParamType::String => value.is_string(),
        ParamType::StringArray => value
            .as_array()
            .is_some_and(|items| items.iter().all(serde_json::Value::is_string)),
    };
    if ok {
        Ok(())
    } else {
        Err(Error::TypeMismatch {
            kind: kind.into(),
            name: name.into(),
            expected: spec.r#type,
            value: value.clone(),
        })
    }
}

/// Validate declared bounds against a resolved value whose type already
/// passed [`check_type`]. Numeric bounds read via `as_f64` (NaN makes every
/// comparison false, so malformed numbers reject naturally); length bounds
/// apply to `StringArray` items.
///
/// # Arguments
///
/// * `kind` — node kind, copied verbatim into the error.
/// * `name` — param name, carried into the error alongside `kind`.
/// * `spec` — the declaration being checked against; supplies the six
///   bounds.
/// * `value` — the resolved value, borrowed: violations record only the
///   checked number or length, never the whole value.
pub fn check_bounds(
    kind: &str,
    name: &str,
    spec: &ParamSpec,
    value: &serde_json::Value,
) -> Result<()> {
    let number = value.as_f64();
    let length = value.as_array().map_or(0, Vec::len) as f64;

    let violation = |bound: &'static str, checked: f64| {
        Err(Error::BoundViolation {
            kind: kind.into(),
            name: name.into(),
            bound,
            value: checked,
        })
    };

    if let Some(min) = spec.min
        && let Some(checked) = number
        && !(checked >= min)
    {
        return violation("minimum", checked);
    }
    if let Some(max) = spec.max
        && let Some(checked) = number
        && !(checked <= max)
    {
        return violation("maximum", checked);
    }
    if let Some(min) = spec.exclusive_min
        && let Some(checked) = number
        && !(checked > min)
    {
        return violation("exclusiveMinimum", checked);
    }
    if let Some(max) = spec.exclusive_max
        && let Some(checked) = number
        && !(checked < max)
    {
        return violation("exclusiveMaximum", checked);
    }
    if let Some(min) = spec.min_len
        && !(length >= min as f64)
    {
        return violation("minItems", length);
    }
    if let Some(max) = spec.max_len
        && !(length <= max as f64)
    {
        return violation("maxItems", length);
    }
    Ok(())
}

/// Validate `requires` gates over the fully resolved param map: when a
/// param resolves to `true`, every bool it requires must also resolve to
/// `true`. Must run after the whole map is built — a target may be
/// satisfied (or not) by its default rather than by a submitted value, so
/// checking per-param during resolution would miss cross-param verdicts.
/// Targets being declared bools is guaranteed by load-time validation.
///
/// # Arguments
///
/// * `node` — the definition supplying param declarations and the kind
///   used in errors.
/// * `resolved` — the complete value map (submitted values merged with
///   defaults), already type- and bounds-checked.
pub fn check_requires(
    node: &crate::node_definition::NodeDefinition,
    resolved: &BTreeMap<String, serde_json::Value>,
) -> Result<()> {
    let activated = serde_json::Value::Bool(true);
    for (name, spec) in &node.params {
        if resolved.get(name) != Some(&activated) {
            continue;
        }
        for target in &spec.requires {
            if resolved.get(target) != Some(&activated) {
                return Err(Error::RequiresUnmet {
                    kind: node.kind.clone(),
                    name: name.clone(),
                    target: target.clone(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(r#type: ParamType) -> ParamSpec {
        ParamSpec {
            r#type,
            default: None,
            doc: None,
            min: None,
            max: None,
            exclusive_min: None,
            exclusive_max: None,
            min_len: None,
            max_len: None,
            requires: Vec::new(),
        }
    }

    #[test]
    fn type_check_accepts_matching_shapes() {
        assert!(check_type("k", "p", &spec(ParamType::Bool), &json!(true)).is_ok());
        assert!(check_type("k", "p", &spec(ParamType::Int), &json!(5)).is_ok());
        assert!(check_type("k", "p", &spec(ParamType::Int), &json!(u64::MAX)).is_ok());
        assert!(check_type("k", "p", &spec(ParamType::Number), &json!(2.5)).is_ok());
        assert!(check_type("k", "p", &spec(ParamType::String), &json!("x")).is_ok());
        assert!(check_type("k", "p", &spec(ParamType::StringArray), &json!(["a", "b"])).is_ok());
    }

    #[test]
    fn type_check_rejects_mismatches_without_coercion() {
        // Integral floats do not satisfy Int.
        assert!(check_type("k", "p", &spec(ParamType::Int), &json!(5.0)).is_err());
        // Numeric strings are not numbers.
        assert!(check_type("k", "p", &spec(ParamType::Number), &json!("5")).is_err());
        // String booleans are not bools.
        assert!(check_type("k", "p", &spec(ParamType::Bool), &json!("true")).is_err());
        // Mixed arrays are not string arrays.
        assert!(check_type("k", "p", &spec(ParamType::StringArray), &json!(["a", 1])).is_err());
        // Null is nothing.
        for ty in [
            ParamType::Bool,
            ParamType::Int,
            ParamType::Number,
            ParamType::String,
        ] {
            assert!(check_type("k", "p", &spec(ty), &serde_json::Value::Null).is_err());
        }
    }

    #[test]
    fn type_check_error_carries_context() {
        let error = check_type(
            "mtag_container",
            "force",
            &spec(ParamType::Bool),
            &json!("yes"),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("mtag_container"));
        assert!(message.contains("force"));
        assert!(message.contains("expects boolean"));
        assert!(message.contains("yes"));
    }

    #[test]
    fn bounds_check_numeric_and_length() {
        let mut number_spec = spec(ParamType::Number);
        number_spec.exclusive_min = Some(0.0);
        number_spec.max = Some(24.0);
        assert!(check_bounds("k", "p", &number_spec, &json!(1.0)).is_ok());
        assert!(check_bounds("k", "p", &number_spec, &json!(24.0)).is_ok());
        assert!(check_bounds("k", "p", &number_spec, &json!(0.0)).is_err());
        assert!(check_bounds("k", "p", &number_spec, &json!(25.0)).is_err());

        let mut array_spec = spec(ParamType::StringArray);
        array_spec.min_len = Some(1);
        array_spec.max_len = Some(8);
        assert!(check_bounds("k", "p", &array_spec, &json!(["a"])).is_ok());
        assert!(check_bounds("k", "p", &array_spec, &json!([])).is_err());
        let nine = vec!["a"; 9];
        assert!(check_bounds("k", "p", &array_spec, &json!(nine)).is_err());
    }

    #[test]
    fn bounds_violation_names_the_bound() {
        let mut number_spec = spec(ParamType::Number);
        number_spec.exclusive_min = Some(0.0);
        let error = check_bounds("k", "p", &number_spec, &json!(0.0)).unwrap_err();
        assert!(error.to_string().contains("exclusiveMinimum"));
    }

    #[test]
    fn bounds_are_inert_for_non_matching_shapes() {
        // Numeric bounds must not fire on arrays and length bounds must not
        // fire on scalars: each bound family only applies to its shape.
        let mut mixed = spec(ParamType::StringArray);
        mixed.min = Some(10.0);
        mixed.min_len = Some(1);
        assert!(check_bounds("k", "p", &mixed, &json!(["a"])).is_ok());
    }

    fn node_with(params: Vec<(&str, ParamSpec)>) -> crate::node_definition::NodeDefinition {
        crate::node_definition::NodeDefinition {
            kind: "test_kind".into(),
            desc: String::new(),
            doc: String::new(),
            deprecated: false,
            timeout_secs: 60,
            artifact_prefix: None,
            ports: crate::node_definition::PortLayout {
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
            params: params
                .into_iter()
                .map(|(name, spec)| (name.into(), spec))
                .collect(),
            command: crate::node_definition::CommandSpec {
                interpreter: "sh".into(),
                argv: Vec::new(),
                script: None,
                env: Default::default(),
                files: Default::default(),
            },
            resources: Default::default(),
        }
    }

    fn bool_param() -> ParamSpec {
        spec(ParamType::Bool)
    }

    #[test]
    fn requires_passes_when_no_gates_are_open() {
        let node = node_with(vec![("force", bool_param())]);
        let mut resolved = BTreeMap::new();
        resolved.insert("force".into(), json!(false));
        assert!(check_requires(&node, &resolved).is_ok());
    }

    #[test]
    fn requires_fires_only_when_all_gates_are_true() {
        // mtag-shaped: equal_h2 requires force to be true.
        let mut equal_h2 = bool_param();
        equal_h2.requires = vec!["force".into()];
        let node = node_with(vec![("force", bool_param()), ("equal_h2", equal_h2)]);

        let mut satisfied = BTreeMap::new();
        satisfied.insert("force".into(), json!(true));
        satisfied.insert("equal_h2".into(), json!(true));
        assert!(check_requires(&node, &satisfied).is_ok());

        // equal_h2 alone, default for force = false -> gate unmet.
        let mut unmet = BTreeMap::new();
        unmet.insert("force".into(), json!(false));
        unmet.insert("equal_h2".into(), json!(true));
        let error = check_requires(&node, &unmet).unwrap_err();
        assert!(error.to_string().contains("requires `force`"));
    }

    #[test]
    fn requires_uses_defaulted_targets_not_just_submitted_values() {
        // target comes from default; we submit only the gate trigger.
        let mut gate = bool_param();
        gate.requires = vec!["target".into()];
        let node = node_with(vec![
            ("target", bool_param()), // no default ⇒ false at runtime
            ("gate", gate),
        ]);

        let mut triggered = BTreeMap::new();
        triggered.insert("target".into(), json!(false));
        triggered.insert("gate".into(), json!(true));
        assert!(check_requires(&node, &triggered).is_err());
    }
}
