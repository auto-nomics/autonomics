//! Sandboxed Rhai orchestration for transactional DAG edits.
//!
//! Scripts see an immutable snapshot of the DAG and stage graph operations.
//! Nothing in this module touches the filesystem, network, environment, or
//! process APIs; only the explicitly registered graph functions are available.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rhai::packages::{BasicArrayPackage, BasicMapPackage, CorePackage, LogicPackage, Package};
use rhai::{Dynamic, Engine, Map, Position};
use serde::Serialize;

use crate::dag::{DAG, LogicalGraph};
use crate::node_registry::registry::NodeRegistry;

pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;
pub const MAX_API_OPERATIONS: usize = 1_000;
pub const MAX_RHAI_OPERATIONS: u64 = 100_000;
pub const MAX_CALL_LEVELS: usize = 64;
pub const MAX_ARRAY_SIZE: usize = 10_000;
pub const MAX_MAP_SIZE: usize = 1_000;
pub const MAX_STRING_SIZE: usize = 64 * 1024;
pub const MAX_EXPRESSION_DEPTH: usize = 64;
pub const DEFAULT_TIMEOUT_MS: u64 = 1_000;
pub const MAX_TIMEOUT_MS: u64 = 5_000;
pub const MAX_RESULT_BYTES: usize = 16 * 1024;

const TIMEOUT_TOKEN: &str = "__DAG_SHELL_TIMEOUT__";
const LIMIT_TOKEN: &str = "__DAG_SHELL_LIMIT__";
type ShellResult<T> = Result<T, Box<rhai::EvalAltResult>>;

#[derive(Debug, Clone)]
pub enum GraphEditOp {
    AddNode {
        id: String,
        kind: String,
        spec: serde_json::Value,
    },
    UpdateNode {
        id: String,
        spec: serde_json::Value,
    },
    RemoveNode {
        id: String,
    },
    AddEdge {
        from: String,
        from_port: u8,
        to: String,
        to_port: u8,
    },
    RemoveEdge {
        from: String,
        from_port: u8,
        to: String,
        to_port: u8,
    },
    AddLogicalGraph {
        graph: LogicalGraph,
    },
}

