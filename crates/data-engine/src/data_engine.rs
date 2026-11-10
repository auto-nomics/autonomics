use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use container_runtime::ContainerExecutionInfra;
use dag_core::resource::MemoryGuardConfig;
use dag_core::{BundleRegistry, FileRef};
use datafusion::{
    execution::{object_store::ObjectStoreUrl, runtime_env::RuntimeEnv},
    prelude::SessionContext,
};
use serde::Serialize;
use vfs::{MountedObjectStore, OpendalFileStorage};

use crate::dag::{
    DAG, DagError, DagHistory, GatherNode, RunRecord, RunReport, RuntimeStatus, SchedulerConfig,
};
use crate::dag_shell::{
    DagShellError, DagShellOutcome, DagShellSnapshot, GraphEditOp, execute_script, graph_summary,
    normalize_timeout, operation_traces,
};
use crate::error::{Error, Result};
use crate::node_registry::registry::NodeRegistry;
use crate::nodes::DagNode;

/// Observable outcome of a DAG clear operation.
#[derive(Debug, Clone, Serialize)]
pub struct ClearDagOutcome {
    /// Snapshot that can be checked out to recover the cleared DAG.
    pub snapshot_id: Option<String>,
    pub history_ref: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub warnings: Vec<String>,
}

/// `DataEngine` is the core object that implements the data analysis engine.
/// It orchestrates ingestion, transformation, and querying of datasets via a
/// [`DAG`] of nodes executed by an async scheduler.
pub struct DataEngine {
    ctx: SessionContext,
    /// The immutable engine ingredients, handed to the DAG scheduler on every
    /// `run` so each node `execute` can build a fresh isolated `SessionContext`
    /// via `NodeCtx::session()`. Owned here (not in the graph or the nodes) so
    /// mutable `SessionContext` state can never accumulate across runs.
    pub(crate) engine_ctx: crate::node_registry::registry::NodeCtx,
    pub(crate) dag: DAG,
    pub(crate) task_executor: Arc<dyn dag_core::TaskExecutor>,
    /// Shared, immutable after construction. Wrapped in `Arc` so that
    /// [`DataEngine::new_isolated_engine`] can share it across per-agent engines
    /// without rebuilding the ~70 factories.
    pub(crate) node_registry: Arc<NodeRegistry>,
    /// Process-wide container execution resources, injected by the runtime
    /// host and shared with the IO node registry.
    container_execution: Arc<ContainerExecutionInfra>,
    config: SchedulerConfig,
    /// Cross-run instance results shared by runners of this session only.
    pub(crate) flow_cache: std::sync::Arc<tokio::sync::Mutex<crate::workflow::InstanceCache>>,
    /// The session's authored flow (`run_flow`). Interior mutability so flow
    /// execution — which never needs the engine lock — can read the current
    /// graph while other commands edit it. Isolated engines start with their
    /// own empty flow and deliberately do NOT inherit this state.
    pub(crate) flow: std::sync::Arc<std::sync::RwLock<crate::workflow::FlowGraph>>,
    /// The most recent flow run's grouped report, for view surfaces (TUI /
    /// GUI / gateway). `None` until the session runs a flow.
    pub(crate) flow_report:
        std::sync::Arc<std::sync::RwLock<Option<crate::flow::FlowRunReportSummary>>>,
    /// Optional DAG history store. When `Some`, every `run()` automatically
    /// commits a snapshot of the current DAG manifest + run report.
    /// `DagHistory` wraps a `turso::Connection` (Arc-backed, `Clone`), so
    /// cloned engines share the same DB file.
    pub(crate) history: Option<DagHistory>,
    /// The history ref that [`Self::run`] commits to (default `"main"`).
    /// Switch via [`Self::set_history_ref`] to isolate unrelated analysis
    /// tasks into independent snapshot lineages.
    history_ref: String,
    /// Optional commit message for the next snapshot. Set via
    /// [`Self::set_commit_message`] before `run()`. Consumed (cleared) on
    /// each `run()` — falls back to a default when `None`.
    pending_commit_message: Option<String>,
    /// Who initiated the next run (e.g. `"agent:/root/researcher"`), recorded
    /// in the run's audit trail. Set via [`Self::set_run_trigger`] before
    /// `run()`; consumed (cleared) on each `run()`.
    run_trigger: Option<String>,
}

pub(crate) fn channel_info() -> crate::node_registry::NodeInfo {
    crate::node_registry::NodeInfo {
        address: "channel".into(),
        plugin: "core".into(),
        kind: "channel".into(),
        desc: "Built-in dataflow channel operators; see get_node_spec for options".into(),
        deprecated: false,
        data_bundles: vec![],
        resident_plugin: None,
    }
}
pub(crate) fn channel_schema() -> Result<schemars::Schema> {
    serde_json::from_value(crate::workflow::dataflow::channel_operator_schema())
        .map_err(|e| Error::Custom(e.to_string()))
}
pub(crate) fn channel_ports(
    spec: Option<&serde_json::Value>,
) -> Result<crate::nodes::meta::NodePorts> {
    use crate::value::PortType;
    let op = spec
        .and_then(|value| value.get("operator"))
        .and_then(|value| value.as_str());
    if let Some(value) = spec {
        let valid = op.is_some_and(|op| {
            crate::workflow::dataflow::channel_operator_schema()["oneOf"]
                .as_array()
                .is_some_and(|variants| {
                    variants
                        .iter()
                        .any(|variant| variant["properties"]["operator"]["const"] == op)
                })
        });
        if !valid {
            return Err(Error::Custom(format!(
                "unknown channel operator in {value}"
            )));
        }
    }
    let inputs = match op {
        Some("of_items") => 0,
        Some("mix" | "concat" | "combine" | "join") => 2,
        _ => 1,
    };
    let mut ports = crate::nodes::meta::NodePorts::new();
    for _ in 0..inputs {
        ports = ports.add_input_port_of_type(None, PortType::Any);
    }
    if op.is_none() || matches!(op, Some("mix" | "concat")) {
        ports = ports.set_fixed_input(false);
    }
    if op == Some("branch") {
        let branches = spec
            .and_then(|v| v.get("branches"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Custom("branch requires branches".into()))?;
        for branch in branches {
            let name = branch
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Custom("branch requires name".into()))?;
            ports = ports.add_output_port_of_type_with_label(None, PortType::Any, name);
        }
    } else {
        ports = ports.add_output_port_of_type(None, PortType::Any);
    }
    Ok(ports)
}
pub(crate) const CHANNEL_DOC: &str = "Built-in dataflow channel operator. Configure with an operator from get_node_spec. Unary operators use input 0; join/combine use inputs 0 and 1; mix/concat accept multiple inputs; branch emits on its branch ports. Channel sources are constructed with from_list, from_path, from_file_pairs or channel_value. Supported operators accept persisted dag_shell functions as documented in their schemas.";

impl DataEngine {
    const DISABLED_NODE_KIND: &str = "container_command";
    const DEFAULT_MEMORY_GUARD_RATIO: f64 = 0.90;
    const DEFAULT_MEMORY_GUARD_INTERVAL_MS: u64 = 250;

    pub(crate) fn ensure_node_kind_allowed(kind: &str) -> Result<()> {
        let local_kind = kind.rsplit_once('/').map_or(kind, |(_, local)| local);
        if local_kind == Self::DISABLED_NODE_KIND {
            return Err(Error::Custom(format!(
                "node kind '{kind}' is disabled; use a registered dedicated node instead"
            )));
        }
        Ok(())
    }

    fn dynamic_node_builder(registry: Arc<NodeRegistry>) -> dag_core::dag::DynamicNodeBuilder {
        Arc::new(move |kind, spec| {
            Self::ensure_node_kind_allowed(kind).map_err(|error| {
                DagError::Schedule(format!("cannot install logical node `{kind}`: {error}"))
            })?;
            registry
                .build_node(kind, spec)
                .map_err(|error| DagError::Schedule(error.to_string()))
        })
    }

    /// Replace the execution backend used for physical tasks.
    pub fn set_task_executor(&mut self, executor: std::sync::Arc<dyn dag_core::TaskExecutor>) {
        self.dag.set_task_executor(Arc::clone(&executor));
        self.task_executor = executor;
    }

    /// Configure an explicit ordinary coordinator concurrency limit.
    /// This replaces both per-run dispatch and the shared host CPU-derived
    /// dispatch limiter. It does not change task CPU/memory requests, executor
    /// admission, or Slurm admission.
    pub fn set_coordinator_concurrency(&mut self, limit: usize) -> Result<()> {
        if !(1..=dag_core::dag::execution::MAX_REMOTE_TASKS).contains(&limit) {
            return Err(DagError::Schedule("invalid coordinator concurrency".into()).into());
        }
        self.config.max_concurrency = limit;
        self.engine_ctx.global_sem = Some(Arc::new(tokio::sync::Semaphore::new(limit)));
        self.refresh_coordinator_factory_context();
        Ok(())
    }

    /// Remove both coordinator dispatch gates for ordinary remote execution.
    /// Ready tasks, including dynamic fanout, have no local CPU-derived or
    /// fixed concurrency cap. Resource requests and executor/Slurm admission
    /// remain unchanged. Configure the template before creating sessions.
    pub fn set_unlimited_coordinator_concurrency(&mut self) {
        self.config.max_concurrency = 0;
        self.engine_ctx.global_sem = None;
        self.refresh_coordinator_factory_context();
    }

    fn refresh_coordinator_factory_context(&mut self) {
        let mut factory_ctx = self.node_registry.ctx().clone();
        factory_ctx.global_sem = self.engine_ctx.global_sem.clone();
        self.node_registry = Arc::new(self.node_registry.with_node_ctx(factory_ctx));
        self.dag
            .set_dynamic_node_builder(Self::dynamic_node_builder(Arc::clone(&self.node_registry)));
    }

    /// Configure the built-in local executor.
    pub fn set_local_task_executor(
        &mut self,
        workspace_root: impl Into<std::path::PathBuf>,
        cpu_limit: Option<u32>,
        memory_limit_bytes: Option<u64>,
        stage_inputs: bool,
    ) -> Result<()> {
        let executor = if stage_inputs {
            dag_core::LocalTaskExecutor::with_workspace_root_resource_limits_and_input_staging(
                workspace_root,
                cpu_limit,
                memory_limit_bytes,
            )?
        } else {
            dag_core::LocalTaskExecutor::with_workspace_root_and_resource_limits(
                workspace_root,
                cpu_limit,
                memory_limit_bytes,
            )?
        };
        self.set_task_executor(Arc::new(executor));
        Ok(())
    }

