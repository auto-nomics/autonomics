use std::sync::Arc;

use container_runtime::ContainerExecutionInfra;
use dag_core::BundleRegistry;
use datafusion::{
    execution::{object_store::ObjectStoreUrl, runtime_env::RuntimeEnv},
    prelude::SessionContext,
};
use vfs::{MountedObjectStore, OpendalFileStorage};

use crate::dag::{DagError, DagHistory, RunRecord};
use crate::dag_shell::{
    DagShellError, DagShellOutcome, DagShellSnapshot, GraphEditOp, execute_script,
    normalize_timeout, operation_traces,
};
use crate::error::{Error, Result};
use crate::node_registry::registry::NodeRegistry;

/// `DataEngine` is the core object that implements the data analysis engine.
/// It orchestrates ingestion, transformation, and querying of datasets via a
/// stage-model flow executed by the reactive scheduler.
pub struct DataEngine {
    ctx: SessionContext,
    /// The immutable engine ingredients, handed to the flow scheduler on every
    /// `run` so each node `execute` can build a fresh isolated `SessionContext`
    /// via `NodeCtx::session()`. Owned here (not in the graph or the nodes) so
    /// mutable `SessionContext` state can never accumulate across runs.
    pub(crate) engine_ctx: crate::node_registry::registry::NodeCtx,
    pub(crate) task_executor: Arc<dyn dag_core::TaskExecutor>,
    /// Shared, immutable after construction. Wrapped in `Arc` so that
    /// [`DataEngine::new_isolated_engine`] can share it across per-agent engines
    /// without rebuilding the ~70 factories.
    pub(crate) node_registry: Arc<NodeRegistry>,
    /// Process-wide container execution resources, injected by the runtime
    /// host and shared with the IO node registry.
    container_execution: Arc<ContainerExecutionInfra>,
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
    /// Optional run-audit store. Flow runs record a [`RunRecord`] per
    /// invocation, and the session's flow + instance cache are persisted here
    /// so a restarted agent restores its workspace.
    /// `DagHistory` wraps a `turso::Connection` (Arc-backed, `Clone`), so
    /// cloned engines share the same DB file.
    pub(crate) history: Option<DagHistory>,
    /// Default ref filter for run-record queries.
    history_ref: String,
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

    pub(crate) fn ensure_node_kind_allowed(kind: &str) -> Result<()> {
        let local_kind = kind.rsplit_once('/').map_or(kind, |(_, local)| local);
        if local_kind == Self::DISABLED_NODE_KIND {
            return Err(Error::Custom(format!(
                "node kind '{kind}' is disabled; use a registered dedicated node instead"
            )));
        }
        Ok(())
    }

    /// Replace the execution backend used for physical tasks.
    pub fn set_task_executor(&mut self, executor: std::sync::Arc<dyn dag_core::TaskExecutor>) {
        self.task_executor = executor;
    }

    /// Configure an explicit coordinator concurrency limit. This sizes the
    /// shared global semaphore every node factory sees. It does not change
    /// task CPU/memory requests, executor admission, or Slurm admission.
    pub fn set_coordinator_concurrency(&mut self, limit: usize) -> Result<()> {
        if !(1..=dag_core::dag::execution::MAX_REMOTE_TASKS).contains(&limit) {
            return Err(DagError::Schedule("invalid coordinator concurrency".into()).into());
        }
        self.engine_ctx.global_sem = Some(Arc::new(tokio::sync::Semaphore::new(limit)));
        self.refresh_coordinator_factory_context();
        Ok(())
    }

    /// Remove the coordinator dispatch gate for ordinary remote execution.
    /// Resource requests and executor/Slurm admission remain unchanged.
    /// Configure the template before creating sessions.
    pub fn set_unlimited_coordinator_concurrency(&mut self) {
        self.engine_ctx.global_sem = None;
        self.refresh_coordinator_factory_context();
    }

    fn refresh_coordinator_factory_context(&mut self) {
        let mut factory_ctx = self.node_registry.ctx().clone();
        factory_ctx.global_sem = self.engine_ctx.global_sem.clone();
        self.node_registry = Arc::new(self.node_registry.with_node_ctx(factory_ctx));
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
        Self {
            ctx,
            engine_ctx,
            task_executor: dag_core::local_task_executor(),
            node_registry,
            container_execution,
            flow_cache: std::default::Default::default(),
            flow: std::sync::Arc::new(std::sync::RwLock::new(crate::workflow::FlowGraph::empty())),
            flow_report: std::sync::Arc::new(std::sync::RwLock::new(None)),
            history: None,
            history_ref: "main".to_string(),
        }
    }

    pub fn builder() -> DataEngineBuilder {
        DataEngineBuilder::default()
    }

    /// Returns the shared session context (object stores, catalogs, …).
    pub fn ctx(&self) -> SessionContext {
        self.ctx.clone()
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

    pub fn with_history(mut self, history: DagHistory) -> Self {
        self.history = Some(history);
        self
    }

    /// Set the default ref filter for run-record queries (builder style).
    pub fn with_history_ref(mut self, ref_name: impl Into<String>) -> Self {
        self.history_ref = ref_name.into();
        self
    }

    /// Create a new **isolated engine** sharing all heavy infrastructure
    /// (NodeRegistry, RuntimeEnv, DagHistory DB) with a fresh empty flow.
    ///
    /// This is the per-agent constructor: each agent calls this to get its
    /// own flow session without duplicating the ~70 node factories,
    /// DataFusion runtime, or Turso DB connection. The cost is negligible —
    /// every shared field is `Arc`-backed or `Clone`-cheap.
    pub fn new_isolated_engine(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
            engine_ctx: self.engine_ctx.clone(),
            task_executor: Arc::clone(&self.task_executor),
            node_registry: Arc::clone(&self.node_registry),
            container_execution: Arc::clone(&self.container_execution),
            flow_cache: std::default::Default::default(),
            flow: std::sync::Arc::new(std::sync::RwLock::new(crate::workflow::FlowGraph::empty())),
            flow_report: std::sync::Arc::new(std::sync::RwLock::new(None)),
            history: self.history.clone(),
            history_ref: "main".to_string(),
        }
    }

    /// Borrow the history store (if attached) for direct queries — e.g.
    /// run records.
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

    /// Maximum payload served through [`Self::read_file`]. Payload rendering
    /// (evidence citations, …) is bounded; anything larger is caller-scope
    /// data movement, not tool rendering.
    pub const MAX_READ_FILE_BYTES: usize = 4 * 1024 * 1024;

    /// Read artifact bytes for one output path: domain URIs (`local://`,
    /// legacy `vfs://`) resolve through the mounted object storage, anything
    /// else reads the host filesystem.
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
    use crate::dag::DagHistory;
    use datafusion::execution::object_store::ObjectStoreUrl;
    use datafusion::prelude::CsvReadOptions;
    use vfs::{MountedObjectStore, OpendalFileStorage, VfsManifest};

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

        // A domain URI without a mounted storage fails closed too.
        let error = engine
            .read_file("local://artifacts/none.json")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("mounted"), "{error}");
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

    // ── Per-kind smoke tests: create via registry + run a minimal DAG ──

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
