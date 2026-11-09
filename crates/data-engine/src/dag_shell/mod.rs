//! Sandboxed Python-syntax orchestration for transactional DAG edits.
//!
//! Scripts are parsed with [`rustpython_parser`] and evaluated by the
//! tree-walking interpreter in [`interp`]. The interpreter only understands the
//! constructs it implements plus the explicitly registered graph functions in
//! [`api`]; file, network, environment, and process access do not exist in the
//! evaluation surface. Scripts see an immutable snapshot of the DAG and stage
//! graph operations; nothing touches the live DAG until `commit()` succeeds and
//! the staged batch validates against a candidate clone.

mod api;
mod interp;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::dag::DAG;
use crate::node_registry::registry::NodeRegistry;

pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;
pub const MAX_API_OPERATIONS: usize = 1_000;
/// Interpreter operation budget (statements + expression nodes).
pub const MAX_SCRIPT_OPERATIONS: u64 = 100_000;
pub const MAX_CALL_LEVELS: usize = 64;
pub const MAX_ARRAY_SIZE: usize = 10_000;
pub const MAX_MAP_SIZE: usize = 10_000;
pub const MAX_STRING_SIZE: usize = 64 * 1024;
/// Maximum bracket nesting before parsing; also bounds runtime value depth.
pub const MAX_EXPRESSION_DEPTH: usize = 64;
/// Maximum leading whitespace of a top-level statement line, in columns.
/// Bounds statement nesting for the parser and the recursive validator walk.
pub const MAX_INDENTATION: usize = 256;
pub const DEFAULT_TIMEOUT_MS: u64 = 1_000;
pub const MAX_TIMEOUT_MS: u64 = 5_000;
pub const MAX_RESULT_BYTES: usize = 16 * 1024;

/// Stack for the evaluation thread. Bounds recursion of the recursive-descent
/// parser, the validator walk, and the tree-walking evaluator for deeply
/// left-nested expressions (e.g. `1+1+1+...`), which are not bracket-nested.
const EVAL_THREAD_STACK: usize = 64 * 1024 * 1024;

pub(crate) fn run_channel_function(
    function: &serde_json::Value,
    input: Option<&dag_core::value::NodeValue>,
    snapshot: &DagShellSnapshot,
    registry: &Arc<NodeRegistry>,
) -> Result<dag_core::value::NodeValue, String> {
    run_channel_function_with_inputs(function, input, None, snapshot, registry)
}

pub(crate) fn run_channel_function_with_inputs(
    function: &serde_json::Value,
    input: Option<&dag_core::value::NodeValue>,
    second_input: Option<&dag_core::value::NodeValue>,
    snapshot: &DagShellSnapshot,
    registry: &Arc<NodeRegistry>,
) -> Result<dag_core::value::NodeValue, String> {
    let value = function
        .get("__dag_shell_function__")
        .and_then(|v| v.as_object())
        .ok_or("invalid channel function")?;
    let source = value
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or("missing channel function source")?
        .to_owned();
    let captures = value
        .get("captures")
        .and_then(|v| v.as_object())
        .ok_or("missing channel function captures")?
        .clone();
    let functions = value.get("functions").cloned();
    let input = input.cloned();
    let second_input = second_input.cloned();
    let snapshot = snapshot.clone();
    let registry = Arc::clone(registry);
    std::thread::Builder::new()
        .name("dag-shell-channel".into())
        .stack_size(EVAL_THREAD_STACK)
        .spawn(move || {
            interp::run_pure_node_function(
                &source,
                &captures,
                functions.as_ref(),
                input.as_ref(),
                second_input.as_ref(),
                &snapshot,
                &registry,
            )
        })
        .map_err(|e| format!("cannot start channel function: {e}"))?
        .join()
        .map_err(|_| "channel function thread panicked".to_string())?
        .map_err(|e| format!("channel function: {}", e.message))
}

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
        cascade: bool,
    },
    AddEdge {
        from: String,
        from_port: u8,
        to: String,
        to_port: u8,
    },
    AddCollectEdge {
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
}

impl GraphEditOp {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AddNode { .. } => "add_node",
            Self::UpdateNode { .. } => "update_node",
            Self::RemoveNode { .. } => "remove_node",
            Self::AddEdge { .. } => "add_edge",
            Self::AddCollectEdge { .. } => "collect_edge",
            Self::RemoveEdge { .. } => "remove_edge",
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cascade: Option<bool>,
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
    /// Whether the transaction successfully committed graph changes.
    ///
    /// A dry run or a rolled-back transaction is not committed even if the
    /// script called `commit()` before validation/installation failed.
    pub committed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub operations: Vec<OperationTrace>,
    pub graph: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DagShellError>,
}