    /// Set resources for one concrete physical node.
    pub fn set_task_resources(
        &mut self,
        id: impl Into<String>,
        resources: dag_core::TaskResources,
    ) -> Result<()> {
        self.dag.set_task_resources(id, resources)?;
        Ok(())
    }

    /// Set resources inherited by every physical job of a logical process.
    pub fn set_logical_task_resources(
        &mut self,
        logical_node: impl Into<String>,
        resources: dag_core::TaskResources,
    ) -> Result<()> {
        self.dag
            .set_logical_task_resources(logical_node, resources)?;
        Ok(())
    }

    /// Set resources inherited by tasks without a more specific override.
    pub fn set_default_task_resources(&mut self, resources: dag_core::TaskResources) -> Result<()> {
        self.dag.set_default_task_resources(resources)?;
        Ok(())
    }

    fn new_from_parts(
        ctx: SessionContext,
        runtime_env: Arc<RuntimeEnv>,
        opendal: Option<Arc<OpendalFileStorage>>,
        bundle_registry: Arc<BundleRegistry>,
        container_execution: Arc<ContainerExecutionInfra>,
        plugins_root: Option<std::path::PathBuf>,
    ) -> Self {
        let bundle_registry = crate::data_bundles::registry_with_builtins(&bundle_registry);
        // Global concurrency limiter shared across all agents.
        // Sized to leave ≥ 2 worker threads for AgentEngineActors +
        // tool execution, preventing CPU-bound node work from starving
        // inter-agent communication on the shared tokio runtime.
        let global_permits = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(2).max(1))
            .unwrap_or(4);
        let global_sem = Some(Arc::new(tokio::sync::Semaphore::new(global_permits)));