impl GraphEditOp {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AddNode { .. } => "add_node",
            Self::UpdateNode { .. } => "update_node",
            Self::RemoveNode { .. } => "remove_node",
            Self::AddEdge { .. } => "add_edge",
            Self::RemoveEdge { .. } => "remove_edge",
            Self::AddLogicalGraph { .. } => "add_logical_graph",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationTrace {
    pub index: usize,
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DagShellError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DagShellOutcome {
    pub ok: bool,
    pub applied: bool,
    pub dry_run: bool,
    pub committed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub operations: Vec<OperationTrace>,
    pub graph: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DagShellError>,
}

#[derive(Debug, Clone)]
struct ShellNode {
    kind: String,
    spec: Option<serde_json::Value>,
    ports: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct DagShellSnapshot {
    nodes: BTreeMap<String, ShellNode>,
    edges: Vec<serde_json::Value>,
    dot: String,
    logical_graph_count: usize,
}

impl DagShellSnapshot {
    pub(crate) fn from_dag(dag: &DAG) -> Self {
        let snapshot = dag.tui_snapshot();
        let mut nodes = BTreeMap::new();
        for node in &snapshot.nodes {
            let retained = dag.node_spec(&node.id);
            nodes.insert(
                node.id.clone(),
                ShellNode {
                    kind: node.kind.clone(),
                    spec: retained.map(|(_, spec)| spec),
                    ports: serde_json::to_value(node)
                        .unwrap_or_else(|_| serde_json::json!({"id": node.id, "kind": node.kind})),
                },
            );
        }
        Self {
            nodes,
            edges: snapshot
                .edges
                .iter()
                .flat_map(serde_json::to_value)
                .collect(),
            dot: dag.to_dot(),
            logical_graph_count: dag.logical_graphs().len(),
        }
    }

    fn summary_dynamic(&self) -> Map {
        let mut map = Map::new();
        map.insert("node_count".into(), Dynamic::from(self.nodes.len() as i64));
        map.insert("edge_count".into(), Dynamic::from(self.edges.len() as i64));
        map.insert(
            "logical_graph_count".into(),
            Dynamic::from(self.logical_graph_count as i64),
        );
        map
    }
}

#[derive(Default)]
struct ScriptState {
    operations: Vec<GraphEditOp>,
    api_operations: usize,
    committed: bool,
}

#[derive(Debug)]
pub(crate) struct ScriptExecution {
    pub operations: Vec<GraphEditOp>,
    pub committed: bool,
    pub result: Dynamic,
}

pub fn normalize_timeout(timeout_ms: Option<u64>) -> Result<Duration, DagShellError> {
    let millis = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    if millis == 0 || millis > MAX_TIMEOUT_MS {
        return Err(DagShellError {
            code: "invalid_operation".into(),
            message: format!("timeout_ms must be between 1 and {MAX_TIMEOUT_MS}"),
            operation_index: None,
            line: None,
            column: None,
        });
    }
    Ok(Duration::from_millis(millis))
}

pub(crate) fn execute_script(
    script: &str,
    timeout: Duration,
    snapshot: &DagShellSnapshot,
    registry: &Arc<NodeRegistry>,
) -> Result<ScriptExecution, DagShellError> {
    if script.len() > MAX_SCRIPT_BYTES {
        return Err(error(
            "script_too_large",
            format!("script exceeds {MAX_SCRIPT_BYTES} bytes"),
            None,
            None,
        ));
    }

    let state = Arc::new(std::sync::Mutex::new(ScriptState::default()));
    let snapshot = Arc::new(snapshot.clone());
    let registry = Arc::clone(registry);
    let mut engine = Engine::new_raw();
    engine
        .register_global_module(CorePackage::new().as_shared_module())
        .register_global_module(LogicPackage::new().as_shared_module())
        .register_global_module(BasicArrayPackage::new().as_shared_module())
        .register_global_module(BasicMapPackage::new().as_shared_module())
        .set_max_operations(MAX_RHAI_OPERATIONS)
        .set_max_call_levels(MAX_CALL_LEVELS)
        .set_max_array_size(MAX_ARRAY_SIZE)
        .set_max_map_size(MAX_MAP_SIZE)
        .set_max_string_size(MAX_STRING_SIZE)
        .set_max_expr_depths(MAX_EXPRESSION_DEPTH, MAX_EXPRESSION_DEPTH);
    engine.disable_symbol("import");

    let started = Instant::now();
    engine
        .on_progress(move |_| (started.elapsed() >= timeout).then(|| Dynamic::from(TIMEOUT_TOKEN)));

    register_graph_functions(&mut engine, Arc::clone(&state), snapshot, registry);

    let ast = engine.compile(script).map_err(|error| {
        let position = error.position();
        error_at("parse_error", error.to_string(), Some(position), None)
    })?;

    let evaluated = std::panic::catch_unwind(AssertUnwindSafe(|| engine.eval_ast::<Dynamic>(&ast)));
    let result = match evaluated {
        Ok(result) => result.map_err(|error| rhai_error(error, None)),
        Err(panic) => Err(error(
            "runtime_error",
            format!("script panicked: {panic:?}"),
            None,
            None,
        )),
    }?;

    let serialized_value = dynamic_to_json(&result, 0).ok_or_else(|| {
        error(
            "limit_exceeded",
            "script result contains an unsupported or oversized value",
            None,
            None,
        )
    })?;
    let serialized = serde_json::to_string(&serialized_value).map_err(|serde_error| {
        error(
            "runtime_error",
            format!("cannot serialize script result: {serde_error}"),
            None,
            None,
        )
    })?;
    if serialized.len() > MAX_RESULT_BYTES {
        return Err(error(
            "limit_exceeded",
            format!("script result exceeds {MAX_RESULT_BYTES} bytes"),
            None,
            None,
        ));
    }

    let state = state
        .lock()
        .map_err(|_| error("runtime_error", "script state poisoned", None, None))?;
    Ok(ScriptExecution {
        operations: state.operations.clone(),
        committed: state.committed,
        result,
    })
}

fn register_graph_functions(
    engine: &mut Engine,
    state: Arc<std::sync::Mutex<ScriptState>>,
    snapshot: Arc<DagShellSnapshot>,
    registry: Arc<NodeRegistry>,
) {
    let s = Arc::clone(&state);
    engine.register_fn("node", move |id: String, kind: String, spec: Dynamic| {
        let spec = require_json(spec)?;
        stage(&s, GraphEditOp::AddNode { id, kind, spec })
    });
    let s = Arc::clone(&state);
    engine.register_fn("update_node", move |id: String, spec: Dynamic| {
        let spec = require_json(spec)?;
        stage(&s, GraphEditOp::UpdateNode { id, spec })
    });
    let s = Arc::clone(&state);
    engine.register_fn("remove_node", move |id: String| {
        stage(&s, GraphEditOp::RemoveNode { id })
    });
    let s = Arc::clone(&state);
    engine.register_fn(
        "edge",
        move |from: String, from_port: i64, to: String, to_port: i64| {
            let (from_port, to_port) = require_ports(from_port, to_port)?;
            stage(
                &s,
                GraphEditOp::AddEdge {
                    from,
                    from_port,
                    to,
                    to_port,
                },
            )
        },
    );
    let s = Arc::clone(&state);
    engine.register_fn(
        "remove_edge",
        move |from: String, from_port: i64, to: String, to_port: i64| {
            let (from_port, to_port) = require_ports(from_port, to_port)?;
            stage(
                &s,
                GraphEditOp::RemoveEdge {
                    from,
                    from_port,
                    to,
                    to_port,
                },
            )
        },
    );
    let s = Arc::clone(&state);
    engine.register_fn("add_logical_graph", move |graph: Dynamic| {
        let graph = serde_json::from_value::<LogicalGraph>(require_json(graph)?)
            .map_err(|error| eval_error(format!("invalid logical graph: {error}")))?;
        stage(&s, GraphEditOp::AddLogicalGraph { graph })
    });
    let s = Arc::clone(&state);
    engine.register_fn("commit", move || {
        let mut state = lock(&s)?;
        state.api_operations += 1;
        state.committed = true;
        Ok::<_, Box<rhai::EvalAltResult>>(())
    });

    let (s, snap) = (Arc::clone(&state), Arc::clone(&snapshot));
    engine.register_fn("has_node", move |id: String| {
        begin_query(&s)?;
        Ok::<bool, Box<rhai::EvalAltResult>>(snap.nodes.contains_key(&id))
    });
    let (s, snap) = (Arc::clone(&state), Arc::clone(&snapshot));
    engine.register_fn("node_info", move |id: String| {
        begin_query(&s)?;
        let info: ShellResult<Dynamic> = snap.nodes.get(&id).map_or_else(
            || Err(eval_error(format!("unknown node `{id}`"))),
            |node| Ok(json_dynamic(&serde_json::json!({"id": id, "kind": node.kind, "spec": node.spec, "ports": node.ports}))),
        );
        info
    });
    let (s, snap) = (Arc::clone(&state), Arc::clone(&snapshot));
    engine.register_fn("node_kinds", move || {
        begin_query(&s)?;
        Ok::<Dynamic, Box<rhai::EvalAltResult>>(Dynamic::from(snapped_kinds(&snap)))
    });
    let (s, reg) = (Arc::clone(&state), Arc::clone(&registry));
    engine.register_fn("node_spec", move |kind: String| {
        begin_query(&s)?;
        let schema = reg
            .get_node_spec(&kind)
            .map_err(|e| eval_error(e.to_string()))?;
        Ok::<Dynamic, Box<rhai::EvalAltResult>>(json_dynamic(
            &serde_json::to_value(schema).map_err(|e| eval_error(e.to_string()))?,
        ))
    });
    let (s, reg) = (Arc::clone(&state), Arc::clone(&registry));
    engine.register_fn("node_ports", move |kind: String| {
        begin_query(&s)?;
        let ports = reg
            .get_node_ports(&kind)
            .map_err(|e| eval_error(e.to_string()))?;
        Ok::<Dynamic, Box<rhai::EvalAltResult>>(json_dynamic(
            &serde_json::to_value(ports).map_err(|e| eval_error(e.to_string()))?,
        ))
    });
    let (s, reg) = (Arc::clone(&state), Arc::clone(&registry));
    engine.register_fn("node_ports_for_spec", move |kind: String, spec: Dynamic| {
        begin_query(&s)?;
        let spec = require_json(spec)?;
        let ports = reg
            .get_node_ports_for_spec(&kind, spec)
            .map_err(|e| eval_error(e.to_string()))?;
        Ok::<Dynamic, Box<rhai::EvalAltResult>>(json_dynamic(
            &serde_json::to_value(ports).map_err(|e| eval_error(e.to_string()))?,
        ))
    });
    let (s, snap) = (Arc::clone(&state), Arc::clone(&snapshot));
    engine.register_fn("graph_summary", move || {
        begin_query(&s)?;
        Ok::<Dynamic, Box<rhai::EvalAltResult>>(Dynamic::from(snap.summary_dynamic()))
    });
    let (s, snap) = (Arc::clone(&state), Arc::clone(&snapshot));
    engine.register_fn("dot", move || {
        begin_query(&s)?;
        Ok::<String, Box<rhai::EvalAltResult>>(snap.dot.clone())
    });
}

fn stage(state: &Arc<std::sync::Mutex<ScriptState>>, operation: GraphEditOp) -> ShellResult<()> {
    let mut state = lock(state)?;
    if state.committed {
        return Err(eval_error(
            "commit() was already called; graph operations cannot be staged afterward",
        ));
    }
    state.api_operations += 1;
    if state.api_operations > MAX_API_OPERATIONS {
        return Err(eval_error(LIMIT_TOKEN));
    }
    state.operations.push(operation);
    Ok(())
}

fn begin_query(state: &Arc<std::sync::Mutex<ScriptState>>) -> ShellResult<()> {
    let mut state = lock(state)?;
    state.api_operations += 1;
    if state.api_operations > MAX_API_OPERATIONS {
        return Err(eval_error(LIMIT_TOKEN));
    }
    Ok(())
}

fn lock(
    state: &Arc<std::sync::Mutex<ScriptState>>,
) -> ShellResult<std::sync::MutexGuard<'_, ScriptState>> {
    state
        .lock()
        .map_err(|_| eval_error("script state poisoned"))
}

fn require_ports(from_port: i64, to_port: i64) -> ShellResult<(u8, u8)> {
    let from = u8::try_from(from_port)
        .map_err(|_| eval_error(format!("invalid from_port {from_port}")))?;
    let to = u8::try_from(to_port).map_err(|_| eval_error(format!("invalid to_port {to_port}")))?;
    Ok((from, to))
}

fn require_json(value: Dynamic) -> ShellResult<serde_json::Value> {
    dynamic_to_json(&value, 0).ok_or_else(|| eval_error(LIMIT_TOKEN))
}

fn eval_error(message: impl Into<String>) -> Box<rhai::EvalAltResult> {
    rhai::EvalAltResult::ErrorRuntime(Dynamic::from(message.into()), Position::NONE).into()
}

fn json_dynamic(value: &serde_json::Value) -> Dynamic {
    match value {
        serde_json::Value::Null => Dynamic::UNIT,
        serde_json::Value::Bool(value) => Dynamic::from(*value),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(Dynamic::from)
            .or_else(|| value.as_f64().map(Dynamic::from))
            .unwrap_or(Dynamic::UNIT),
        serde_json::Value::String(value) => Dynamic::from(value.clone()),
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_dynamic)
            .collect::<rhai::Array>()
            .into(),
        serde_json::Value::Object(values) => {
            let mut map = Map::new();
            for (key, value) in values {
                map.insert(key.clone().into(), json_dynamic(value));
            }
            Dynamic::from(map)
        }
    }
}

pub(crate) fn dynamic_to_json(value: &Dynamic, depth: usize) -> Option<serde_json::Value> {
    if depth > 64 {
        return None;
    }
    if value.is_unit() {
        Some(serde_json::Value::Null)
    } else if let Some(value) = value.clone().try_cast::<bool>() {
        Some(value.into())
    } else if let Some(value) = value.clone().try_cast::<i64>() {
        Some(value.into())
    } else if let Some(value) = value.clone().try_cast::<f64>() {
        serde_json::Number::from_f64(value).map(serde_json::Value::Number)
    } else if let Some(value) = value.clone().try_cast::<String>() {
        if value.len() > MAX_STRING_SIZE {
            return None;
        }
        Some(value.into())
    } else if let Some(values) = value.clone().try_cast::<rhai::Array>() {
        if values.len() > MAX_ARRAY_SIZE {
            return None;
        }
        values
            .iter()
            .map(|value| dynamic_to_json(value, depth + 1))
            .collect()
    } else if let Some(values) = value.clone().try_cast::<Map>() {
        if values.len() > MAX_MAP_SIZE {
            return None;
        }
        let mut map = serde_json::Map::new();
        for (key, value) in values {
            map.insert(key.to_string(), dynamic_to_json(&value, depth + 1)?);
        }
        Some(map.into())
    } else {
        None
    }
}

fn snapped_kinds(snapshot: &DagShellSnapshot) -> rhai::Array {
    let kinds: BTreeSet<String> = snapshot
        .nodes
        .values()
        .map(|node| node.kind.clone())
        .collect();
    kinds.into_iter().map(Dynamic::from).collect()
}

fn error(
    code: &str,
    message: impl Into<String>,
    line: Option<u32>,
    column: Option<u32>,
) -> DagShellError {
    DagShellError {
        code: code.into(),
        message: message.into(),
        operation_index: None,
        line,
        column,
    }
}

fn error_at(
    code: &str,
    message: impl Into<String>,
    position: Option<Position>,
    operation_index: Option<usize>,
) -> DagShellError {
    let position = position.unwrap_or(Position::NONE);
    let line = position.line().map(|value| value as u32);
    let column = position.position().map(|value| value as u32);
    error(code, message, line, column).with_operation(operation_index)
}

impl DagShellError {
    fn with_operation(mut self, operation_index: Option<usize>) -> Self {
        self.operation_index = operation_index;
        self
    }
}

fn rhai_error(error: Box<rhai::EvalAltResult>, operation_index: Option<usize>) -> DagShellError {
    let position = error.position();
    let message = error.to_string();
    if let rhai::EvalAltResult::ErrorTerminated(token, _) = &*error
        && token.clone().try_cast::<String>().as_deref() == Some(TIMEOUT_TOKEN)
    {
        return error_at(
            "timeout",
            "script timed out",
            Some(position),
            operation_index,
        );
    }
    if message.contains(LIMIT_TOKEN)
        || matches!(
            &*error,
            rhai::EvalAltResult::ErrorTooManyOperations(_)
                | rhai::EvalAltResult::ErrorStackOverflow(_)
                | rhai::EvalAltResult::ErrorDataTooLarge(..)
        )
    {
        return error_at(
            "limit_exceeded",
            message.replace(LIMIT_TOKEN, "DAG Shell API limit exceeded"),
            Some(position),
            operation_index,
        );
    }
    if matches!(&*error, rhai::EvalAltResult::ErrorParsing(..)) {
        return error_at("parse_error", message, Some(position), operation_index);
    }
    error_at("runtime_error", message, Some(position), operation_index)
}

pub fn operation_traces(operations: &[GraphEditOp]) -> Vec<OperationTrace> {
    operations
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            let mut trace = OperationTrace {
                index,
                kind: operation.kind(),
                id: None,
                from: None,
                to: None,
            };
            match operation {
                GraphEditOp::AddNode { id, .. }
                | GraphEditOp::UpdateNode { id, .. }
                | GraphEditOp::RemoveNode { id } => {
                    trace.id = Some(id.clone());
                }
                GraphEditOp::AddEdge { from, to, .. }
                | GraphEditOp::RemoveEdge { from, to, .. } => {
                    trace.from = Some(from.clone());
                    trace.to = Some(to.clone());
                }
                GraphEditOp::AddLogicalGraph { .. } => {}
            }
            trace
        })
        .collect()
}

