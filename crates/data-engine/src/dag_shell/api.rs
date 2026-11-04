//! Host-side staging API for dag_shell scripts.
//!
//! The functions listed in [`HOST_FUNCTIONS`] are the only graph surface a
//! script can reach. They mirror the previous Rhai-registered functions: all
//! mutations stage into [`ScriptState`] and queries read the immutable
//! execution-start snapshot. [`Interp::call_host`](super::interp::Interp)
//! dispatches by name and performs the argument marshalling.

use std::sync::{Arc, Mutex};

use rustpython_ast::TextSize;

use super::interp::{ErrKind, EvalError, EvalResult, Value, json_to_value, value_to_json};
use super::{GraphEditOp, MAX_API_OPERATIONS, MAX_MAP_SIZE, ScriptState};

pub(crate) const HOST_FUNCTIONS: &[&str] = &[
    "node",
    "update_node",
    "remove_node",
    "edge",
    "remove_edge",
    "commit",
    "has_node",
    "node_info",
    "node_kinds",
    "node_spec",
    "node_ports",
    "node_ports_for_spec",
    "graph_summary",
    "dot",
];

pub(crate) fn is_host_function(name: &str) -> bool {
    HOST_FUNCTIONS.contains(&name)
}

pub(crate) const API_LIMIT_MESSAGE: &str = "DAG Shell API limit exceeded";

pub(crate) type Offset = TextSize;

pub(crate) fn stage(
    state: &Arc<Mutex<ScriptState>>,
    operation: GraphEditOp,
    offset: Offset,
) -> EvalResult<()> {
    let mut state = lock(state, offset)?;
    if state.committed {
        return Err(runtime_error(
            offset,
            "commit() was already called; graph operations cannot be staged afterward",
        ));
    }
    state.api_operations += 1;
    if state.api_operations > MAX_API_OPERATIONS {
        return Err(limit_error(offset, API_LIMIT_MESSAGE));
    }
    state.operations.push(operation);
    Ok(())
}

pub(crate) fn begin_query(state: &Arc<Mutex<ScriptState>>, offset: Offset) -> EvalResult<()> {
    let mut state = lock(state, offset)?;
    state.api_operations += 1;
    if state.api_operations > MAX_API_OPERATIONS {
        return Err(limit_error(offset, API_LIMIT_MESSAGE));
    }
    Ok(())
}

fn lock(
    state: &Arc<Mutex<ScriptState>>,
    offset: Offset,
) -> Result<std::sync::MutexGuard<'_, ScriptState>, EvalError> {
    state
        .lock()
        .map_err(|_| runtime_error(offset, "script state poisoned"))
}

pub(crate) fn runtime_error(offset: Offset, message: impl Into<String>) -> EvalError {
    EvalError {
        kind: ErrKind::Runtime,
        message: message.into(),
        offset,
    }
}

pub(crate) fn limit_error(offset: Offset, message: impl Into<String>) -> EvalError {
    EvalError {
        kind: ErrKind::Limit,
        message: message.into(),
        offset,
    }
}

/// Count JSON object properties recursively, in the same units the previous
/// Rhai integration used for its nested object-map limit.
pub(crate) fn json_map_property_count(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(fields) => {
            fields.len() + fields.values().map(json_map_property_count).sum::<usize>()
        }
        serde_json::Value::Array(items) => items.iter().map(json_map_property_count).sum(),
        _ => 0,
    }
}

/// Convert a script value into a staged JSON payload (node specs, logical
/// graphs). Oversized/unsupported values surface as limit errors, matching the
/// previous `require_json` behavior.
pub(crate) fn spec_to_json(value: &Value, offset: Offset) -> EvalResult<serde_json::Value> {
    value_to_json(value, 0).ok_or_else(|| limit_error(offset, API_LIMIT_MESSAGE))
}

/// Convert snapshot/registry JSON into script values.
pub(crate) fn json_to_script_value(json: &serde_json::Value) -> Value {
    json_to_value(json)
}

/// Deserialize a logical graph from staged JSON.
pub(crate) fn parse_logical_graph(
    json: serde_json::Value,
    offset: Offset,
) -> EvalResult<crate::dag::LogicalGraph> {
    let requested_map_properties = json_map_property_count(&json);
    if requested_map_properties > MAX_MAP_SIZE {
        return Err(limit_error(
            offset,
            format!(
                "logical graph object-map budget exceeded: \
                 limit={MAX_MAP_SIZE}, requested={requested_map_properties} \
                 (unit: recursively nested map properties)"
            ),
        ));
    }
    serde_json::from_value::<crate::dag::LogicalGraph>(json)
        .map_err(|error| runtime_error(offset, format!("invalid logical graph: {error}")))
}

/// String argument coercion for host functions.
pub(crate) fn str_arg(value: &Value, offset: Offset) -> EvalResult<String> {
    match value {
        Value::Str(text) => Ok(text.to_string()),
        other => Err(EvalError {
            kind: ErrKind::Type,
            message: format!("expected str, got '{}'", other.type_name()),
            offset,
        }),
    }
}

/// Port coercion (u8, like the previous `require_ports`).
pub(crate) fn port_arg(value: &Value, label: &str, offset: Offset) -> EvalResult<u8> {
    let as_int = match value {
        Value::Int(port) => *port,
        other => {
            return Err(EvalError {
                kind: ErrKind::Type,
                message: format!("{label} must be an int, got '{}'", other.type_name()),
                offset,
            });
        }
    };
    u8::try_from(as_int).map_err(|_| EvalError {
        kind: ErrKind::Value,
        message: format!("invalid {label} {as_int}"),
        offset,
    })
}