#[derive(Debug, Clone)]
pub(crate) struct ShellNode {
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

    pub(crate) fn from_flow(flow: &crate::workflow::FlowGraph) -> Self {
        let mut nodes = BTreeMap::new();
        for p in flow.processes() {
            nodes.insert(
                p.id.clone(),
                ShellNode {
                    kind: p.kind.clone(),
                    spec: Some(p.spec.clone()),
                    ports: serde_json::Value::Null,
                },
            );
        }
        for source in flow.sources() {
            nodes.insert(
                source.id.clone(),
                ShellNode {
                    kind: "channel".into(),
                    spec: serde_json::to_value(&source.spec).ok(),
                    ports: serde_json::Value::Null,
                },
            );
        }
        Self {
            nodes,
            edges: flow
                .wires()
                .iter()
                .filter_map(|w| serde_json::to_value(w).ok())
                .collect(),
            dot: String::new(),
            logical_graph_count: 0,
        }
    }

    pub(crate) fn node(&self, id: &str) -> Option<&ShellNode> {
        self.nodes.get(id)
    }

    pub(crate) fn node_kinds(&self) -> Vec<String> {
        let kinds: BTreeSet<String> = self.nodes.values().map(|node| node.kind.clone()).collect();
        kinds.into_iter().collect()
    }

    pub(crate) fn dot(&self) -> &str {
        &self.dot
    }

    pub(crate) fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "node_count": self.nodes.len(),
            "edge_count": self.edges.len(),
            "logical_graph_count": self.logical_graph_count,
        })
    }
}

#[derive(Default)]
pub(crate) struct ScriptState {
    pub(crate) operations: Vec<GraphEditOp>,
    pub(crate) api_operations: usize,
    pub(crate) committed: bool,
}

#[derive(Debug)]
pub(crate) struct ScriptExecution {
    pub operations: Vec<GraphEditOp>,
    pub committed: bool,
    /// Script result serialized as JSON text (16 KB cap enforced).
    pub result: String,
}

pub fn normalize_timeout(timeout_ms: Option<u64>) -> Result<Duration, DagShellError> {
    let millis = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    if millis == 0 || millis > MAX_TIMEOUT_MS {
        return Err(error(
            "invalid_operation",
            format!("timeout_ms must be between 1 and {MAX_TIMEOUT_MS}"),
            None,
            None,
        ));
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

    interp::prescan_source(script)?;

    // Parse, validate, and evaluate on a dedicated thread with a large stack:
    // left-nested expression chains (`1+1+1+...`) are not bracket-nested, so
    // their recursion depth is bounded only by the script size limit.
    let script = script.to_string();
    let snapshot = Arc::new(snapshot.clone());
    let registry = Arc::clone(registry);
    let started = Instant::now();
    let spawned = std::thread::Builder::new()
        .name("dag-shell-eval".into())
        .stack_size(EVAL_THREAD_STACK)
        .spawn(move || interp::run(&script, started, timeout, &snapshot, &registry));
    match spawned {
        Ok(handle) => match handle.join() {
            Ok(result) => result,
            Err(panic) => Err(error(
                "runtime_error",
                format!("script panicked: {panic:?}"),
                None,
                None,
            )),
        },
        Err(spawn_error) => Err(error(
            "runtime_error",
            format!("cannot spawn script evaluation thread: {spawn_error}"),
            None,
            None,
        )),
    }
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
                cascade: None,
            };
            match operation {
                GraphEditOp::AddNode { id, .. }
                | GraphEditOp::UpdateNode { id, .. }
                | GraphEditOp::RemoveNode { id, .. } => {
                    trace.id = Some(id.clone());
                    if matches!(operation, GraphEditOp::RemoveNode { cascade: true, .. }) {
                        trace.cascade = Some(true);
                    }
                }
                GraphEditOp::AddEdge { from, to, .. }
                | GraphEditOp::AddCollectEdge { from, to, .. }
                | GraphEditOp::RemoveEdge { from, to, .. } => {
                    trace.from = Some(from.clone());
                    trace.to = Some(to.clone());
                }
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
mod tests;