        let engine_ctx = crate::node_registry::registry::NodeCtx {
            runtime_env: runtime_env.clone(),
            opendal: opendal.clone(),
            bundle_registry: bundle_registry.clone(),
            bound_data_bundles: Default::default(),
            global_sem,
            runtime_output_projection: None,
        };
        let node_registry =
            crate::default_registry::build_default_registry_with_container_execution(
                runtime_env.clone(),
                opendal.clone(),
                bundle_registry,
                Arc::clone(&container_execution),
                plugins_root,
            );
        let node_registry = Arc::new(node_registry);
        let mut dag = DAG::default();
        dag.set_dynamic_node_builder(Self::dynamic_node_builder(Arc::clone(&node_registry)));
        Self {
            ctx,
            engine_ctx,
            task_executor: dag.task_executor(),
            dag,
            node_registry,
            container_execution,
            config: SchedulerConfig {
                memory_guard: Self::memory_guard_from_env(),
                ..SchedulerConfig::default()
            },
            flow_cache: std::default::Default::default(),
            flow: std::sync::Arc::new(std::sync::RwLock::new(crate::workflow::FlowGraph::empty())),
            flow_report: std::sync::Arc::new(std::sync::RwLock::new(None)),
            history: None,
            history_ref: "main".to_string(),
            pending_commit_message: None,
            run_trigger: None,
        }
    }

    fn memory_guard_from_env() -> Option<dag_core::resource::MemoryGuardConfig> {
        let ratio = std::env::var_os("AUTONOMICS_DAG_MEMORY_LIMIT_RATIO")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| Self::DEFAULT_MEMORY_GUARD_RATIO.to_string());
        if matches!(
            ratio.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "disabled"
        ) {
            return None;
        }
        let Ok(ratio) = ratio.trim().parse::<f64>() else {
            tracing::warn!(
                value = %ratio,
                "invalid AUTONOMICS_DAG_MEMORY_LIMIT_RATIO; using 0.90"
            );
            return MemoryGuardConfig::new(
                Self::DEFAULT_MEMORY_GUARD_RATIO,
                Self::default_memory_interval(),
            );
        };

        let interval_ms = std::env::var_os("AUTONOMICS_DAG_MEMORY_SAMPLE_INTERVAL_MS")
            .and_then(|value| value.to_string_lossy().parse::<u64>().ok())
            .unwrap_or(Self::DEFAULT_MEMORY_GUARD_INTERVAL_MS)
            .max(10);
        let interval = std::time::Duration::from_millis(interval_ms);
        let guard = MemoryGuardConfig::new(ratio, interval);
        if guard.is_none() {
            tracing::warn!(
                value = ratio,
                "AUTONOMICS_DAG_MEMORY_LIMIT_RATIO must be in (0, 1]; memory guard disabled"
            );
        }
        guard
    }

    fn default_memory_interval() -> std::time::Duration {
        std::time::Duration::from_millis(Self::DEFAULT_MEMORY_GUARD_INTERVAL_MS)
    }

    pub fn builder() -> DataEngineBuilder {
        DataEngineBuilder::default()
    }

    /// Returns the shared session context (object stores, catalogs, …).
    pub fn ctx(&self) -> SessionContext {
        self.ctx.clone()
    }

    /// Register a node under `id`(for test purposes).
    ///
    /// Prefer [`Self::add_node_from_registry`] for all standard node kinds —
    /// it validates the spec against the registered JSON Schema and builds
    /// the node automatically. Use this raw `add_node` only for custom test
    /// nodes or ad-hoc types not in the registry.
    ///
    /// ```ignore
    /// let meta = NodePorts::new();
    /// engine.add_node("x", MyNode::new(meta, ...))?;
    /// ```
    pub fn add_node<N: DagNode + 'static>(
        &mut self,
        id: impl Into<String>,
        node: N,
    ) -> Result<&mut Self> {
        self.dag.add_node(id.into(), Box::new(node))?;
        Ok(self)
    }

    /// Create a node by its registered `kind` and a JSON `spec`.
    ///
    /// The spec is validated against the kind's JSON Schema and deserialized
    /// by the corresponding factory. This is the primary path for node
    /// creation — all standard node kinds (file_to_dataframe, sql,
    /// dataframe_to_file, ldsc, linear_regression, mock, mr) are available.
    pub fn add_node_from_registry(
        &mut self,
        node_id: impl Into<String>,
        kind: &str,
        spec: serde_json::Value,
    ) -> Result<()> {
        Self::ensure_node_kind_allowed(kind)?;
        let address = self.node_registry.resolve_node_address(kind)?;
        let node = self.node_registry.build_node(&address, spec.clone())?;
        self.dag
            .add_node_with_spec(node_id.into(), node, address, spec)?;
        Ok(())
    }

    fn remove_node_cascade(dag: &mut DAG, id: &str) -> std::result::Result<(), DagError> {
        if dag.get_node(id).is_none() {
            return Err(DagError::UnknownNode(id.to_string()));
        }

        let mut downstream = BTreeSet::from([id.to_string()]);
        let mut pending = vec![id.to_string()];
        while let Some(current) = pending.pop() {
            for successor in dag.successors(&current) {
                if downstream.insert(successor.clone()) {
                    pending.push(successor);
                }
            }
        }

        for node in dag
            .topo_order()?
            .into_iter()
            .rev()
            .filter(|node| downstream.contains(node))
        {
            dag.delete_node(&node)?;
        }
        Ok(())
    }

    fn apply_graph_edit_ops_to_candidate(&self, operations: &[GraphEditOp]) -> Result<DAG> {
        let mut candidate = self.dag.clone();
        for (index, operation) in operations.iter().enumerate() {
            let result = match operation {
                GraphEditOp::AddNode { id, kind, spec } => Self::ensure_node_kind_allowed(kind)
                    .and_then(|_| {
                        let node = self.node_registry.build_node(kind, spec.clone())?;
                        candidate.add_node_with_spec(
                            id.clone(),
                            node,
                            kind.clone(),
                            spec.clone(),
                        )?;
                        Ok(())
                    }),
                GraphEditOp::UpdateNode { id, spec } => {
                    let kind = candidate
                        .get_node(id)
                        .map(|node| node.kind().to_string())
                        .ok_or_else(|| Error::Dag(DagError::UnknownNode(id.clone())))?;
                    Self::ensure_node_kind_allowed(&kind)?;
                    let node = self.node_registry.build_node(&kind, spec.clone())?;
                    candidate.replace_node_with_spec(id, node, kind, spec.clone())?;
                    Ok(())
                }
                GraphEditOp::RemoveNode { id, cascade } => {
                    if !*cascade {
                        candidate.delete_node(id).map_err(Error::from)?;
                        Ok(())
                    } else {
                        Self::remove_node_cascade(&mut candidate, id).map_err(Error::from)
                    }
                }
                GraphEditOp::AddEdge {
                    from,
                    from_port,
                    to,
                    to_port,
                } => candidate
                    .add_edge(from.clone(), to.clone(), *from_port, *to_port)
                    .map(|_| ())
                    .map_err(Error::from),
                GraphEditOp::AddCollectEdge { .. } => Err(Error::Custom(
                    "collect_edge is only supported for flow graphs".into(),
                )),
                GraphEditOp::RemoveEdge {
                    from,
                    from_port,
                    to,
                    to_port,
                } => candidate
                    .delete_edge(from.clone(), to.clone(), *from_port, *to_port)
                    .map_err(Error::from),
            };
            if let Err(error) = result {
                return Err(Error::Custom(format!(
                    "operation {index} ({}) failed: {error}",
                    operation.kind()
                )));
            }
        }
        candidate.validate()?;
        Ok(candidate)
    }

    /// Validate a staged graph edit batch against a cloned candidate DAG.
    pub fn dry_run_graph_edit_ops(
        &self,
        operations: Vec<GraphEditOp>,
    ) -> Result<serde_json::Value> {
        let candidate = self.apply_graph_edit_ops_to_candidate(&operations)?;
        Ok(graph_summary(&candidate))
    }

    /// Atomically apply a staged graph edit batch. A failed operation or final
    /// validation leaves the live DAG untouched.
    pub fn apply_graph_edit_ops(
        &mut self,
        operations: Vec<GraphEditOp>,
    ) -> Result<serde_json::Value> {
        let mut candidate = self.apply_graph_edit_ops_to_candidate(&operations)?;
        candidate.live_view = self.dag.live_view.clone();
        self.dag = candidate;
        Ok(graph_summary(&self.dag))
    }

    pub fn dag(&self) -> &DAG {
        &self.dag
    }

    pub fn run_dag_shell(
        &mut self,
        script: &str,
        dry_run: bool,
        timeout_ms: Option<u64>,
    ) -> DagShellOutcome {
        let timeout = match normalize_timeout(timeout_ms) {
            Ok(timeout) => timeout,
            Err(error) => return shell_failure(dry_run, error),
        };
        let snapshot = DagShellSnapshot::from_dag(&self.dag);
        let execution = match execute_script(script, timeout, &snapshot, &self.node_registry) {
            Ok(execution) => execution,
            Err(error) => return shell_failure(dry_run, error),
        };
        let operations = execution.operations;
        let traces = operation_traces(&operations);
        let result = execution.result;

        if !execution.committed {
            return DagShellOutcome {
                ok: true,
                applied: false,
                dry_run,
                committed: false,
                result: Some(result),
                operations: traces,
                graph: graph_summary(&self.dag),
                error: None,
            };
        }

        let summary = if dry_run {
            self.dry_run_graph_edit_ops(operations.clone())
        } else {
            self.apply_graph_edit_ops(operations.clone())
        };
        match summary {
            Ok(graph) => DagShellOutcome {
                ok: true,
                applied: !dry_run,
                dry_run,
                committed: true,
                result: Some(result),
                operations: traces,
                graph,
                error: None,
            },
            Err(error) => DagShellOutcome {
                ok: false,
                applied: false,
                dry_run,
                // The transaction rolled back. `committed` describes whether
                // graph changes were committed, not whether a failure record
                // was observed.
                committed: false,
                result: None,
                operations: traces,
                graph: graph_summary(&self.dag),
                error: Some(transaction_error(error)),
            },
        }
    }

    /// Evaluate the existing sandbox against a snapshot, then apply the staged
    /// operations atomically to the authored process/channel graph.
    pub fn run_flow_shell(
        &mut self,
        script: &str,
        dry_run: bool,
        timeout_ms: Option<u64>,
    ) -> DagShellOutcome {
        let timeout = match normalize_timeout(timeout_ms) {
            Ok(t) => t,
            Err(e) => return shell_failure(dry_run, e),
        };
        let old = self.flow.read().expect("flow lock").clone();
        let snapshot = DagShellSnapshot::from_flow(&old);
        let execution = match execute_script(script, timeout, &snapshot, &self.node_registry) {
            Ok(e) => e,
            Err(e) => return shell_failure(dry_run, e),
        };
        let traces = operation_traces(&execution.operations);
        let summary = |g: &crate::workflow::FlowGraph| serde_json::json!({"process_count":g.processes().len(), "source_count":g.sources().len(), "wire_count":g.wires().len()});
        if !execution.committed {
            return DagShellOutcome {
                ok: true,
                applied: false,
                dry_run,
                committed: false,
                result: Some(execution.result),
                operations: traces,
                graph: summary(&old),
                error: None,
            };
        }
        let result = self.preview_flow_edits(&execution.operations);
        match result {
            Ok(candidate) => {
                let graph = summary(&candidate);
                if !dry_run {
                    *self.flow.write().expect("flow lock") = candidate;
                }
                DagShellOutcome {
                    ok: true,
                    applied: !dry_run,
                    dry_run,
                    committed: !dry_run,
                    result: Some(execution.result),
                    operations: traces,
                    graph,
                    error: None,
                }
            }
            Err(error) => DagShellOutcome {
                ok: false,
                applied: false,
                dry_run,
                committed: false,
                result: None,
                operations: traces,
                graph: summary(&old),
                error: Some(transaction_error(error)),
            },
        }
    }

    /// Query the JSON Schema of a registered node kind.
    pub(crate) fn preview_flow_edits(
        &self,
        operations: &[GraphEditOp],
    ) -> Result<crate::workflow::FlowGraph> {
        let old = self.flow.read().expect("flow lock").clone();
        let mut json = serde_json::to_value(&old).map_err(|e| Error::Custom(e.to_string()))?;
        for (index, op) in operations.iter().enumerate() {
            let edit = (|| -> Result<()> {
                match op {
                    GraphEditOp::AddNode { id, kind, spec } => {
                        if kind == "__channel_source" {
                            json["sources"]
                                .as_array_mut()
                                .expect("source array")
                                .push(serde_json::json!({"id":id,"spec":spec}));
                        } else {
                            Self::ensure_node_kind_allowed(kind)?;
                            json["processes"]
                                .as_array_mut()
                                .expect("process array")
                                .push(serde_json::json!({"id":id,"kind":kind,"spec":spec}));
                        }
                    }
                    GraphEditOp::UpdateNode { id, spec } => {
                        let process = json["processes"]
                            .as_array_mut()
                            .expect("process array")
                            .iter_mut()
                            .find(|p| p["id"] == *id)
                            .ok_or_else(|| Error::Custom(format!("unknown process `{id}`")))?;
                        process["spec"] = spec.clone();
                    }
                    GraphEditOp::RemoveNode { id, cascade } => {
                        let connected = json["wires"]
                            .as_array()
                            .expect("wires")
                            .iter()
                            .any(|w| w["from"] == *id || w["to"] == *id);
                        if connected && !cascade {
                            return Err(Error::Custom(format!("node `{id}` has connected wires")));
                        }
                        let mut removed = false;
                        for field in ["processes", "sources"] {
                            let list = json[field].as_array_mut().expect("graph array");
                            let before = list.len();
                            list.retain(|v| v["id"] != *id);
                            removed |= before != list.len();
                        }
                        if !removed {
                            return Err(Error::Custom(format!("unknown node `{id}`")));
                        }
                        if *cascade {
                            json["wires"]
                                .as_array_mut()
                                .expect("wires")
                                .retain(|w| w["from"] != *id && w["to"] != *id);
                        }
                    }
                    GraphEditOp::AddEdge {
                        from,
                        from_port,
                        to,
                        to_port,
                    }
                    | GraphEditOp::AddCollectEdge {
                        from,
                        from_port,
                        to,
                        to_port,
                    } => {
                        let mode = if matches!(op, GraphEditOp::AddCollectEdge { .. }) {
                            "collect"
                        } else {
                            "direct"
                        };
                        json["wires"].as_array_mut().expect("wires").push(serde_json::json!({"from":from,"from_port":from_port,"to":to,"to_port":to_port,"op":mode}));
                    }
                    GraphEditOp::RemoveEdge {
                        from,
                        from_port,
                        to,
                        to_port,
                    } => {
                        let wires = json["wires"].as_array_mut().expect("wires");
                        let before = wires.len();
                        wires.retain(|w| {
                            !(w["from"] == *from
                                && w["from_port"] == *from_port
                                && w["to"] == *to
                                && w["to_port"] == *to_port)
                        });
                        if before == wires.len() {
                            return Err(Error::Custom("unknown wire".into()));
                        }
                    }
                }
                Ok(())
            })();
            edit.map_err(|e| {
                Error::Custom(format!("operation {index} ({}) failed: {e}", op.kind()))
            })?;
        }
        let graph: crate::workflow::FlowGraph =
            serde_json::from_value(json).map_err(|e| Error::Custom(e.to_string()))?;
        graph.validate().map_err(|e| Error::Custom(e.to_string()))?;
        crate::workflow::dataflow::Dataflow::<()>::new(&graph).map_err(Error::Custom)?;
        Ok(graph)
    }

    pub fn get_node_spec(&self, kind: &str) -> Result<schemars::Schema> {
        if kind == "channel" {
            return channel_schema();
        }
        Ok(self.node_registry.get_node_spec(kind)?)
    }

    /// Query the input/output port layout of a registered node kind.
    pub fn get_node_ports(&self, kind: &str) -> Result<crate::nodes::meta::NodePorts> {
        if kind == "channel" {
            return channel_ports(None);
        }
        Ok(self.node_registry.get_node_ports(kind)?)
    }

    /// Query the concrete port layout induced by a node spec.
    pub fn get_node_ports_for_spec(
        &self,
        kind: &str,
        spec: serde_json::Value,
    ) -> Result<crate::nodes::meta::NodePorts> {
        if kind == "channel" {
            return channel_ports(Some(&spec));
        }
        Ok(self.node_registry.get_node_ports_for_spec(kind, spec)?)
    }

    /// Query the documentation string of a registered node kind.
    pub fn get_node_doc(&self, kind: &str) -> Result<String> {
        if kind == "channel" {
            return Ok(CHANNEL_DOC.into());
        }
        Ok(self.node_registry.get_node_doc(kind)?)
    }

    /// List metadata of every registered node kind (kind + JSON Schema).
    pub fn list_nodes(&self) -> Vec<crate::node_registry::NodeInfo> {
        self.node_registry.list_nodes()
    }

    pub fn remove_node(&mut self, id: impl Into<String>) -> Result<&mut Self> {
        let id = id.into();
        self.dag.delete_node(&id)?;
        Ok(self)
    }

    /// Update an existing node's spec in-place.
    ///
    /// The node's `kind` is discovered from its current `node_type()` (which
    /// equals the registry kind via the `DagNode` trait contract). A new
    /// instance is built through the same factory with `spec`, then the DAG
    /// payload is atomically replaced — all edges are preserved and
    /// re-validated against the new node's port topology.
    ///
    /// Errors if `id` does not exist, if the kind has no registered factory,
    /// if the spec fails deserialization, or if any existing edge becomes
    /// incompatible with the new port layout.
    pub fn update_node(&mut self, id: impl Into<String>, spec: serde_json::Value) -> Result<()> {
        let id = id.into();
        let kind = self
            .dag
            .get_node(&id)
            .ok_or_else(|| Error::Dag(DagError::UnknownNode(id.clone())))?
            .kind()
            .to_string();
        Self::ensure_node_kind_allowed(&kind)?;
        let node = self.node_registry.build_node(&kind, spec.clone())?;
        self.dag.replace_node_with_spec(&id, node, kind, spec)?;
        Ok(())
    }

    pub fn view_dag(&self) -> Result<String> {
        Ok(self.dag.to_dot())
    }

    /// Return the stable, owned snapshot used by interactive DAG consumers.
    pub fn dag_tui_snapshot(&self) -> Result<crate::dag::DagTuiSnapshot> {
        Ok(self.dag.tui_snapshot())
    }

    /// Get the retained `(kind, spec)` of an existing node instance.
    ///
    /// Returns `None` when `id` does not exist in the DAG, or when the node
    /// was added through the raw `add_node` path (no spec retained). This is
    /// the instance-level counterpart of [`Self::get_node_spec`] (which returns
    /// a kind's parameter *schema*, not the instance's stored configuration).
    pub fn get_node(&self, id: &str) -> Option<(String, serde_json::Value)> {
        self.dag.node_spec(id)
    }

    /// Whether a node with `id` exists in the DAG (regardless of whether it
    /// has a retained spec). Use to distinguish "unknown id" from "exists but
    /// no spec" when interpreting a `None` from [`Self::get_node`].
    pub fn node_exists(&self, id: &str) -> bool {
        self.dag.get_node(id).is_some()
    }

    /// Commit a recoverable pre-clear snapshot when history is attached, then
    /// clear all nodes, edges, and runtime state.
    pub async fn clear_dag(&mut self) -> Result<ClearDagOutcome> {
        let manifest = self.dag.to_manifest();
        let manifest_hash = manifest.content_hash();
        let node_count = manifest.nodes.len();
        let edge_count = manifest.edges.len();
        let mut warnings = Vec::new();
        let snapshot_id = if node_count == 0 && edge_count == 0 {
            None
        } else if let Some(history) = self.history.clone() {
            let head = history
                .ref_head(&self.history_ref)
                .await
                .map_err(Error::Dag)?;
            match head {
                Some(head) if head.manifest_hash == manifest_hash => Some(head.id),
                _ => Some(
                    history
                        .commit(
                            &self.history_ref,
                            &manifest,
                            None::<&RunReport>,
                            "before clear DAG",
                        )
                        .await
                        .map_err(Error::Dag)?,
                ),
            }
        } else {
            warnings
                .push("no DAG history store attached; pre-clear snapshot was not persisted".into());
            None
        };
        self.dag.clear();
        tracing::info!(
            history_ref = %self.history_ref,
            snapshot_id = ?snapshot_id,
            node_count,
            edge_count,
            "DAG cleared"
        );
        Ok(ClearDagOutcome {
            snapshot_id,
            history_ref: self.history_ref.clone(),
            node_count,
            edge_count,
            warnings,
        })
    }

    // ── history / ref management ───────────────────────────────────────────

    /// Create a new analysis ref: clears the in-memory DAG and switches the
    /// engine's history ref to `name`. If a history store is attached, the
    /// ref is created in the database (the first `run()` will create a root
    /// snapshot).
    ///
    /// This is the replacement for the old `clear_dag` — instead of wiping
    /// state without trace, it starts a new independent lineage.
    pub async fn new_dag_ref(&mut self, name: &str) -> Result<()> {
        // Reject if the ref already exists.
        if let Some(history) = &self.history
            && history.ref_head(name).await.map_err(Error::Dag)?.is_some()
        {
            return Err(Error::Custom(format!(
                "ref '{name}' already exists. Use switch_dag_ref to activate it, \
                     or branch_from_snapshot to create a new lineage from a snapshot."
            )));
        }
        self.dag.clear();
        self.history_ref = name.to_string();
        Ok(())
    }

    /// Switch the engine's history ref to an existing ref name **and load
    /// that ref's head snapshot into memory**.
    ///
    /// This ensures the in-memory DAG matches the target ref's latest state,
    /// preventing cross-ref contamination where a stale workspace from ref A
    /// gets committed to ref B.
    ///
    /// Rejects unknown ref names.
    pub async fn switch_dag_ref(&mut self, name: &str) -> Result<()> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;

        let head = history
            .ref_head(name)
            .await
            .map_err(Error::Dag)?
            .ok_or_else(|| {
                Error::Custom(format!(
                    "ref '{name}' does not exist. Use new_dag_ref to create it, \
                     or list_dag_refs to see available refs."
                ))
            })?;

        // Load the ref's head snapshot into the in-memory DAG so the workspace
        // matches the lineage the user just switched to.
        let manifest = head
            .manifest()
            .map_err(|e| Error::Custom(format!("manifest deserialization: {e}")))?;
        self.rebuild_dag_from_manifest(&manifest)?;
        self.history_ref = name.to_string();
        Ok(())
    }

    /// List all refs in the history store. Returns `Ok(vec![])` if no history
    /// is attached.
    pub async fn list_dag_refs(&self) -> Result<Vec<(String, String, bool)>> {
        let mut refs = match &self.history {
            Some(h) => h.list_refs().await.map_err(Error::Dag)?,
            None => vec![],
        };
        // Include the current ref even if it has no snapshots yet (just created
        // via new_dag_ref, never run). Shows with an empty snapshot id.
        let current_exists = refs.iter().any(|(name, _, _)| name == &self.history_ref);
        if !current_exists {
            refs.push((self.history_ref.clone(), String::new(), false));
        }
        Ok(refs)
    }

    /// Show the snapshot lineage for a ref (default: current ref).
    /// Returns snapshots newest-first.
    pub async fn dag_log(
        &self,
        ref_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::dag::Snapshot>> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;
        let r = ref_name.unwrap_or(&self.history_ref);
        history.log(r, limit).await.map_err(Error::Dag)
    }

    /// List recent execution records, newest-first. `ref_name = None` spans
    /// all refs. The audit counterpart of [`Self::dag_log`]: snapshots version
    /// definitions, runs version executions.
    pub async fn list_runs(&self, limit: usize, ref_name: Option<&str>) -> Result<Vec<RunRecord>> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;
        history.list_runs(limit, ref_name).await.map_err(Error::Dag)
    }

    /// Fetch a single execution record by run id.
    pub async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;
        let run_id = run_id.trim();
        if let Some(run) = history.get_run(run_id).await.map_err(Error::Dag)? {
            return Ok(Some(run));
        }
        if run_id.is_empty() {
            return Ok(None);
        }
        let candidates = history.list_runs(1000, None).await.map_err(Error::Dag)?;
        let mut matches = candidates
            .into_iter()
            .filter(|run| run.id.starts_with(run_id));
        let first = matches.next();
        if matches.next().is_some() {
            return Err(Error::Custom(format!(
                "ambiguous run id prefix `{run_id}`; use a longer prefix or the full id"
            )));
        }
        Ok(first)
    }

    /// Export one recorded run as deliverable provenance evidence — a W3C
    /// PROV-JSON document or an RO-Crate 1.1 directory (result files pulled
    /// from the object store, content-verified). See
    /// [`crate::dag::export`]. `run_id` accepts `latest` or a unique id
    /// prefix. `out_dir` may be a `vfs://` uri or any path covered by a
    /// mount — the export is then uploaded through the object store so
    /// agents can see it — or an absolute host path.
    pub async fn export_run(
        &self,
        run_id: &str,
        format: crate::dag::ExportFormat,
        out_dir: std::path::PathBuf,
    ) -> Result<crate::dag::ExportSummary> {
        use crate::dag::export as dag_export;

        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;
        let run = dag_export::resolve_run(history, run_id)
            .await
            .map_err(Error::Custom)?;
        // Engine-level error rows carry no run report; export what exists.
        let report: serde_json::Value = run
            .run_report_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let manifest = match &run.snapshot_id {
            Some(snapshot_id) => history
                .get_snapshot(snapshot_id)
                .await
                .map_err(Error::Dag)?
                .map(|snapshot| snapshot.manifest_json),
            None => None,
        };

        // VFS-visible destinations materialize into a staging directory
        // first, then upload through the object store.
        let target = dag_export::resolve_export_target(
            &out_dir.to_string_lossy(),
            self.engine_ctx.opendal.as_deref(),
        )
        .map_err(Error::Custom)?;
        let staging = match &target {
            dag_export::ExportTarget::Vfs(_) => {
                let dir = std::env::temp_dir()
                    .join(format!("autonomics-export-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&dir)
                    .map_err(|e| Error::Custom(format!("create staging dir: {e}")))?;
                Some(dir)
            }
            dag_export::ExportTarget::Host(_) => None,
        };
        let out_path = staging.as_deref().unwrap_or(out_dir.as_path());

        let mut summary = match format {
            crate::dag::ExportFormat::Prov => {
                dag_export::write_prov_document(&run, &report, manifest.as_deref(), out_path)
                    .map_err(Error::Custom)
            }
            crate::dag::ExportFormat::Crate => dag_export::export_ro_crate(
                &run,
                &report,
                manifest.as_deref(),
                out_path,
                self.engine_ctx.opendal.as_deref(),
            )
            .await
            .map_err(Error::Custom),
        }?;

        if let (dag_export::ExportTarget::Vfs(prefix), Some(staging), Some(storage)) =
            (&target, &staging, self.engine_ctx.opendal.as_deref())
        {
            dag_export::upload_export_to_vfs(&mut summary, staging, prefix, storage)
                .await
                .map_err(Error::Custom)?;
        }
        if let Some(staging) = &staging {
            let _ = std::fs::remove_dir_all(staging);
        }
        Ok(summary)
    }

    /// Fetch a single snapshot by id or short-hash prefix.
    pub async fn get_snapshot(&self, snapshot_id: &str) -> Result<Option<crate::dag::Snapshot>> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;
        history
            .resolve_snapshot(snapshot_id)
            .await
            .map_err(Error::Dag)
    }

    /// Diff two snapshots' manifests. Returns a textual diff.
    pub async fn diff_snapshots(&self, old_id: &str, new_id: &str) -> Result<String> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;

        let old_snap = history
            .resolve_snapshot(old_id)
            .await
            .map_err(Error::Dag)?
            .ok_or_else(|| Error::Custom(format!("snapshot '{old_id}' not found")))?;
        let new_snap = history
            .resolve_snapshot(new_id)
            .await
            .map_err(Error::Dag)?
            .ok_or_else(|| Error::Custom(format!("snapshot '{new_id}' not found")))?;

        let old_m = old_snap
            .manifest()
            .map_err(|e| Error::Custom(e.to_string()))?;
        let new_m = new_snap
            .manifest()
            .map_err(|e| Error::Custom(e.to_string()))?;

        Ok(format_manifest_diff(&old_m, &new_m))
    }

    /// Load a historical snapshot's DAG into memory **without** moving the
    /// history ref.
    ///
    /// Clears the current DAG, rebuilds every node + edge from the snapshot's
    /// manifest. The ref stays where it is — subsequent `run()` calls commit
    /// with the current ref head as parent (just like a normal run).
    ///
    /// This is the equivalent of `git checkout <commit>`: you inspect and
    /// work from a historical state, but the branch pointer doesn't move.
    /// Short-hash prefixes are accepted.
    pub async fn checkout_dag(&mut self, snapshot_id: &str) -> Result<()> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;

        let snap = history
            .resolve_snapshot(snapshot_id)
            .await
            .map_err(Error::Dag)?
            .ok_or_else(|| Error::Custom(format!("snapshot '{snapshot_id}' not found")))?;

        let manifest = snap
            .manifest()
            .map_err(|e| Error::Custom(format!("manifest deserialization: {e}")))?;

        self.rebuild_dag_from_manifest(&manifest)?;

        Ok(())
    }

    /// Restore the head snapshot of the current history ref into the
    /// in-memory DAG.
    ///
    /// A no-op (returns `Ok(false)`) when no history store is attached, the
    /// ref has no snapshots yet, or the in-memory DAG is non-empty — never
    /// wipes live state. Engine actors call this right after an agent is
    /// created so a restarted agent resumes its previous workspace instead of
    /// facing an empty DAG (the run snapshots were always committed; only the
    /// in-memory graph was lost).
    pub async fn restore_ref_head(&mut self) -> Result<bool> {
        if !self.dag.node_ids().is_empty() {
            return Ok(false);
        }
        let Some(history) = self.history.as_ref() else {
            return Ok(false);
        };
        let Some(head) = history
            .ref_head(&self.history_ref)
            .await
            .map_err(Error::Dag)?
        else {
            return Ok(false);
        };
        let manifest = head
            .manifest()
            .map_err(|e| Error::Custom(format!("manifest deserialization: {e}")))?;
        self.rebuild_dag_from_manifest(&manifest)?;
        if let Some(report_json) = &head.run_report_json {
            self.restore_checkpoint_from_report(report_json)?;
        }
        Ok(true)
    }

    /// Rebuild reusable file outputs from the report embedded in the head
    /// snapshot. Only successful nodes with both a fingerprint and port
    /// assignments are restored; everything else stays dirty.
    fn restore_checkpoint_from_report(&mut self, report_json: &str) -> Result<()> {
        let report: serde_json::Value = serde_json::from_str(report_json).map_err(|error| {
            Error::Custom(format!("checkpoint run report deserialization: {error}"))
        })?;
        let Some(nodes) = report.get("nodes").and_then(|nodes| nodes.as_array()) else {
            return Ok(());
        };

        for node in nodes {
            if node.get("status").and_then(|status| status.as_str()) != Some("success") {
                continue;
            }
            let (Some(id), Some(fingerprint)) = (
                node.get("id").and_then(|id| id.as_str()),
                node.get("fingerprint").and_then(|fp| fp.as_str()),
            ) else {
                continue;
            };
            let Some(ports) = node
                .get("port_assignments")
                .and_then(|ports| ports.as_object())
            else {
                continue;
            };

            let mut outputs = Vec::new();
            for (port, value) in ports {
                let Ok(port) = port.parse::<u8>() else {
                    continue;
                };
                let Ok(file) = serde_json::from_value::<FileRef>(value.clone()) else {
                    continue;
                };
                outputs.push((port, file));
            }
            if !outputs.is_empty() {
                self.dag
                    .restore_cached_file_outputs(id, fingerprint, outputs)?;
            }
        }
        Ok(())
    }

    /// Create a new ref diverging from an arbitrary snapshot, switch the
    /// engine to it, and load that snapshot's DAG into memory.
    ///
    /// This is the equivalent of `git checkout -b <branch> <commit>`: you
    /// start a new independent lineage from a historical point without
    /// affecting the original ref. Short-hash prefixes are accepted.
    pub async fn branch_from_snapshot(
        &mut self,
        snapshot_id: &str,
        new_ref_name: &str,
    ) -> Result<()> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;

        // Create the new ref pointing at the resolved snapshot.
        history
            .branch_from_snapshot(new_ref_name, snapshot_id)
            .await
            .map_err(Error::Dag)?;

        // Switch the engine to the new ref.
        self.history_ref = new_ref_name.to_string();

        // Load the snapshot's DAG into memory.
        let snap = history
            .resolve_snapshot(snapshot_id)
            .await
            .map_err(Error::Dag)?
            .ok_or_else(|| Error::Custom(format!("snapshot '{snapshot_id}' not found")))?;

        let manifest = snap
            .manifest()
            .map_err(|e| Error::Custom(format!("manifest deserialization: {e}")))?;

        self.rebuild_dag_from_manifest(&manifest)?;

        Ok(())
    }

    /// Internal helper: clear the DAG and rebuild nodes + edges from a
    /// manifest.
    fn rebuild_dag_from_manifest(&mut self, manifest: &crate::dag::DagManifest) -> Result<()> {
        manifest
            .validate_layers()
            .map_err(|e| Error::Custom(format!("manifest layer validation: {e}")))?;
        self.dag.clear();
        for entry in &manifest.nodes {
            Self::ensure_node_kind_allowed(&entry.kind)?;
            let node = if entry.kind == "logical_gather" {
                Box::new(GatherNode::default()) as Box<dyn crate::nodes::DagNode>
            } else if entry.kind == "channel" {
                Box::new(crate::dag::ChannelNode::from_spec(&entry.spec)?)
                    as Box<dyn crate::nodes::DagNode>
            } else if entry.kind == "dynamic_fanout" {
                Box::new(crate::dag::DynamicFanoutNode::default()) as Box<dyn crate::nodes::DagNode>
            } else {
                self.node_registry
                    .build_node(&entry.kind, entry.spec.clone())?
            };
            self.dag.add_node_with_spec(
                entry.id.clone(),
                node,
                entry.kind.clone(),
                entry.spec.clone(),
            )?;
        }
        for edge in &manifest.edges {
            self.dag.add_edge(
                edge.from.clone(),
                edge.to.clone(),
                edge.from_port,
                edge.to_port,
            )?;
        }
        self.dag
            .restore_execution_layers(manifest.logical.clone(), manifest.physical_jobs.clone())?;
        Ok(())
    }

    /// Add an edge from `from`'s default output port to `to`'s default input
    /// port (convenience form for single-port nodes).
    pub fn add_edge(
        &mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        from_port: u8,
        to_port: u8,
    ) -> Result<&mut Self> {
        self.dag.add_edge(from, to, from_port, to_port)?;
        Ok(self)
    }

    /// Remove the edge from `from`'s `from_port` to `to`'s `to_port`.
    ///
    /// Errors if either node does not exist or no matching edge is found.
    pub fn delete_edge(
        &mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        from_port: u8,
        to_port: u8,
    ) -> Result<()> {
        self.dag.delete_edge(from, to, from_port, to_port)?;
        Ok(())
    }

    // /// Add an edge connecting `from`'s `from_port` output port to `to`'s
    // /// `to_port` input port.
    // pub fn add_edge_port(
    //     &mut self,
    //     from: impl Into<String>,
    //     from_port: impl Into<String>,
    //     to: impl Into<String>,
    //     to_port: impl Into<String>,
    // ) -> Result<&mut Self> {
    //     self.dag.add_edge_port(from, from_port, to, to_port)?;
    //     Ok(self)
    // }
    //
    /// Replace the scheduler configuration (concurrency, retry, …).
    pub fn with_config(mut self, config: SchedulerConfig) -> Self {
        self.config = config;
        self
    }

    /// Attach a DAG history store. After this, every [`Self::run`] /
    /// [`Self::run_with_events`] will automatically commit a snapshot
    /// (manifest + run report) to the history database.
    pub fn with_history(mut self, history: DagHistory) -> Self {
        self.history = Some(history);
        self
    }

    /// Set the history ref name for this engine instance (builder style).
    ///
    /// Every [`Self::run`] will commit snapshots under this ref, forming an
    /// independent lineage. Use different ref names for unrelated analysis
    /// tasks to keep their histories separate.
    pub fn with_history_ref(mut self, ref_name: impl Into<String>) -> Self {
        self.history_ref = ref_name.into();
        self
    }

    /// Create a new **isolated engine** sharing all heavy infrastructure
    /// (NodeRegistry, RuntimeEnv, DagHistory DB) but with
    /// a fresh empty DAG and an independent history ref.
    ///
    /// This is the per-agent constructor: each agent calls this to get its
    /// own DAG graph without duplicating the ~70 node factories, DataFusion
    /// runtime, or Turso DB connection. The cost is negligible — every
    /// shared field is `Arc`-backed or `Clone`-cheap.
    ///
    /// The new engine starts with `history_ref = "main"`.
    pub fn new_isolated_engine(&self) -> Self {
        let mut dag = self.dag.empty_with_execution_policy();
        dag.set_dynamic_node_builder(Self::dynamic_node_builder(Arc::clone(&self.node_registry)));
        // Per-agent sessions answer to the same operator compute policy as
        // the main engine: without the executor, every session DAG silently
        // falls back to local execution and remote targets are never bound.
        dag.set_task_executor(Arc::clone(&self.task_executor));
        Self {
            ctx: self.ctx.clone(),
            engine_ctx: self.engine_ctx.clone(),
            task_executor: Arc::clone(&self.task_executor),
            dag,
            node_registry: Arc::clone(&self.node_registry),
            container_execution: Arc::clone(&self.container_execution),
            config: self.config.clone(),
            flow_cache: std::default::Default::default(),
            flow: std::sync::Arc::new(std::sync::RwLock::new(crate::workflow::FlowGraph::empty())),
            flow_report: std::sync::Arc::new(std::sync::RwLock::new(None)),
            history: self.history.clone(),
            history_ref: "main".to_string(),
            pending_commit_message: None,
            run_trigger: None,
        }
    }

    /// Switch the history ref at runtime.
    ///
    /// Subsequent [`Self::run`] calls will commit to the new ref. If the ref
    /// does not exist yet, the first commit creates it (as a root snapshot).
    /// The in-memory DAG is **not** affected — call [`Self::clear_dag`] and
    /// rebuild if you want to start a fresh DAG topology.
    ///
    /// Returns the previous ref name.
    pub fn set_history_ref(&mut self, ref_name: impl Into<String>) -> String {
        std::mem::replace(&mut self.history_ref, ref_name.into())
    }

    /// The current history ref name that [`Self::run`] commits to.
    pub fn history_ref(&self) -> &str {
        &self.history_ref
    }

    /// Set the commit message for the next `run()` snapshot. Consumed on the
    /// next run — if not called, a default message is used.
    pub fn set_commit_message(&mut self, message: Option<String>) {
        self.pending_commit_message = message;
    }

    /// Set who initiated the next run (e.g. `"agent:/root/researcher"`).
    /// Consumed on the next run and recorded in its run record; if not
    /// called, the run is recorded as unattributed.
    pub fn set_run_trigger(&mut self, trigger: Option<String>) {
        self.run_trigger = trigger;
    }

    /// Borrow the history store (if attached) for direct queries — e.g.
    /// `log`, `refs`, `get_snapshot`.
    pub fn history(&self) -> Option<&DagHistory> {
        self.history.as_ref()
    }

    /// Borrow the shared `NodeRegistry`. Used by the runtime client to serve
    /// metadata queries (list/spec/ports/doc) directly, bypassing the actor
    /// channel so they never block on a running DAG.
    pub fn node_registry(&self) -> &Arc<NodeRegistry> {
        &self.node_registry
    }

    /// Borrow the shared container execution infrastructure.
    pub fn container_execution(&self) -> &Arc<ContainerExecutionInfra> {
        &self.container_execution
    }

    /// Append this execution's audit record to the history `runs` table.
    ///
    /// Returns a warning string when the record could not be persisted — the
    /// run itself is never failed by an audit write.
    #[expect(dead_code, reason = "retained for legacy DAG run history")]
    async fn persist_run_record(
        &mut self,
        run_id: String,
        started_at: String,
        manifest_hash: String,
        message: Option<String>,
        report: Option<&RunReport>,
        error: Option<&str>,
    ) -> Option<String> {
        let history = self.history.clone()?;
        let record = RunRecord {
            id: run_id,
            ref_name: self.history_ref.clone(),
            // Backfilled by `commit_history_snapshot`: the snapshot committed
            // by this run, or the existing head when the manifest was
            // unchanged. `None` only when no head resolved.
            snapshot_id: report.and_then(|report| report.snapshot_id.clone()),
            manifest_hash,
            trigger: self.run_trigger.take(),
            started_at,
            finished_at: chrono::Utc::now().to_rfc3339(),
            ok: error.is_none() && report.is_some_and(|report| report.ok),
            cancelled: report.is_some_and(|report| {
                report
                    .statuses
                    .values()
                    .any(|status| matches!(status, RuntimeStatus::Cancelled))
            }),
            error: error.map(str::to_string),
            message,
            engine_version: dag_core::engine_version().to_string(),
            source_revision: dag_core::source_revision().to_string(),
            // Serialized after the snapshot commit, so `snapshot_id` is
            // backfilled here (unlike the snapshot's own embedded copy).
            run_report_json: report.and_then(|report| serde_json::to_string(report).ok()),
        };
        if let Err(persist_error) = history.record_run(&record).await {
            let warning = format!("DAG run record was not persisted: {persist_error}");
            tracing::warn!(run_id = %record.id, error = %persist_error, "{warning}");
            return Some(warning);
        }
        None
    }

    #[expect(dead_code, reason = "retained for legacy DAG run history")]
    async fn commit_history_snapshot(
        &mut self,
        manifest: &crate::dag::DagManifest,
        manifest_hash: String,
        report: &mut RunReport,
    ) {
        let Some(history) = self.history.clone() else {
            report
                .warnings
                .push("no DAG history store attached; run snapshot was not persisted".into());
            return;
        };

        let head = match history.ref_head(&self.history_ref).await {
            Ok(head) => head,
            Err(error) => {
                let warning = format!("DAG history snapshot commit failed: {error}");
                tracing::warn!(ref = %self.history_ref, error = %error, "{warning}");
                report.warnings.push(warning);
                return;
            }
        };

        if let Some(head) = head
            && head.manifest_hash == manifest_hash
        {
            // The executed definition is exactly the current head: no new
            // snapshot (the manifest did not change), but link the run to it
            // so the run record still resolves to its definition.
            report.snapshot_id = Some(head.id);
            return;
        }

        let message = self
            .pending_commit_message
            .take()
            .unwrap_or_else(|| "auto-snapshot after run".to_string());
        match history
            .commit(&self.history_ref, manifest, Some(report), &message)
            .await
        {
            Ok(snapshot_id) => report.snapshot_id = Some(snapshot_id),
            Err(error) => {
                let warning = format!("DAG history snapshot commit failed: {error}");
                tracing::warn!(ref = %self.history_ref, error = %error, "{warning}");
                report.warnings.push(warning);
            }
        }
    }

    pub async fn get_output(
        &self,
        node_id: impl Into<String>,
    ) -> Option<crate::dag::graph::PortOutputs> {
        self.dag.output(node_id.into().as_ref())
    }

    /// Release retained in-memory node outputs without changing graph topology.
    ///
    /// Returns whether anything was released — `false` means the cache was
    /// already empty, so idle maintenance must not count this as a success.
    pub fn release_output_cache(&mut self) -> bool {
        self.dag.release_output_cache()
    }

    /// Seed reusable file outputs from the manager's cross-session cache.
    ///
    /// Returns the number of nodes whose id and fingerprint both matched.
    ///
    /// Every candidate file is freshness-checked before it is trusted: a
    /// missing local file, or one whose size/mtime/content no longer match the
    /// recorded fingerprint, is skipped so the node re-executes instead of
    /// consuming stale bytes. `vfs://` outputs are validated through the
    /// mounted object storage by the same check.
    pub async fn restore_cached_file_outputs(
        &mut self,
        entries: &[crate::runtime::CachedFileOutput],
    ) -> usize {
        let storage = self.engine_ctx.opendal.as_deref();
        let mut restored = 0;
        for entry in entries {
            let fresh = match entry.file.fingerprint.as_ref() {
                // No recorded fingerprint — existence is all we can verify.
                None => {
                    vfs::uri::storage_path(&entry.file.path).is_some()
                        || std::path::Path::new(&entry.file.path).exists()
                }
                // Immutable remote content never goes stale.
                Some(fp) if fp.immutable_remote && fp.content_hash.is_some() => true,
                Some(_) => !dag_core::fingerprint::cached_file_changed(&entry.file, storage).await,
            };
            if !fresh {
                continue;
            }
            let file = if let Some(storage) = storage {
                match entry.file.clone().canonicalized(storage) {
                    Ok(file) => file,
                    Err(_) => continue,
                }
            } else {
                entry.file.clone()
            };
            let outputs = vec![(entry.port, file)];
            if self
                .dag
                .restore_cached_file_outputs(&entry.node_id, &entry.fingerprint, outputs)
                .is_ok()
            {
                restored += 1;
            }
        }
        restored
    }

    /// Maximum payload served through [`Self::read_file`]. Payload rendering
    /// (evidence citations, …) is bounded; anything larger is caller-scope
    /// data movement, not tool rendering.
    pub const MAX_READ_FILE_BYTES: usize = 4 * 1024 * 1024;

    /// Read artifact bytes for one output path: `vfs://` URIs resolve through
    /// the mounted object storage, anything else reads the host filesystem.
    ///
    /// Fails closed on payloads larger than [`Self::MAX_READ_FILE_BYTES`] so
    /// the tool layer never materializes a runaway artifact by accident.
    pub async fn read_file(&self, path: &str) -> crate::error::Result<Vec<u8>> {
        if let Some(virtual_path) = vfs::uri::storage_path(path) {
            let storage = self.engine_ctx.opendal.as_ref().ok_or_else(|| {
                crate::error::Error::Custom(format!(
                    "VFS path `{path}` requires a mounted runtime VFS"
                ))
            })?;
            let length = storage
                .content_length(virtual_path)
                .await
                .map_err(|e| crate::error::Error::Custom(e.to_string()))?;
            if length > Self::MAX_READ_FILE_BYTES as u64 {
                return Err(crate::error::Error::Custom(format!(
                    "file `{path}` exceeds read_file byte limit"
                )));
            }
            let bytes = storage
                .read_range(virtual_path, 0..length)
                .await
                .map_err(|error| {
                    crate::error::Error::Custom(format!("cannot read VFS file `{path}`: {error}"))
                })?;
            if bytes.len() > Self::MAX_READ_FILE_BYTES {
                return Err(crate::error::Error::Custom(format!(
                    "file `{path}` is {} bytes; read_file serves at most {} bytes",
                    bytes.len(),
                    Self::MAX_READ_FILE_BYTES
                )));
            }
            return Ok(bytes.to_vec());
        }

        let local = path.strip_prefix("file://").unwrap_or(path);
        let bytes = tokio::fs::read(local).await.map_err(|error| {
            crate::error::Error::Custom(format!("cannot read file `{path}`: {error}"))
        })?;
        if bytes.len() > Self::MAX_READ_FILE_BYTES {
            return Err(crate::error::Error::Custom(format!(
                "file `{path}` is {} bytes; read_file serves at most {} bytes",
                bytes.len(),
                Self::MAX_READ_FILE_BYTES
            )));
        }
        Ok(bytes)
    }

    // ── incremental execution API ──────────────────────────────────────

    /// Drop a node's recorded execution fingerprint, so the next
    /// [`Self::run`] re-executes it even in incremental mode.
    ///
    /// Use this when an external input (file, VFS dataset, API response) has
    /// changed outside the engine and the node's cached output is stale.
    /// Descendants are deliberately not invalidated: they re-evaluate through
    /// the identity chain, and are correctly reused when this node reproduces
    /// identical outputs.
    pub fn mark_node_dirty(&mut self, node_id: &str) {
        self.dag.mark_dirty(node_id);
    }

    /// Drop every recorded fingerprint — forces a full re-run on the next
    /// [`Self::run`] regardless of incremental mode.
    pub fn mark_all_dirty(&mut self) {
        self.dag.mark_all_dirty();
    }

    /// Whether a node will re-execute on the next incremental run, to the
    /// extent knowable without dispatching (no recorded fingerprint or no
    /// cached outputs).
    pub fn is_node_dirty(&self, node_id: &str) -> bool {
        self.dag.is_dirty(node_id)
    }

    /// Enable or disable incremental execution mode.
    ///
    /// When enabled, subsequent [`Self::run`] calls reuse the cached outputs
    /// of nodes whose computed execution fingerprint matches the one recorded
    /// at their last successful execution.
    pub fn set_incremental(&mut self, enabled: bool) {
        self.config.incremental = enabled;
    }

    /// Set the input identity depth for fingerprint computation (see
    /// [`dag_core::dag::InputHashing`]). `Content` additionally hashes
    /// file-like inputs that lack a recorded content hash — full
    /// Nextflow-style semantics at the cost of reading those inputs.
    pub fn set_input_hashing(&mut self, hashing: crate::dag::InputHashing) {
        self.config.input_hashing = hashing;
    }

    /// Dispatch ready physical jobs in fixed-size barrier waves. For large
    /// scans this bounds memory and gives reports stable assay-wave groups.
    pub fn set_wave_size(&mut self, wave_size: usize) -> Result<()> {
        if wave_size == 0 {
            return Err(Error::Custom("wave_size must be at least 1".into()));
        }
        self.config.wave_size = Some(wave_size);
        Ok(())
    }

    /// Query a node's runtime status. Returns `None` when the DAG has never
    /// been run (no status entry exists for the node).
    pub fn node_status(&self, node_id: &str) -> Option<crate::dag::runtime::RuntimeStatus> {
        self.dag.status(node_id)
    }
}