pub fn graph_summary(dag: &DAG) -> serde_json::Value {
    serde_json::json!({
        "node_count": dag.tui_snapshot().nodes.len(),
        "edge_count": dag.tui_snapshot().edges.len(),
        "logical_graph_count": dag.logical_graphs().len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_normalized() {
        assert_eq!(
            normalize_timeout(None).unwrap(),
            Duration::from_millis(DEFAULT_TIMEOUT_MS)
        );
        assert_eq!(
            normalize_timeout(Some(0)).unwrap_err().code,
            "invalid_operation"
        );
        assert_eq!(
            normalize_timeout(Some(MAX_TIMEOUT_MS + 1))
                .unwrap_err()
                .code,
            "invalid_operation"
        );
    }

    #[test]
    fn rejects_oversized_script_before_compilation() {
        let engine = crate::data_engine::DataEngine::builder().build();
        let registry = Arc::clone(engine.node_registry());
        let snapshot = DagShellSnapshot::from_dag(engine.dag());
        let script = "x".repeat(MAX_SCRIPT_BYTES + 1);
        let error =
            execute_script(&script, Duration::from_millis(10), &snapshot, &registry).unwrap_err();
        assert_eq!(error.code, "script_too_large");
    }

    fn sandbox() -> (Arc<NodeRegistry>, DagShellSnapshot) {
        let engine = crate::data_engine::DataEngine::builder().build();
        let registry = Arc::clone(engine.node_registry());
        let snapshot = DagShellSnapshot::from_dag(engine.dag());
        (registry, snapshot)
    }

    #[test]
    fn times_out_unbounded_scripts() {
        let (registry, snapshot) = sandbox();
        let started = Instant::now();
        let error = execute_script("loop {}", Duration::ZERO, &snapshot, &registry).unwrap_err();
        assert_eq!(error.code, "timeout");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn limits_deep_recursion() {
        let (registry, snapshot) = sandbox();
        let error = execute_script(
            r#"
            fn recurse(n) {
                recurse(n + 1);
            }
            recurse(0)
            "#,
            Duration::from_millis(100),
            &snapshot,
            &registry,
        )
        .unwrap_err();
        assert_eq!(error.code, "limit_exceeded");
    }

    #[test]
    fn parse_errors_carry_source_position() {
        let (registry, snapshot) = sandbox();
        let error = execute_script(
            "node(\"bad\", \"echo\", #{);\ncommit();",
            Duration::from_millis(100),
            &snapshot,
            &registry,
        )
        .unwrap_err();
        assert_eq!(error.code, "parse_error");
        assert_eq!(error.line, Some(1));
        assert!(error.column.unwrap_or_default() > 0);
    }

    #[test]
    fn unregistered_host_functions_are_unavailable() {
        let (registry, snapshot) = sandbox();
        let error = execute_script(
            r#"read_file("/etc/passwd")"#,
            Duration::from_millis(100),
            &snapshot,
            &registry,
        )
        .unwrap_err();
        assert_eq!(error.code, "runtime_error");
        assert!(error.message.contains("Function not found"));
    }

    #[test]
    fn module_imports_are_disabled() {
        let (registry, snapshot) = sandbox();
        let error = execute_script(
            r#"import "anything" as secret;"#,
            Duration::from_millis(100),
            &snapshot,
            &registry,
        )
        .unwrap_err();
        assert_eq!(error.code, "parse_error");
        assert!(error.message.contains("import"));
    }

    #[test]
    fn expression_depth_is_bounded() {
        let (registry, snapshot) = sandbox();
        let script = format!("{}1{}", "(".repeat(128), ")".repeat(128));
        let error =
            execute_script(&script, Duration::from_millis(100), &snapshot, &registry).unwrap_err();
        assert!(matches!(
            error.code.as_str(),
            "parse_error" | "limit_exceeded"
        ));
    }

    #[test]
    fn array_size_is_bounded() {
        let (registry, snapshot) = sandbox();
        let error = execute_script(
            r#"
            let values = [];
            for value in 0..10001 {
                values.push(value);
            }
            values
            "#,
            Duration::from_millis(1000),
            &snapshot,
            &registry,
        )
        .unwrap_err();
        assert_eq!(error.code, "limit_exceeded");
    }

    #[test]
    fn map_size_is_bounded() {
        let (registry, snapshot) = sandbox();
        let error = execute_script(
            r#"
            let values = #{};
            for value in 0..1001 {
                values[value.to_string()] = value;
            }
            values
            "#,
            Duration::from_millis(1000),
            &snapshot,
            &registry,
        )
        .unwrap_err();
        assert_eq!(error.code, "limit_exceeded");
    }

    #[test]
    fn oversized_maps_cannot_be_staged() {
        let (registry, snapshot) = sandbox();
        let error = execute_script(
            r#"
            let values = #{};
            for value in 0..1001 {
                values[value.to_string()] = value;
            }
            node("oversized", "echo", values);
            commit();
            "#,
            Duration::from_millis(1000),
            &snapshot,
            &registry,
        )
        .unwrap_err();
        assert_eq!(error.code, "limit_exceeded");
    }

    #[test]
    fn api_operation_count_is_bounded() {
        let (registry, snapshot) = sandbox();
        let error = execute_script(
            r#"
            for value in 0..1002 {
                node("node_" + value.to_string(), "echo", #{});
            }
            "#,
            Duration::from_millis(1000),
            &snapshot,
            &registry,
        )
        .unwrap_err();
        if error.code != "limit_exceeded" {
            panic!("api operation error: {error:?}");
        }
    }

    #[test]
    fn queries_use_the_execution_start_snapshot() {
        let mut engine = crate::data_engine::DataEngine::builder().build();
        engine
            .add_node_from_registry("source", "echo", serde_json::json!({}))
            .unwrap();
        let registry = Arc::clone(engine.node_registry());
        let snapshot = DagShellSnapshot::from_dag(engine.dag());
        let execution = execute_script(
            r#"
            let observed = [has_node("source"), has_node("staged"), graph_summary().node_count];
            node("staged", "echo", #{});
            commit();
            observed
            "#,
            Duration::from_millis(100),
            &snapshot,
            &registry,
        )
        .unwrap();
        let observed = dynamic_to_json(&execution.result, 0).unwrap();
        assert_eq!(observed, serde_json::json!([true, false, 1]));
    }
}