/// Builder for the process-wide [`DataEngine`].
pub struct DataEngineBuilder {
    runtime_env: Arc<RuntimeEnv>,
    opendal: Option<Arc<OpendalFileStorage>>,
    bundle_registry: Arc<BundleRegistry>,
    container_execution: Arc<ContainerExecutionInfra>,
    plugins_root: Option<std::path::PathBuf>,
}

impl Default for DataEngineBuilder {
    fn default() -> Self {
        let ctx = SessionContext::new();
        let runtime_env = ctx.runtime_env();
        Self {
            runtime_env,
            opendal: None,
            bundle_registry: Arc::new(BundleRegistry::new()),
            container_execution: Arc::new(ContainerExecutionInfra::from_env()),
            plugins_root: None,
        }
    }
}

impl DataEngineBuilder {
    /// Use an explicit host-owned plugin root for this engine instance.
    pub fn with_plugins_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.plugins_root = Some(root.into());
        self
    }
    pub fn register_opendal_fs(self, file_session: Arc<OpendalFileStorage>) -> Result<Self> {
        let object_url = ObjectStoreUrl::parse("file://")
            .map_err(|e| Error::Custom(format!("cannot parse datafusion url: {e}")))?;
        self.runtime_env
            .register_object_store(object_url.as_ref(), file_session.clone());
        file_session
            .register_domain_stores(&self.runtime_env)
            .map_err(|e| Error::Custom(format!("cannot register VFS domains: {e}")))?;
        // Keep the handle so artifact-producing nodes can write
        // into the virtualized filesystem instead of the host filesystem.
        Ok(Self {
            opendal: Some(file_session),
            ..self
        })
    }

    /// Install the engine-wide mapping from bundle identifiers to VFS paths.
    ///
    /// The registry is shared by every node factory and DAG session. Built-in
    /// entries are layered beneath it when the engine is built.
    pub fn with_bundle_registry(mut self, registry: BundleRegistry) -> Self {
        self.bundle_registry = Arc::new(registry);
        self
    }

    /// Inject process-wide container execution resources owned by the runtime.
    ///
    /// Tests and embedded engines may omit this and use the environment-derived
    /// default; `RuntimeHost` should always inject its shared instance.
    pub fn with_container_execution(
        mut self,
        container_execution: Arc<ContainerExecutionInfra>,
    ) -> Self {
        self.container_execution = container_execution;
        self
    }

    /// Register the Unix-style virtual filesystem under `vfs://`.
    ///
    /// All mounted backends are addressed through one namespace, for example
    /// `vfs:///data/ldscore/1000g_eur/`.
    pub fn with_vfs(mut self, vfs: MountedObjectStore) -> Self {
        let url = ObjectStoreUrl::parse("vfs://").expect("vfs:// is a valid object-store URL");
        let vfs = Arc::new(vfs);
        // When OpendalFileStorage already wraps this mount table, `vfs://`
        // must resolve to that same adapter. Otherwise replacing the separate
        // MountedObjectStore would leave VFS reads and writes with two
        // independent lock tables for one physical object.
        let shared_storage = self
            .opendal
            .as_ref()
            .filter(|storage| {
                storage.mounts.as_ref().is_some_and(|storage_mounts| {
                    storage_mounts.mount_definitions() == vfs.mount_definitions()
                })
            })
            .cloned();
        let storage = shared_storage.unwrap_or_else(|| {
            // Embedded engines also need the mount-aware byte adapter for
            // domain references. The unmapped fallback is an isolated temp FS.
            let mut storage = OpendalFileStorage::new_temp();
            storage.mounts = Some(vfs);
            Arc::new(storage)
        });
        storage
            .register_domain_stores(&self.runtime_env)
            .expect("validated VFS mount domains must be registerable URI schemes");
        self.runtime_env
            .register_object_store(url.as_ref(), storage.clone());
        self.opendal = Some(storage);
        self
    }

    pub fn build(self) -> DataEngine {
        // Keep a backward-compatible ctx for tests / ad-hoc table registration.
        let ctx = crate::node_registry::registry::new_isolated_ctx(self.runtime_env.clone());
        DataEngine::new_from_parts(
            ctx,
            self.runtime_env,
            self.opendal,
            self.bundle_registry,
            self.container_execution,
            self.plugins_root,
        )
    }
}

/// Produce a human-readable diff between two manifests.
fn format_manifest_diff(old: &crate::dag::DagManifest, new: &crate::dag::DagManifest) -> String {
    use std::collections::{HashMap as StdHashMap, HashSet};

    let old_nodes: StdHashMap<&str, &crate::dag::history::NodeEntry> =
        old.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let new_nodes: StdHashMap<&str, &crate::dag::history::NodeEntry> =
        new.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let old_edges: HashSet<String> = old
        .edges
        .iter()
        .map(|e| format!("{}.[{}] → {}.[{}]", e.from, e.from_port, e.to, e.to_port))
        .collect();
    let new_edges: HashSet<String> = new
        .edges
        .iter()
        .map(|e| format!("{}.[{}] → {}.[{}]", e.from, e.from_port, e.to, e.to_port))
        .collect();
    let old_logical = logical_manifest_entries(old);
    let new_logical = logical_manifest_entries(new);

    let mut out = String::new();
    let mut changes = 0;

    for (key, definition) in &new_logical {
        match old_logical.get(key) {
            None => {
                out.push_str(&format!("  + logical {key}\n"));
                changes += 1;
            }
            Some(old_definition) if old_definition != definition => {
                out.push_str(&format!("  ~ logical {key}\n"));
                changes += 1;
            }
            _ => {}
        }
    }
    for key in old_logical
        .keys()
        .filter(|key| !new_logical.contains_key(*key))
    {
        out.push_str(&format!("  - logical {key}\n"));
        changes += 1;
    }

    for n in &new.nodes {
        if !old_nodes.contains_key(n.id.as_str()) {
            out.push_str(&format!("  + node {} ({})\n", n.id, n.kind));
            changes += 1;
        }
    }
    for n in &old.nodes {
        if !new_nodes.contains_key(n.id.as_str()) {
            out.push_str(&format!("  - node {} ({})\n", n.id, n.kind));
            changes += 1;
        }
    }
    for n in &new.nodes {
        if let Some(old_n) = old_nodes.get(n.id.as_str())
            && (old_n.kind != n.kind || old_n.spec != n.spec)
        {
            out.push_str(&format!("  ~ node {} ({})\n", n.id, n.kind));
            changes += 1;
        }
    }
    for e in new_edges.difference(&old_edges) {
        out.push_str(&format!("  + edge {e}\n"));
        changes += 1;
    }
    for e in old_edges.difference(&new_edges) {
        out.push_str(&format!("  - edge {e}\n"));
        changes += 1;
    }
    for (id, job) in &new.physical_jobs {
        match old.physical_jobs.get(id) {
            None => {
                out.push_str(&format!("  + physical job {id} → {}\n", job.logical_node));
                changes += 1;
            }
            Some(old_job) if old_job != job => {
                out.push_str(&format!("  ~ physical job {id} → {}\n", job.logical_node));
                changes += 1;
            }
            _ => {}
        }
    }
    for id in old
        .physical_jobs
        .keys()
        .filter(|id| !new.physical_jobs.contains_key(*id))
    {
        out.push_str(&format!("  - physical job {id}\n"));
        changes += 1;
    }

    if changes == 0 {
        out.push_str("(no changes)");
    } else {
        out.push_str(&format!("\n{changes} change(s)"));
    }
    out
}

fn logical_manifest_entries(
    manifest: &crate::dag::DagManifest,
) -> BTreeMap<String, serde_json::Value> {
    manifest
        .logical
        .graphs
        .iter()
        .enumerate()
        .flat_map(|(graph_index, graph)| {
            let nodes = graph.nodes().iter().map(move |node| {
                (
                    format!("graph {graph_index} node {}", node.id),
                    serde_json::to_value(node).unwrap_or(serde_json::Value::Null),
                )
            });
            let edges = graph
                .edges()
                .iter()
                .enumerate()
                .map(move |(edge_index, edge)| {
                    (
                        format!(
                            "graph {graph_index} edge {edge_index}: {}.[{}] → {}.[{}]",
                            edge.from, edge.from_port, edge.to, edge.to_port
                        ),
                        serde_json::to_value(edge).unwrap_or(serde_json::Value::Null),
                    )
                });
            nodes.chain(edges)
        })
        .collect()
}

fn shell_failure(dry_run: bool, error: DagShellError) -> DagShellOutcome {
    DagShellOutcome {
        ok: false,
        applied: false,
        dry_run,
        committed: false,
        result: None,
        operations: Vec::new(),
        graph: serde_json::json!({}),
        error: Some(error),
    }
}

fn transaction_error(error: Error) -> DagShellError {
    let message = error.to_string();
    let operation_index = message
        .strip_prefix("operation ")
        .and_then(|rest| rest.split_once(' '))
        .and_then(|(index, _)| index.parse::<usize>().ok());
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("duplicate node") {
        "duplicate_node"
    } else if lower.contains("node factory") || lower.contains("unknown node kind") {
        "unknown_node_kind"
    } else if lower.contains("unknown node") {
        "unknown_node"
    } else if lower.contains("spec") || lower.contains("deserialize") {
        "invalid_spec"
    } else if lower.contains("port not found") || lower.contains("overconnected") {
        "port_not_found"
    } else if lower.contains("input port")
        || lower.contains("not connected")
        || lower.contains("edge")
        || lower.contains("cycle")
    {
        "invalid_edge"
    } else if lower.contains("disabled") {
        "invalid_operation"
    } else {
        "transaction_failed"
    };
    DagShellError {
        code: code.into(),
        message,
        operation_index,
        line: None,
        column: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::DataEngine;
    use crate::dag::graph::PortOutputs;
    use crate::dag::{
        ChannelOperator, DagError, DagHistory, LogicalGraph, LogicalNode, RuntimeStatus,
        SchedulerConfig,
    };
    use crate::error::Error;
    use crate::nodes::{DagNode, NodeInput, NodePorts};
    use datafusion::execution::object_store::ObjectStoreUrl;
    use datafusion::prelude::CsvReadOptions;
    use vfs::{MountedObjectStore, OpendalFileStorage, VfsManifest};

    fn datasets_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_datasets")
    }

    #[test]
    fn injected_container_execution_is_shared_by_sessions() {
        let infra = Arc::new(container_runtime::ContainerExecutionInfra::from_config(
            container_runtime::PodmanConfig::default(),
        ));
        let engine = DataEngine::builder()
            .with_container_execution(Arc::clone(&infra))
            .build();
        let session = engine.new_isolated_engine();

        assert!(Arc::ptr_eq(engine.container_execution(), &infra));
        assert!(Arc::ptr_eq(session.container_execution(), &infra));
    }

    #[tokio::test]
    async fn read_file_serves_local_paths_and_enforces_the_size_cap() {
        let engine = DataEngine::builder().build();

        // Local absolute path.
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"evidence-bytes").unwrap();
        let bytes = engine
            .read_file(file.path().to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(bytes, b"evidence-bytes");

        // Oversize payloads fail closed with the limit named.
        let big = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(big.path(), vec![0u8; DataEngine::MAX_READ_FILE_BYTES + 1]).unwrap();
        let error = engine
            .read_file(big.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("at most"), "{error}");

        // vfs:// without a mounted storage fails closed too.
        let error = engine
            .read_file("vfs://artifacts/none.json")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("mounted"), "{error}");
    }

    #[tokio::test]
    async fn clear_dag_commits_a_recoverable_pre_clear_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let history = DagHistory::open(&directory.path().join("history.db"))
            .await
            .unwrap();
        let mut engine = DataEngine::builder().build().with_history(history);
        engine
            .add_node_from_registry(
                "read",
                "file_to_dataframe",
                serde_json::json!({"path": datasets_dir().join("Iris.csv").to_string_lossy()}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "write",
                "dataframe_to_file",
                serde_json::json!({
                    "path": directory.path().join("out.csv").to_string_lossy(),
                    "format": "csv"
                }),
            )
            .unwrap();
        engine.add_edge("read", "write", 0, 0).unwrap();

        let outcome = engine.clear_dag().await.unwrap();
        assert_eq!(outcome.node_count, 2);
        assert_eq!(outcome.edge_count, 1);
        let snapshot_id = outcome.snapshot_id.expect("pre-clear snapshot");
        assert!(!engine.node_exists("read"));
        assert!(!engine.node_exists("write"));

        engine.checkout_dag(&snapshot_id).await.unwrap();
        assert!(engine.node_exists("read"));
        assert!(engine.node_exists("write"));
    }

    #[test]
    fn mounted_vfs_url_uses_the_atomic_opendal_adapter() {
        let data_dir = tempfile::tempdir().unwrap();
        let mounted_root = tempfile::tempdir().unwrap();
        let manifest = VfsManifest::local_root(mounted_root.path().to_string_lossy().to_string());
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(
            data_dir.path(),
            mounted.clone(),
        ));

        let engine = DataEngine::builder()
            .register_opendal_fs(storage.clone())
            .unwrap()
            .with_vfs((*mounted).clone())
            .build();
        let registry = engine.ctx.runtime_env().object_store_registry.clone();
        let opendal_url = ObjectStoreUrl::parse("opendal-test://").unwrap();
        let vfs_url = ObjectStoreUrl::parse("vfs://").unwrap();
        registry.register_store(opendal_url.as_ref(), storage);
        let opendal_store = registry.get_store(opendal_url.as_ref()).unwrap();
        let vfs_store = registry.get_store(vfs_url.as_ref()).unwrap();

        assert!(
            Arc::ptr_eq(&opendal_store, &vfs_store),
            "vfs:// mounted paths must use the atomic Opendal adapter"
        );
    }

    // ── Existing OpenDAL/DataFusion integration test ────────────────────

    #[tokio::test]
    async fn test_dataengine_opendal_datafusion() {
        let file_session = Arc::new(OpendalFileStorage::new_temp());
        let test_data_file = std::fs::read("test_datasets/Iris.csv").unwrap();
        file_session
            .write_bytes("/iris.csv", test_data_file)
            .await
            .unwrap();
        let builder = DataEngine::builder()
            .register_opendal_fs(file_session)
            .unwrap();
        let engine = builder.build();

        engine
            .ctx
            .register_csv("iris", "/iris.csv", CsvReadOptions::default())
            .await
            .unwrap();

        let df = engine.ctx.sql("SELECT * FROM iris LIMIT 5").await.unwrap();
        df.clone().show().await.unwrap();
        let length = df.clone().count().await.unwrap();
        assert_eq!(length, 5);
    }

    // ── DAG pipeline tests (migrated from tests/dag_pipeline.rs) ────────

    #[tokio::test]
    async fn unknown_port_rejected() {
        let mut engine = DataEngine::builder().build();
        engine
            .add_node_from_registry("a", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        engine
            .add_node_from_registry("b", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        // "a" has no output port 99 — rejected immediately at add_edge time,
        // before the edge can silently deliver nothing.
        let err = match engine.add_edge("a", "b", 99, 0) {
            Err(e) => e,
            Ok(_) => panic!("add_edge with unknown output port must be rejected"),
        };
        assert!(
            matches!(err, Error::Dag(DagError::PortNotFound { ref node, direction: "output", .. }) if node == "a"),
            "expected PortNotFound(output) at add_edge, got {err:?}"
        );
    }

    #[test]
    fn tui_snapshot_exposes_topology_and_state() {
        let mut engine = DataEngine::builder().build();
        engine
            .add_node_from_registry("b", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        engine
            .add_node_from_registry("a", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        engine.add_edge("a", "b", 0, 0).unwrap();

        let snapshot = engine.dag_tui_snapshot().unwrap();
        assert_eq!(
            snapshot
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(snapshot.edges.len(), 1);
        assert_eq!(snapshot.edges[0].from, "a");
        assert_eq!(snapshot.edges[0].to, "b");
        assert_eq!(
            snapshot.status_count(RuntimeStatus::Pending),
            snapshot.nodes.len()
        );
    }

    /// A node that always fails — for the cascade test. Source-like (no inputs).
    #[derive(Clone)]
    struct BoomNode(NodePorts);
    #[async_trait::async_trait]
    impl DagNode for BoomNode {
        fn ports(&self) -> &NodePorts {
            &self.0
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }
        fn kind(&self) -> &'static str {
            "boom"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            _ctx: &crate::node_registry::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &crate::dag::node_event::NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            Err(DagError::Schedule("kaboom".into()))
        }
    }

    /// A node that sleeps — for the parallelism test. No inputs, no real output.
    #[derive(Clone)]
    struct SleepNode(NodePorts);
    #[async_trait::async_trait]
    impl DagNode for SleepNode {
        fn ports(&self) -> &NodePorts {
            &self.0
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }
        fn kind(&self) -> &'static str {
            "sleep"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            _ctx: &crate::node_registry::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &crate::dag::node_event::NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(PortOutputs::new())
        }
    }

    // ── Registry-based node creation tests ──────────────────────────

    /// All node kinds must be discoverable via list_nodes.
    #[tokio::test]
    async fn list_nodes_returns_all_registered_kinds() {
        let engine = DataEngine::builder().build();
        let nodes = engine.list_nodes();
        let kinds: Vec<&str> = nodes.iter().map(|n| n.kind.as_str()).collect();
        for expected in [
            "sql",
            "file_to_dataframe",
            "dataframe_to_file",
            "linear_regression",
            "echo",
            "enrichment_ora",
        ] {
            assert!(
                kinds.contains(&expected),
                "missing kind '{expected}'; got {kinds:?}"
            );
        }
    }

    /// Each registered kind must have a non-null JSON Schema.
    #[tokio::test]
    async fn get_node_spec_returns_schema_for_every_kind() {
        let engine = DataEngine::builder().build();
        for kind in [
            "sql",
            "file_to_dataframe",
            "dataframe_to_file",
            "linear_regression",
            "echo",
        ] {
            let schema = engine
                .get_node_spec(kind)
                .unwrap_or_else(|e| panic!("get_node_spec({kind}) failed: {e}"));
            let raw = serde_json::to_value(&schema).unwrap();
            assert!(
                raw.is_object(),
                "schema for {kind} should be a JSON object; got {raw}"
            );
        }
    }

    /// Unknown kind → FactoryNotFound
    #[tokio::test]
    async fn add_node_via_registry_unknown_kind() {
        let mut engine = DataEngine::builder().build();
        let err = engine
            .add_node_from_registry("n", "nonexistent_kind_42", serde_json::json!({}))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("nonexistent_kind_42"),
            "error should mention the kind; got: {msg}"
        );
    }

    #[test]
    fn add_node_accepts_qualified_plugin_address() {
        let mut engine = DataEngine::builder().build();
        engine
            .add_node_from_registry(
                "qualified",
                "core/sql",
                serde_json::json!({"sql_query": "SELECT 1"}),
            )
            .unwrap();

        assert_eq!(
            engine.get_node("qualified").unwrap().0,
            "core/sql",
            "qualified addresses must be retained in DAG manifests"
        );
    }

    #[test]
    fn add_node_rejects_disabled_generic_container_kind() {
        let mut engine = DataEngine::builder().build();
        let error = engine
            .add_node_from_registry(
                "generic_container",
                "container_command",
                serde_json::json!({}),
            )
            .expect_err("container_command creation must be rejected");

        assert!(
            error.to_string().contains(
                "node kind 'container_command' is disabled; use a registered dedicated node instead"
            ),
            "unexpected error: {error}"
        );
    }

    // ── Per-kind smoke tests: create via registry + run a minimal DAG ──

    // ── update_node tests ────────────────────────────────────────────

    /// update_node rejects a non-existent node id.
    #[tokio::test]
    async fn update_node_unknown_id_rejected() {
        let mut engine = DataEngine::builder().build();
        let err = engine
            .update_node("ghost", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap_err();
        assert!(err.to_string().contains("unknown node"), "{err}");
    }

    /// update_node rejects a malformed spec.
    #[tokio::test]
    async fn update_node_bad_spec_rejected() {
        let mut engine = DataEngine::builder().build();
        engine
            .add_node_from_registry("x", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        // Empty spec missing sql_query.
        let err = engine.update_node("x", serde_json::json!({})).unwrap_err();
        assert!(
            err.to_string().contains("sql_query") || err.to_string().contains("missing field"),
            "expected deserialization error, got {err}"
        );
    }
    #[tokio::test]
    async fn run_lookup_accepts_unique_prefix_and_rejects_ambiguous_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let history = DagHistory::open(&directory.path().join("history.db"))
            .await
            .unwrap();
        for id in [
            "aaaabbbb-1111-2222-3333-444444444444",
            "aaaabbbb-2222-2222-3333-444444444444",
        ] {
            history
                .record_run(&crate::dag::RunRecord {
                    id: id.into(),
                    ref_name: "flow".into(),
                    snapshot_id: None,
                    manifest_hash: "definition".into(),
                    trigger: None,
                    started_at: "2026-11-06T00:00:00Z".into(),
                    finished_at: "2026-11-06T00:00:01Z".into(),
                    ok: true,
                    cancelled: false,
                    error: None,
                    message: None,
                    engine_version: "test".into(),
                    source_revision: "test".into(),
                    run_report_json: None,
                })
                .await
                .unwrap();
        }
        let engine = DataEngine::builder().build().with_history(history);
        assert!(
            engine
                .get_run("aaaabbbb")
                .await
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert_eq!(
            engine.get_run("aaaabbbb-111").await.unwrap().unwrap().id,
            "aaaabbbb-1111-2222-3333-444444444444"
        );
        assert!(engine.get_run("unknown").await.unwrap().is_none());
        assert!(
            engine
                .get_run("aaaabbbb-2222-2222-3333-444444444444")
                .await
                .unwrap()
                .is_some()
        );
    }
}
