//! The DAG data structure: a payload store + a structural index.

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;

use datafusion::common::HashMap;
use datafusion::prelude::DataFrame;
use petgraph::Direction;
use petgraph::algo::{has_path_connecting, is_cyclic_directed, kosaraju_scc, toposort};
use petgraph::dot::Dot;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info_span, warn};

use super::utils::{build_inputs, cascade_skip};

use super::error::DagError;
use super::runtime::{
    DirtyState, NodeReport, RunReport, RuntimeStatus, SchedulerConfig, SchemaReport,
};
use super::{DagNode, NodeId};
use crate::dag::node_event::{JobResult, NodeEvent, NodeEventKind, NodeReporter};
use crate::resource::{MemoryGuardConfig, MemoryObservation, MemorySample, sample_memory_usage};
use crate::value::{DataRef, FileFingerprint, FileRef, NodeValue, PortType};

/// Output values keyed by output port index.
#[derive(Debug, Clone, Default)]
pub struct PortOutputs {
    values: HashMap<u8, NodeValue>,
}

impl PortOutputs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<V: Into<NodeValue>>(&mut self, port: u8, value: V) -> Option<NodeValue> {
        self.values.insert(port, value.into())
    }

    pub fn insert_file(&mut self, port: u8, file: FileRef) -> Option<NodeValue> {
        self.values.insert(port, NodeValue::File(file))
    }

    pub fn insert_data(&mut self, port: u8, data: DataRef) -> Option<NodeValue> {
        self.values.insert(port, NodeValue::Data(data))
    }

    pub fn get(&self, port: &u8) -> Option<&NodeValue> {
        self.values.get(port)
    }

    pub fn dataframe(&self, port: u8) -> std::result::Result<&DataFrame, DagError> {
        self.values
            .get(&port)
            .ok_or_else(|| DagError::Schedule(format!("output port {port} has no value")))?
            .as_dataframe()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u8, &NodeValue)> {
        self.values.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &NodeValue> {
        self.values.values()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl std::ops::Index<&u8> for PortOutputs {
    type Output = NodeValue;

    fn index(&self, index: &u8) -> &Self::Output {
        &self.values[index]
    }
}

/// Cancels a still-live node task if the scheduler returns before the task.
///
/// Normal completed tasks ignore `abort`; a task dropped by the memory guard is
/// explicitly cancelled instead of continuing invisibly in the background.
struct AbortOnDropHandle(JoinHandle<()>);

impl Drop for AbortOnDropHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct MemoryGuardState {
    config: MemoryGuardConfig,
    sample_count: usize,
    peak: Option<MemorySample>,
    source: Option<&'static str>,
    error: Option<String>,
}

impl MemoryGuardState {
    fn new(config: MemoryGuardConfig) -> Self {
        Self {
            config,
            sample_count: 0,
            peak: None,
            source: None,
            error: None,
        }
    }

    fn record(&mut self, sample: MemorySample) -> Option<MemorySample> {
        self.sample_count += 1;
        self.source = Some(sample.source);
        let triggered = sample.ratio() >= self.config.threshold_ratio;
        if self
            .peak
            .as_ref()
            .is_none_or(|peak| peak.usage_bytes < sample.usage_bytes)
        {
            self.peak = Some(sample);
        }
        triggered.then_some(sample)
    }

    fn unavailable(&mut self, error: String) {
        self.error.get_or_insert(error);
    }

    fn into_report(self, trigger: Option<MemorySample>) -> super::runtime::ResourceRunReport {
        let memory = super::runtime::MemoryRunReport {
            enabled: true,
            source: self.source,
            threshold_ratio: Some(self.config.threshold_ratio),
            sample_interval_ms: Some(
                self.config
                    .sample_interval
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
            ),
            sample_count: self.sample_count,
            peak: self.peak.as_ref().map(Into::into),
            trigger: trigger.as_ref().map(Into::into),
            error: self.error,
        };
        super::runtime::ResourceRunReport { memory }
    }
}

async fn run_memory_monitor(config: MemoryGuardConfig, tx: mpsc::Sender<MemoryObservation>) {
    loop {
        match sample_memory_usage() {
            Ok(sample) => {
                let triggered = sample.ratio() >= config.threshold_ratio;
                if tx.send(MemoryObservation::Sample(sample)).await.is_err() {
                    break;
                }
                if triggered {
                    break;
                }
            }
            Err(error) => {
                let _ = tx.send(MemoryObservation::Unavailable(error)).await;
                break;
            }
        }
        tokio::time::sleep(config.sample_interval).await;
    }
}

pub struct DagEdge {
    #[allow(dead_code)]
    pub from_node: NodeId,
    #[allow(dead_code)]
    pub to_node: NodeId,
    #[allow(dead_code)]
    pub from_port: u8,
    #[allow(dead_code)]
    pub to_port: u8,
}

/// Metadata attached to every edge in the graph: which output port of the
/// source feeds which input port of the target. Exactly one [`DataFrame`] flows
/// along each edge.
#[derive(Debug, Clone)]
pub struct EdgeLabel {
    /// Output port name on the `from` node.
    pub from_port: u8,
    /// Input port name on the `to` node.
    pub to_port: u8,
}

/// Module-local Result alias — every fallible operation in this module fails
/// with [`DagError`].
pub type Result<T> = std::result::Result<T, DagError>;

/// The workflow graph: payload store + connectivity index.
///
/// Node payloads (`Box<dyn DagNode>`) live in a [`HashMap`] keyed by id; a
/// lightweight [`petgraph`] directed graph mirrors both the connectivity and
/// edge metadata (see [`DependencyKind`]) so we get cycle detection,
/// topological sort, predecessor/successor queries, and edge iteration for
/// free. The two are decoupled on purpose — keeping payloads out of the graph
/// lets the scheduler `clone_box` a node into a spawned task without
/// fighting the graph's borrow.
#[derive(Default)]
pub struct DAG {
    /// Node payloads, keyed by id. Public so external tooling can introspect.
    pub nodes: HashMap<NodeId, Box<dyn DagNode>>,
    /// Connectivity index (edge weight carries the port label).
    graph: DiGraph<NodeId, EdgeLabel>,
    pub id_to_idx: HashMap<NodeId, NodeIndex>,
    /// Per-node runtime status. Populated on [`Self::run`], queryable via [`Self::status`].
    pub statuses: HashMap<NodeId, RuntimeStatus>,
    outputs: HashMap<NodeId, PortOutputs>,
    errors: HashMap<NodeId, DagError>,
    /// Retained `(kind, spec)` per node, populated via [`Self::add_node_with_spec`]
    /// / [`Self::replace_node_with_spec`]. Enables manifest export for
    /// snapshot persistence without modifying the `DagNode` trait.
    specs: HashMap<NodeId, (String, serde_json::Value)>,
    /// Per-node dirty-mark state for incremental execution. A node is `Dirty`
    /// when its spec/payload/topology/upstream has changed since its last
    /// successful execution. `run` with `SchedulerConfig::incremental = true`
    /// skips `Clean` nodes and reuses their cached outputs.
    dirty: HashMap<NodeId, DirtyState>,
}

impl DAG {
    /// Query the runtime status of a node. Returns `None` if the DAG has never
    /// been run.
    pub fn status(&self, id: &str) -> Option<RuntimeStatus> {
        self.statuses.get(id).copied()
    }

    pub fn output(&self, id: &str) -> Option<PortOutputs> {
        self.outputs.get(id).cloned()
    }

    // ── dirty-mark API (incremental execution) ──────────────────────────

    /// Whether `id` is marked dirty (needs re-execution in an incremental run).
    ///
    /// A node with no dirty entry (e.g. freshly constructed DAG) is considered
    /// dirty — it has no cached output.
    pub fn is_dirty(&self, id: &str) -> bool {
        self.dirty.get(id) != Some(&DirtyState::Clean)
    }

    /// Mark `id` **and all its transitive descendants** as [`DirtyState::Dirty`].
    ///
    /// This is the core propagation primitive: any mutation that could
    /// invalidate a node's cached output calls this to ensure the node and
    /// everything downstream will be re-executed on the next incremental run.
    ///
    /// Stops at nodes already dirty (no redundant re-propagation).
    pub fn mark_dirty(&mut self, id: &str) {
        let mut queue: VecDeque<NodeId> = VecDeque::from([id.to_string()]);
        while let Some(nid) = queue.pop_front() {
            if self.dirty.get(&nid) == Some(&DirtyState::Dirty) {
                continue;
            }
            self.dirty.insert(nid.clone(), DirtyState::Dirty);
            for succ in self.successors(&nid) {
                queue.push_back(succ);
            }
        }
    }

    /// Mark **every** node dirty — forces a full re-run on the next
    /// incremental `run`. Equivalent to the default (non-incremental) behavior.
    pub fn mark_all_dirty(&mut self) {
        for id in self.nodes.keys() {
            self.dirty.insert(id.clone(), DirtyState::Dirty);
        }
    }

    /// Mark a single node [`DirtyState::Clean`] after it has been successfully
    /// executed. Internal — called from the scheduler loop.
    fn mark_clean(&mut self, id: &str) {
        self.dirty.insert(id.to_string(), DirtyState::Clean);
    }

    /// Mark clean file-producing nodes dirty when cached local artifacts no
    /// longer match their recorded fingerprints.
    fn invalidate_stale_file_outputs(&mut self) {
        let stale: Vec<NodeId> = self
            .outputs
            .iter()
            .filter(|(_, outputs)| {
                outputs.values().any(|value| match value {
                    NodeValue::File(file) => cached_file_changed(file),
                    NodeValue::FileSet(files) => files.iter().any(cached_file_changed),
                    NodeValue::Data(_) | NodeValue::DataSet(_) => false,
                    NodeValue::DataFrame(_) => false,
                })
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in stale {
            self.mark_dirty(&id);
        }
    }

    /// Remove all nodes, edges, statuses, outputs, and errors — a full reset.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.graph.clear();
        self.id_to_idx.clear();
        self.statuses.clear();
        self.outputs.clear();
        self.errors.clear();
        self.specs.clear();
        self.dirty.clear();
    }

    /// Reset all node statuses to [`RuntimeStatus::Pending`] and mark every
    /// node dirty, preparing for a full re-run.
    pub fn reset(&mut self) {
        for id in self.nodes.keys() {
            self.statuses.insert(id.clone(), RuntimeStatus::Pending);
        }
        self.mark_all_dirty();
    }

    /// Execute every node of the DAG according to its dependencies.
    ///
    /// Uses [`DagNode::clone_box`] to copy node payloads into spawned tasks so
    /// the original nodes stay in the DAG for re-runs / iterative optimisation.
    ///
    /// When [`SchedulerConfig::incremental`] is `true`, only nodes marked
    /// [`DirtyState::Dirty`] (and their dirty descendants) are re-executed.
    /// Clean nodes are skipped and their cached outputs from the previous run
    /// are reused.
    pub async fn run(
        &mut self,
        cfg: &SchedulerConfig,
        engine_ctx: &crate::registry::NodeCtx,
        event_sink: Option<mpsc::Sender<NodeEvent>>,
    ) -> Result<RunReport> {
        self.run_internal(cfg, engine_ctx, event_sink, None).await
    }

    /// Execute the DAG and propagate an external cancellation token to node
    /// tasks that have already been spawned.
    pub async fn run_with_external_cancel(
        &mut self,
        cfg: &SchedulerConfig,
        engine_ctx: &crate::registry::NodeCtx,
        event_sink: Option<mpsc::Sender<NodeEvent>>,
        external_cancel: CancellationToken,
    ) -> Result<RunReport> {
        self.run_internal(cfg, engine_ctx, event_sink, Some(external_cancel))
            .await
    }

    async fn run_internal(
        &mut self,
        cfg: &SchedulerConfig,
        engine_ctx: &crate::registry::NodeCtx,
        event_sink: Option<mpsc::Sender<NodeEvent>>,
        external_cancel: Option<CancellationToken>,
    ) -> Result<RunReport> {
        let _span = info_span!("dag_execution");
        // The immutable engine ingredients, wrapped in an Arc so each spawned
        // task can hold a cheap reference for the lifetime of its `execute`
        // call. `NodeCtx` is all-`Arc` fields, so this clone is just a few ref
        // bumps. Every node receives `&engine_ctx` and builds its own fresh,
        // isolated `SessionContext` via `NodeCtx::session()` — the graph never
        // stores or shares a `SessionContext`.
        let engine_ctx = Arc::new(engine_ctx.clone());

        let incremental = cfg.incremental;

        if !incremental {
            // Full re-run: clear all cached state.
            self.outputs.clear();
            self.statuses.clear();
            self.mark_all_dirty();
            tracing::info!("Full re-run");
        }
        // In incremental mode, keep cached outputs + statuses for clean nodes.
        // Only dirty nodes will be re-executed; clean nodes retain their
        // `Success` status and cached `outputs` from the previous run.
        if incremental {
            self.invalidate_stale_file_outputs();
            tracing::info!("Incremental execution");
        }

        self.validate()?;
        // Topological order is computed mainly to validate the graph and to seed a
        // deterministic processing order for the ready queue.
        let _topo = self.topo_order()?;

        let all_ids = self.node_ids();

        // Precompute adjacency + per-node port assignment so the dispatch loop only
        // needs a single mutable borrow of `self`.
        let mut successors: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        // (predecessor id, edge port label) per node, in declared edge order
        let mut incoming: HashMap<NodeId, Vec<(NodeId, super::graph::EdgeLabel)>> = HashMap::new();
        // unresolved-predecessor count per node
        let mut pending: HashMap<NodeId, usize> = HashMap::new();
        for id in &all_ids {
            successors.insert(id.clone(), self.successors(id));
            let preds = self.predecessors(id);
            // In incremental mode, only dirty predecessors count as
            // "unresolved" — clean predecessors already have cached outputs.
            let pending_count = if incremental {
                preds.iter().filter(|p| self.is_dirty(p)).count()
            } else {
                preds.len()
            };
            pending.insert(id.clone(), pending_count);
            let inc = self.incoming_edges_with_ports(id);
            incoming.insert(id.clone(), inc);
        }

        // Initialise runtime state for nodes that will execute.
        if incremental {
            // Only dirty nodes get reset to Pending; clean nodes keep Success.
            for id in &all_ids {
                if self.is_dirty(id) {
                    self.statuses.insert(id.clone(), RuntimeStatus::Pending);
                }
            }
        } else {
            self.statuses.clear();
            for id in &all_ids {
                self.statuses.insert(id.clone(), RuntimeStatus::Pending);
            }
        }

        let sem = Arc::new(Semaphore::new(cfg.max_concurrency.max(1)));
        let run_cancel = CancellationToken::new();
        if let Some(external_cancel) = &external_cancel {
            let internal_cancel = run_cancel.clone();
            let external_cancel = external_cancel.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = external_cancel.cancelled() => internal_cancel.cancel(),
                    _ = internal_cancel.cancelled() => {}
                }
            });
        }

        let mut memory_guard = cfg.memory_guard.map(MemoryGuardState::new);
        let mut memory_triggered: Option<MemorySample> = None;
        let (memory_tx, mut memory_rx) = mpsc::channel::<MemoryObservation>(8);
        let mut memory_monitor_active = false;
        if let Some(config) = cfg.memory_guard {
            match sample_memory_usage() {
                Ok(sample) => {
                    let triggered = memory_guard
                        .as_mut()
                        .expect("memory guard state follows configured guard")
                        .record(sample);
                    if let Some(sink) = &event_sink {
                        let _ = sink.try_send(NodeEvent::new(
                            "memory",
                            NodeEventKind::Resource {
                                usage_bytes: sample.usage_bytes,
                                limit_bytes: sample.limit_bytes,
                                usage_ratio: sample.ratio(),
                                threshold_ratio: config.threshold_ratio,
                            },
                        ));
                    }
                    if let Some(trigger) = triggered {
                        memory_triggered = Some(trigger);
                        run_cancel.cancel();
                    } else {
                        tokio::spawn(run_memory_monitor(config, memory_tx));
                        memory_monitor_active = true;
                    }
                }
                Err(error) => {
                    if let Some(state) = memory_guard.as_mut() {
                        state.unavailable(error);
                    }
                }
            }
        }
        let dirty_count = if incremental {
            all_ids.iter().filter(|id| self.is_dirty(id)).count()
        } else {
            all_ids.len()
        };
        let (tx, mut rx) = mpsc::channel::<NodeEvent>(dirty_count.max(1));

        // Per-node execution duration and skip root-cause tracking.
        let mut durations: HashMap<NodeId, std::time::Duration> = HashMap::new();
        let mut skipped_because: HashMap<NodeId, NodeId> = HashMap::new();

        // Seed the ready queue: dirty nodes whose dirty predecessors have all
        // completed (pending == 0). In non-incremental mode every node is
        // dirty, so this is equivalent to the original "source nodes first".
        let mut ready: VecDeque<NodeId> = all_ids
            .iter()
            .filter(|id| {
                if incremental && !self.is_dirty(id) {
                    return false; // clean node — skip dispatch entirely
                }
                pending[*id] == 0
            })
            .cloned()
            .collect();
        let mut in_flight: usize = 0;
        let mut job_handles: Vec<AbortOnDropHandle> = Vec::new();
        let mut external_cancellation = false;

        loop {
            if memory_triggered.is_some() {
                break;
            }

            // Dispatch every currently-ready node.
            while let Some(id) = ready.pop_front() {
                if self.statuses[&id] != RuntimeStatus::Pending {
                    // Already skipped/finished by a cascade — don't dispatch.
                    continue;
                }
                // Borrow the node payload, then clone it into an owned Box so it
                // can be moved into the 'static future. The original stays in
                // `self` for re-runs / iterative optimisation.
                let Some(node_box) = self.get_node(&id).map(|n| n.clone_box()) else {
                    warn!(node = %id, "scheduler: node payload missing");
                    continue;
                };
                let inputs = build_inputs(&id, &incoming, &self.outputs);
                if let Some(node) = self.nodes.get(&id) {
                    for input in &inputs {
                        let Some(port) = node.ports().input_port(input.port) else {
                            continue;
                        };
                        if !port.data_type.accepts(input.data.data_type()) {
                            return Err(DagError::PortTypeMismatch {
                                from_node: id.clone(),
                                from_port: input.port,
                                to_node: id.clone(),
                                to_port: input.port,
                                expected: port.data_type.to_string(),
                                actual: input.data.data_type().to_string(),
                            });
                        }
                    }
                    // An edge whose predecessor published no value on the
                    // wired output port would otherwise be dropped silently
                    // and this node executed with missing inputs. Fail the
                    // run with the exact edge instead.
                    if let Some(edges) = incoming.get(&id) {
                        for (from, edge) in edges {
                            let Some(port) = node.ports().input_port(edge.to_port) else {
                                continue;
                            };
                            if port.required
                                && !inputs.iter().any(|input| input.port == edge.to_port)
                            {
                                return Err(DagError::MissingUpstreamOutput {
                                    from_node: from.clone(),
                                    from_port: edge.from_port,
                                    to_node: id.clone(),
                                    to_port: edge.to_port,
                                });
                            }
                            if let Some(input) =
                                inputs.iter().find(|input| input.port == edge.to_port)
                            {
                                let actual_format = input
                                    .data
                                    .as_file()
                                    .ok()
                                    .and_then(|file| file.format.as_deref());
                                if !port.accepts_format(actual_format) {
                                    return Err(DagError::PortFormatMismatch {
                                        from_node: from.clone(),
                                        from_port: edge.from_port,
                                        to_node: id.clone(),
                                        to_port: edge.to_port,
                                        expected: port
                                            .format
                                            .clone()
                                            .unwrap_or_else(|| "unspecified".into()),
                                        actual: actual_format.unwrap_or("unspecified").into(),
                                    });
                                }
                            }
                        }
                    }
                }
                self.statuses.insert(id.clone(), RuntimeStatus::Running);
                in_flight += 1;
                let tx = tx.clone();
                let sem = sem.clone();
                let global_sem = engine_ctx.global_sem.clone();
                let job_id = id.clone();
                let reporter = NodeReporter::new(job_id.clone(), tx.clone());
                let engine_ctx = Arc::clone(&engine_ctx);
                let task_cancel = run_cancel.clone();
                let handle = tokio::spawn(async move {
                    // Acquire the **global** semaphore first (limits total
                    // concurrent node executions across ALL agents), then the
                    // per-run semaphore (limits concurrency within this DAG).
                    // Ordering matters: global-before-local prevents one agent's
                    // DAG from monopolising all tokio worker threads while
                    // waiting for a local permit it will never get.
                    let _global_permit = if let Some(gs) = &global_sem {
                        match tokio::select! {
                            permit = gs.acquire() => permit.ok(),
                            _ = task_cancel.cancelled() => None,
                        } {
                            Some(permit) => Some(permit),
                            None => return,
                        }
                    } else {
                        None
                    };
                    let _permit = match tokio::select! {
                        permit = sem.acquire() => permit.ok(),
                        _ = task_cancel.cancelled() => None,
                    } {
                        Some(permit) => permit,
                        None => return,
                    };
                    let mut node = node_box;
                    let start = std::time::Instant::now();

                    // Catch panics from `execute` so a crashing node is converted
                    // to a `JobResult::Failed` instead of silently dropping the
                    // `Done` signal — which would hang the scheduler (in_flight
                    // never decrements, rx.recv() blocks forever).
                    let result = tokio::select! {
                        result = AssertUnwindSafe(node.execute(&engine_ctx, &inputs, &reporter))
                            .catch_unwind() => result,
                        _ = task_cancel.cancelled() => {
                            let duration = start.elapsed();
                            let res = JobResult::Failed {
                                id: job_id.clone(),
                                error: DagError::Schedule(
                                    "node cancelled by DAG run cancellation".into(),
                                ),
                                duration,
                            };
                            let _ = tx
                                .send(NodeEvent::new(job_id, NodeEventKind::Done(res)))
                                .await;
                            return;
                        }
                    };

                    let duration = start.elapsed();
                    let res = match result {
                        Ok(Ok(outs)) => JobResult::Success {
                            id: job_id.clone(),
                            outputs: outs,
                            duration,
                        },
                        Ok(Err(error)) => {
                            warn!(node = %job_id, error = %error, "node failed");
                            JobResult::Failed {
                                id: job_id.clone(),
                                error,
                                duration,
                            }
                        }
                        Err(panic_payload) => {
                            let msg = panic_payload
                                .downcast_ref::<&str>()
                                .map(|s| (*s).to_string())
                                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "panicked with non-string payload".to_string());
                            warn!(node = %job_id, panic = %msg, "node panicked");
                            JobResult::Failed {
                                id: job_id.clone(),
                                error: DagError::Schedule(format!("node panicked: {msg}")),
                                duration,
                            }
                        }
                    };
                    // Authoritative terminal signal: use send().await so it is
                    // never dropped (ephemeral reporter events use try_send and
                    // may be lossy). Wrapping in NodeEvent keeps a single
                    // channel element type for the whole run.
                    let _ = tx
                        .send(NodeEvent::new(job_id, NodeEventKind::Done(res)))
                        .await;
                });
                job_handles.push(AbortOnDropHandle(handle));
            }

            if in_flight == 0 {
                break;
            }

            tokio::select! {
                observation = memory_rx.recv(), if memory_monitor_active => {
                    match observation {
                        Some(MemoryObservation::Sample(sample)) => {
                            let triggered = memory_guard
                                .as_mut()
                                .expect("memory guard state follows active monitor")
                                .record(sample);
                            if let Some(sink) = &event_sink {
                                let _ = sink.try_send(NodeEvent::new(
                                    "memory",
                                    NodeEventKind::Resource {
                                        usage_bytes: sample.usage_bytes,
                                        limit_bytes: sample.limit_bytes,
                                        usage_ratio: sample.ratio(),
                                        threshold_ratio: cfg.memory_guard
                                            .expect("active monitor has configured threshold")
                                            .threshold_ratio,
                                    },
                                ));
                            }
                            if let Some(trigger) = triggered {
                                memory_triggered = Some(trigger);
                                run_cancel.cancel();
                                break;
                            }
                        }
                        Some(MemoryObservation::Unavailable(error)) => {
                            if let Some(state) = memory_guard.as_mut() {
                                state.unavailable(error);
                            }
                            memory_monitor_active = false;
                        }
                        None => memory_monitor_active = false,
                    }
                },
                _ = run_cancel.cancelled(), if external_cancel.is_some() => {
                    external_cancellation = true;
                    break;
                },
                msg = rx.recv() => {
                    // Block until at least one dispatched job reports back.
                    let Some(msg) = msg else {
                        return Err(DagError::Schedule(
                            "result channel closed unexpectedly".into(),
                        ));
                    };

            // Only the authoritative `Done` drives the scheduler (decrements
            // in_flight, updates status, advances the ready queue). Ephemeral
            // observations (Status/Progress/Log) emitted mid-`execute` are
            // forwarded here but never affect scheduling correctness.
            let res = match msg.kind {
                NodeEventKind::Done(res) => res,
                // Ephemeral lightweight observations (no DataFrames): forward to
                // the external sink (if any), then continue without touching
                // in_flight or the ready queue. `Finished` is never produced
                // internally — it is emitted to the sink in the `Done` arm below.
                lightweight @ (NodeEventKind::Status { .. }
                | NodeEventKind::Progress { .. }
                | NodeEventKind::Log { .. }
                | NodeEventKind::Resource { .. }) => {
                    debug!(node = %msg.node_id, kind = ?lightweight, "node observation forwarded");
                    if let Some(sink) = &event_sink {
                        let _ = sink.try_send(NodeEvent::new(msg.node_id.clone(), lightweight));
                    }
                    continue;
                }
                NodeEventKind::Finished { .. } => continue,
            };
            in_flight -= 1;

            match res {
                JobResult::Success {
                    id,
                    outputs: outs,
                    duration,
                } => {
                    let output_type_error = self.nodes.get(&id).and_then(|node| {
                        outs.iter().find_map(|(port, value)| {
                            let declared = node.ports().output_port(*port)?;
                            if !declared.data_type.accepts(value.data_type()) {
                                return Some(DagError::PortTypeMismatch {
                                    from_node: id.clone(),
                                    from_port: *port,
                                    to_node: id.clone(),
                                    to_port: *port,
                                    expected: declared.data_type.to_string(),
                                    actual: value.data_type().to_string(),
                                });
                            }
                            let actual_format =
                                value.as_file().ok().and_then(|file| file.format.as_deref());
                            (!declared.accepts_format(actual_format)).then(|| {
                                DagError::PortFormatMismatch {
                                    from_node: id.clone(),
                                    from_port: *port,
                                    to_node: id.clone(),
                                    to_port: *port,
                                    expected: declared
                                        .format
                                        .clone()
                                        .unwrap_or_else(|| "unspecified".into()),
                                    actual: actual_format.unwrap_or("unspecified").into(),
                                }
                            })
                        })
                    });
                    if let Some(error) = output_type_error {
                        self.statuses.insert(id.clone(), RuntimeStatus::Failed);
                        self.errors.insert(id.clone(), error);
                        durations.insert(id.clone(), duration);
                        cascade_skip(
                            &id,
                            &successors,
                            &mut self.statuses,
                            &mut ready,
                            &mut skipped_because,
                        );
                    } else {
                        self.outputs.insert(id.clone(), outs);
                        self.statuses.insert(id.clone(), RuntimeStatus::Success);
                        self.mark_clean(&id);
                        self.errors.remove(&id);
                        durations.insert(id.clone(), duration);
                        debug!(node = %id, "node succeeded");
                        // External terminal observation (no DataFrame payload).
                        if let Some(sink) = &event_sink {
                            let _ = sink.try_send(NodeEvent::new(
                                &id,
                                NodeEventKind::Finished {
                                    status: RuntimeStatus::Success,
                                    elapsed_ms: duration.as_millis() as u64,
                                },
                            ));
                        }
                        for succ in &successors[&id] {
                            // In incremental mode, clean successors are never
                            // dispatched — only decrement pending for dirty ones.
                            if incremental && !self.is_dirty(succ) {
                                continue;
                            }
                            let left = {
                                let c = pending.entry(succ.clone()).or_insert(0);
                                *c = c.saturating_sub(1);
                                *c
                            };
                            if left == 0 && self.statuses[succ] == RuntimeStatus::Pending {
                                ready.push_back(succ.clone());
                            }
                        }
                    }
                }
                JobResult::Failed {
                    id,
                    error,
                    duration,
                } => {
                    self.statuses.insert(id.clone(), RuntimeStatus::Failed);
                    durations.insert(id.clone(), duration);
                    debug!(node = %id, error = %error, "node failed; cascading skip to descendants");
                    // External terminal observation (no DataFrame payload).
                    if let Some(sink) = &event_sink {
                        let _ = sink.try_send(NodeEvent::new(
                            &id,
                            NodeEventKind::Finished {
                                status: RuntimeStatus::Failed,
                                elapsed_ms: duration.as_millis() as u64,
                            },
                        ));
                    }
                    self.errors.insert(id.clone(), error);
                    cascade_skip(
                        &id,
                        &successors,
                        &mut self.statuses,
                        &mut ready,
                        &mut skipped_because,
                    );
                }
            }
                }
            }
        }

        run_cancel.cancel();
        drop(job_handles);

        let memory_trigger = memory_triggered;
        if memory_trigger.is_some() || external_cancellation {
            if let Some(sample) = &memory_trigger {
                let threshold_ratio = cfg
                    .memory_guard
                    .expect("triggered guard has configured threshold")
                    .threshold_ratio;
                for (id, status) in self.statuses.iter() {
                    if *status == RuntimeStatus::Running {
                        self.errors.insert(
                            id.clone(),
                            DagError::MemoryLimitExceeded {
                                usage_bytes: sample.usage_bytes,
                                limit_bytes: sample.limit_bytes,
                                usage_ratio: sample.ratio(),
                                threshold_ratio,
                            },
                        );
                    }
                }
            }

            for (id, status) in self.statuses.iter() {
                if *status == RuntimeStatus::Running
                    && let Some(sink) = &event_sink
                {
                    let _ = sink.try_send(NodeEvent::new(
                        id,
                        NodeEventKind::Finished {
                            status: RuntimeStatus::Cancelled,
                            elapsed_ms: 0,
                        },
                    ));
                }
            }
            for status in self.statuses.values_mut() {
                if matches!(
                    status,
                    RuntimeStatus::Pending | RuntimeStatus::Ready | RuntimeStatus::Running
                ) {
                    *status = RuntimeStatus::Cancelled;
                }
            }
        }

        let ok = !self
            .statuses
            .values()
            .any(|s| matches!(s, RuntimeStatus::Failed | RuntimeStatus::Cancelled));

        let mut warnings = Vec::new();
        if let Some(sample) = &memory_trigger {
            warnings.push(format!(
                "DAG run cancelled by memory guard: usage {} / limit {} bytes ({:.1}%); reduce node fan-out or lower max_concurrency",
                sample.usage_bytes,
                sample.limit_bytes,
                sample.ratio() * 100.0
            ));
        }

        let resource = match memory_guard {
            Some(state) => state.into_report(memory_trigger),
            None => super::runtime::ResourceRunReport::default(),
        };

        // Build per-node reports for the agent-friendly result.
        let node_reports = self
            .build_node_reports(
                &all_ids,
                &durations,
                &skipped_because,
                cfg.compute_row_counts && memory_trigger.is_none(),
            )
            .await;

        Ok(RunReport {
            ok,
            warnings,
            snapshot_id: None,
            resource,
            nodes: node_reports,
            statuses: self.statuses.clone(),
            errors: self.errors.drain().collect(),
        })
    }

    /// Build per-node [`NodeReport`] summaries from the execution state
    /// available after the scheduler loop.
    ///
    /// Row counts (`output_rows`) require running `SELECT COUNT(*)` over the
    /// LogicalPlan of every successful node's primary output DataFrame. That is
    /// an **eager** operation — for a source over a multi-GB VCF.gz it forces
    /// the file to be decompressed, parsed, and counted end-to-end. We never
    /// compute row counts here unless the caller explicitly opted in via
    /// [`SchedulerConfig::compute_row_counts`]. When opted in, the per-node
    /// count futures are joined concurrently via `futures::future::join_all`
    /// so the count phase runs in parallel rather than sequentially.
    async fn build_node_reports(
        &self,
        all_ids: &[NodeId],
        durations: &HashMap<NodeId, std::time::Duration>,
        skipped_because: &HashMap<NodeId, NodeId>,
        compute_row_counts: bool,
    ) -> Vec<NodeReport> {
        // Only run `count()` when the caller opted in. Default is off, so a
        // VCF source node does NOT trigger full decompression/parse just to
        // surface `output_rows` in the report.
        let counts: HashMap<NodeId, usize> = if compute_row_counts {
            self.collect_row_counts(all_ids).await
        } else {
            HashMap::new()
        };

        all_ids
            .iter()
            .map(|id| {
                let status = self
                    .statuses
                    .get(id)
                    .copied()
                    .unwrap_or(RuntimeStatus::Pending);
                let node_type = self
                    .nodes
                    .get(id)
                    .map(|n| n.kind())
                    .unwrap_or("unknown")
                    .to_string();
                let output_type = self
                    .outputs
                    .get(id)
                    .and_then(|outputs| outputs.values().next())
                    .map(|value| value.data_type().to_string());
                let port_assignments = self
                    .outputs
                    .get(id)
                    .and_then(|outputs| {
                        self.nodes.get(id).map(|node| {
                            node.ports()
                                .output_ports()
                                .iter()
                                .filter_map(|port| {
                                    outputs
                                        .get(&port.index)
                                        .and_then(|value| value.as_file().ok())
                                        .cloned()
                                        .map(|file| (port.index, file))
                                })
                                .collect::<std::collections::BTreeMap<_, _>>()
                        })
                    })
                    .unwrap_or_default();
                let output_files = self
                    .outputs
                    .get(id)
                    .map(|outputs| {
                        outputs
                            .values()
                            .flat_map(|value| match value {
                                NodeValue::File(file) => vec![file.clone()],
                                NodeValue::FileSet(files) => files.clone(),
                                NodeValue::Data(data) => vec![data.to_file_ref()],
                                NodeValue::DataSet(data) => {
                                    data.iter().map(DataRef::to_file_ref).collect()
                                }
                                NodeValue::DataFrame(_) => Vec::new(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                // Extract output schema from the first output port's DataFrame.
                // `schema()` only inspects the LogicalPlan — it does NOT trigger
                // execution, so it's safe (and free) to query unconditionally.
                // Wide schemas are folded to a leading-column sample + type
                // distribution (see `SchemaReport`).
                let output_schema = self
                    .outputs
                    .get(id)
                    .and_then(|outputs| {
                        outputs.values().find_map(|value| value.as_dataframe().ok())
                    })
                    .map(|df| SchemaReport::from_fields(df.schema().fields()));

                let output_rows = counts.get(id).copied();
                let elapsed_ms = durations.get(id).map(|d| d.as_millis() as u64);

                // Extract file sink path / artifact path via DagNode trait hooks.
                let file_path = self
                    .nodes
                    .get(id)
                    .and_then(|n| n.sink_path().map(|s| s.to_string()));

                let artifact_path = self
                    .nodes
                    .get(id)
                    .and_then(|n| n.artifact_path().map(|s| s.to_string()));

                let error = self.errors.get(id).map(|e| e.to_report());
                let skipped_because = skipped_because.get(id).cloned();

                NodeReport {
                    id: id.clone(),
                    status,
                    node_type,
                    output_type,
                    output_files,
                    port_assignments,
                    output_schema,
                    output_rows,
                    elapsed_ms,
                    artifact_path,
                    file_path,
                    error,
                    skipped_because,
                }
            })
            .collect()
    }

    /// Drive `df.count()` for every successful node's primary output DataFrame
    /// concurrently. Returns an empty map if no node produced output.
    ///
    /// **This is the only place in the runtime that voluntarily does eager I/O
    /// on output DataFrames.** Callers must gate it behind
    /// `SchedulerConfig::compute_row_counts` — invoking it unconditionally
    /// would force every source (e.g. `.vcf.gz`) to be fully scanned.
    ///
    /// Execution model: each count future is spawned on the runtime and we
    /// `join_all` them so multiple nodes' counts run in parallel instead of
    /// blocking the report on the slowest single scan.
    async fn collect_row_counts(&self, all_ids: &[NodeId]) -> HashMap<NodeId, usize> {
        // Build a list of (id, owned future). Cloning `DataFrame` is cheap (it
        // wraps an `Arc<LogicalPlan>`), so we can move each clone into its own
        // future without contention.
        let mut pairs: Vec<(
            NodeId,
            futures::future::BoxFuture<'static, datafusion::error::Result<usize>>,
        )> = Vec::new();
        for id in all_ids {
            let Some(dfs) = self.outputs.get(id) else {
                continue;
            };
            let Some(df) = dfs.values().find_map(|value| value.as_dataframe().ok()) else {
                continue;
            };
            let owned = df.clone();
            let id_owned = id.clone();
            pairs.push((id_owned, Box::pin(async move { owned.count().await })));
        }

        // Join all counts concurrently. If any future panics or returns an
        // error, fall back to 0 for that node — the row count is purely
        // diagnostic and shouldn't poison the report.
        let futs: Vec<_> = pairs.into_iter().map(|(_, f)| f).collect();
        let results = futures::future::join_all(futs).await;

        // We need the ids in the order that matches `results`. Rebuild the
        // ordered list by walking `all_ids` and skipping nodes that had no
        // output DataFrame — keeping the result-index aligned with the source
        // ordering.
        let mut counts: HashMap<NodeId, usize> = HashMap::new();
        let mut result_iter = results.into_iter();
        for id in all_ids {
            if self
                .outputs
                .get(id)
                .and_then(|d| d.values().next())
                .is_some()
                && let Some(res) = result_iter.next()
            {
                counts.insert(id.clone(), res.unwrap_or(0));
            }
        }
        counts
    }
}

impl DAG {
    /// Register a node under `id`. Errors if the id is already taken.
    pub fn add_node(&mut self, id: NodeId, node: Box<dyn DagNode>) -> Result<()> {
        if self.nodes.contains_key(&id) {
            return Err(DagError::DuplicateNode(id));
        }
        let idx = self.graph.add_node(id.clone());
        self.id_to_idx.insert(id.clone(), idx);
        self.nodes.insert(id.clone(), node);
        // New node has no cached output — must be executed.
        self.dirty.insert(id, DirtyState::Dirty);
        Ok(())
    }

    /// Like [`Self::add_node`] but also retains the `(kind, spec)` pair so the
    /// node can be serialized into a manifest for snapshot persistence.
    pub fn add_node_with_spec(
        &mut self,
        id: NodeId,
        node: Box<dyn DagNode>,
        kind: String,
        spec: serde_json::Value,
    ) -> Result<()> {
        self.add_node(id.clone(), node)?;
        self.specs.insert(id, (kind, spec));
        Ok(())
    }

    /// Add an edge from `from`'s `from_port` output port to `to`'s `to_port`
    /// input port. Enforces the strict 1:1 rule on declared input ports at
    /// insertion time (does not defer to [`Self::validate`]).
    ///
    /// Both endpoints must name ports the nodes actually declare (variadic
    /// targets may extend beyond their declared input ports) — an edge to a
    /// nonexistent port is rejected immediately rather than being accepted
    /// silently and later delivering no input.
    pub fn add_edge(
        &mut self,
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
        from_port: u8,
        to_port: u8,
    ) -> Result<()> {
        let from = from.into();
        let to = to.into();
        self.resolve_nodes(&from, &to)?;
        if self.nodes[&from].is_terminal() {
            return Err(DagError::Schedule(format!(
                "node `{from}` is terminal and cannot have downstream edges"
            )));
        }

        // Port existence — reject out-of-range indices here instead of
        // letting the edge validate but never deliver a value.
        if self.nodes[&from].ports().output_port(from_port).is_none() {
            return Err(DagError::PortNotFound {
                node: from.clone(),
                port: from_port,
                direction: "output",
            });
        }
        let to_ports = self.nodes[&to].ports();
        if to_ports.is_fixed_input() && to_ports.input_port(to_port).is_none() {
            return Err(DagError::PortNotFound {
                node: to.clone(),
                port: to_port,
                direction: "input",
            });
        }

        // Enforce strict 1:1 on declared input ports at edge-insertion time.
        if to_ports.is_fixed_input() && to_ports.input_port(to_port).is_some() {
            self.ensure_port_available(&to, to_port)?;
        }

        // Validate schema compatibility for this edge before inserting it.
        self.validate_edge_schema(&from, from_port, &to, to_port)?;

        if let (Some(&a), Some(&b)) = (self.id_to_idx.get(&from), self.id_to_idx.get(&to)) {
            // Reject if adding `from -> to` would close a cycle, i.e. `to` can
            // already reach `from` (also covers the self-loop case where from==to).
            if has_path_connecting(&self.graph, b, a, None) {
                return Err(DagError::Cycle(format!(
                    "adding edge {from} -> {to} would create a cycle"
                )));
            }
            self.graph.add_edge(a, b, EdgeLabel { from_port, to_port });
        }
        // The target node's input set changed — it and all descendants need
        // re-execution.
        self.mark_dirty(&to);
        Ok(())
    }
    fn resolve_nodes(&self, from: &str, to: &str) -> Result<()> {
        if !self.nodes.contains_key(from) {
            return Err(DagError::UnknownNode(from.to_string()));
        }

        if !self.nodes.contains_key(to) {
            return Err(DagError::UnknownNode(to.to_string()));
        };

        Ok(())
    }

    /// Reject if `node`'s input `port` already has an incoming edge.
    fn ensure_port_available(&self, node: &str, port: u8) -> Result<()> {
        let Some(&idx) = self.id_to_idx.get(node) else {
            return Ok(());
        };
        if self
            .graph
            .edges_directed(idx, Direction::Incoming)
            .any(|e| e.weight().to_port == port)
        {
            return Err(DagError::PortOverconnected {
                node: node.to_string(),
                port,
            });
        }
        Ok(())
    }

    pub fn delete_node(&mut self, id: &str) -> Result<()> {
        let target_node_idx = *self
            .id_to_idx
            .get(id)
            .ok_or_else(|| DagError::UnknownNode(id.to_string()))?;
        let successors = self.successors(id);
        if !successors.is_empty() {
            return Err(DagError::Schedule(format!(
                "Cannot delete node `{id}`: the following successor node(s) still depend on it: [{}]. \
                 Remove those nodes (or their incoming edges) first.",
                successors.join(", ")
            )));
        }
        self.graph.remove_node(target_node_idx);
        self.nodes.remove(id);
        self.id_to_idx.remove(id);
        // petgraph's Graph uses swap-remove: if the removed node wasn't the
        // last, the trailing node was moved into its slot, changing its
        // NodeIndex.  Update the id → index mapping for the swapped node.
        if let Some(swapped_id) = self.graph.node_weight(target_node_idx) {
            self.id_to_idx.insert(swapped_id.clone(), target_node_idx);
        }
        self.statuses.remove(id);
        self.outputs.remove(id);
        self.specs.remove(id);
        self.dirty.remove(id);
        Ok(())
    }

    /// Remove the edge from `from`'s `from_port` to `to`'s `to_port`.
    ///
    /// Returns [`DagError::UnknownNode`] if either endpoint does not exist, or
    /// [`DagError::EdgeNotFound`] if no matching edge is present.
    pub fn delete_edge(
        &mut self,
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
        from_port: u8,
        to_port: u8,
    ) -> Result<()> {
        let from = from.into();
        let to = to.into();
        self.resolve_nodes(&from, &to)?;

        let &a = self
            .id_to_idx
            .get(&from)
            .ok_or_else(|| DagError::CannotResolveNodeIdx {
                node_id: from.clone(),
            })?;
        let &b = self
            .id_to_idx
            .get(&to)
            .ok_or_else(|| DagError::CannotResolveNodeIdx {
                node_id: to.clone(),
            })?;

        let edge_id = self
            .graph
            .edges_connecting(a, b)
            .find(|e| e.weight().from_port == from_port && e.weight().to_port == to_port)
            .map(|e| e.id());

        match edge_id {
            Some(id) => {
                self.graph.remove_edge(id);
                // The target lost an input — it and its descendants are stale.
                self.mark_dirty(&to);
                Ok(())
            }
            None => Err(DagError::EdgeNotFound {
                from,
                from_port,
                to,
                to_port,
            }),
        }
    }

    /// Replace the payload of an existing node, keeping its id, `NodeIndex`,
    /// and all incoming/outgoing edges intact.
    ///
    /// Before swapping, every edge touching this node is re-validated:
    ///   - the port referenced by the edge must still exist on the new node's
    ///     `NodePorts`
    ///   - if both endpoints declare a port schema, they must remain compatible
    ///
    /// If any edge fails validation the replacement is rejected and the old
    /// payload is left untouched (atomic — no partial state).
    pub fn replace_node(&mut self, id: &str, new_node: Box<dyn DagNode>) -> Result<()> {
        let &idx = self
            .id_to_idx
            .get(id)
            .ok_or_else(|| DagError::UnknownNode(id.to_string()))?;

        let new_ports = new_node.ports();

        // Validate incoming edges: each upstream's from_port must exist on the
        // source node (unchanged) and the to_port must exist on the new node.
        for e in self.graph.edges_directed(idx, Direction::Incoming) {
            let from = &self.graph[e.source()];
            let label = e.weight();
            let from_node = self.nodes.get(from).map(|b| b.as_ref());
            let Some(from_node) = from_node else {
                continue;
            };
            let from_ports = from_node.ports();

            // Source port still exists.
            if from_ports.output_port(label.from_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: from.clone(),
                    port: label.from_port,
                    direction: "output",
                });
            }
            // Target port must exist on the new node.
            if new_ports.is_fixed_input() && new_ports.input_port(label.to_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: id.to_string(),
                    port: label.to_port,
                    direction: "input",
                });
            }
            // Schema compatibility.
            self.validate_edge_schema(from, label.from_port, id, label.to_port)?;
        }

        // Validate outgoing edges: each from_port must exist on the new node
        // and the target port must still exist on the downstream node.
        for e in self.graph.edges_directed(idx, Direction::Outgoing) {
            let to = &self.graph[e.target()];
            let label = e.weight();
            let to_node = self.nodes.get(to).map(|b| b.as_ref());
            let Some(to_node) = to_node else {
                continue;
            };
            let to_ports = to_node.ports();

            // Source port must exist on the new node.
            if new_ports.output_port(label.from_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: id.to_string(),
                    port: label.from_port,
                    direction: "output",
                });
            }
            // Target port still exists on downstream.
            if to_ports.is_fixed_input() && to_ports.input_port(label.to_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: to.clone(),
                    port: label.to_port,
                    direction: "input",
                });
            }
            // Schema compatibility.
            self.validate_edge_schema(id, label.from_port, to, label.to_port)?;
        }

        // All edges valid — swap the payload.
        self.nodes.insert(id.to_string(), new_node);

        // Old outputs are stale; clear them so a re-run produces fresh results.
        self.outputs.remove(id);
        self.errors.remove(id);
        self.statuses.insert(id.to_string(), RuntimeStatus::Pending);
        // Propagate dirty to this node + all transitive descendants.
        self.mark_dirty(id);
        Ok(())
    }

    /// All node ids, in graph (arbitrary) order.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().cloned().collect()
    }

    /// Direct predecessors of `id`.
    pub fn predecessors(&self, id: &str) -> Vec<NodeId> {
        let Some(&idx) = self.id_to_idx.get(id) else {
            return Vec::new();
        };
        self.graph
            .neighbors_directed(idx, Direction::Incoming)
            .map(|i| self.graph[i].clone())
            .collect()
    }

    /// Direct successors of `id`.
    pub fn successors(&self, id: &str) -> Vec<NodeId> {
        let Some(&idx) = self.id_to_idx.get(id) else {
            return Vec::new();
        };
        self.graph
            .neighbors_directed(idx, Direction::Outgoing)
            .map(|i| self.graph[i].clone())
            .collect()
    }

    /// Incoming edges for `id`, in insertion order. Returns predecessor ids.
    pub fn incoming_edges(&self, id: &str) -> Vec<NodeId> {
        self.incoming_edges_with_ports(id)
            .into_iter()
            .map(|(nid, _)| nid)
            .collect()
    }

    /// Incoming edges for `id` with their port labels, in insertion order.
    /// Returns `(predecessor_id, edge_label)` pairs.
    pub fn incoming_edges_with_ports(&self, id: &str) -> Vec<(NodeId, EdgeLabel)> {
        let Some(&idx) = self.id_to_idx.get(id) else {
            return Vec::new();
        };
        self.graph
            .edges_directed(idx, Direction::Incoming)
            .map(|e| (self.graph[e.source()].clone(), e.weight().clone()))
            .collect()
    }

    /// All edges in graph insertion order.
    ///
    /// This is the stable topology view needed by exporters and UI snapshots;
    /// per-node predecessor/successor queries do not expose edge ports.
    pub fn edges(&self) -> Vec<DagEdge> {
        self.graph
            .edge_references()
            .map(|edge| DagEdge {
                from_node: self.graph[edge.source()].clone(),
                to_node: self.graph[edge.target()].clone(),
                from_port: edge.weight().from_port,
                to_port: edge.weight().to_port,
            })
            .collect()
    }

    /// Validate the graph: cycles, port wiring, payload types, and schemas.
    ///
    /// Checks (in order):
    /// 1. No cycles.
    /// 2. Every edge references an existing output port, and — for fixed-input
    ///    nodes — an existing input port (variadic nodes accept undeclared
    ///    input ports).
    /// 3. The default-port `add_edge` form was only used on single-port nodes.
    /// 4. Each input port has at most one incoming edge (strict 1:1).
    /// 5. Every declared input port has exactly one incoming edge.
    /// 6. Connected payload types are compatible.
    /// 7. Where both endpoints are DataFrame ports with schemas, the output
    ///    schema covers the input schema's required fields with compatible types.
    pub fn validate(&self) -> Result<()> {
        if is_cyclic_directed(&self.graph) {
            return Err(DagError::Cycle(self.cycle_node_names()));
        }
        self.validate_port_wiring()?;
        self.validate_schemas()?;
        Ok(())
    }

    /// Port existence, default-edge disambiguation, strict-1:1, and completeness.
    fn validate_port_wiring(&self) -> Result<()> {
        // (node, port) pairs that have at least one incoming edge.
        let mut connected: std::collections::HashSet<(NodeId, u8)> =
            std::collections::HashSet::new();

        for edge in self.graph.edge_references() {
            let from = self.graph[edge.source()].clone();
            let to = self.graph[edge.target()].clone();
            let label = edge.weight();
            let from_ports = self.nodes[&from].ports();
            let to_ports = self.nodes[&to].ports();

            // Port existence.
            if from_ports.output_port(label.from_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: from,
                    port: label.from_port,
                    direction: "output",
                });
            }

            if to_ports.is_fixed_input() && to_ports.input_port(label.to_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: to.clone(),
                    port: label.to_port,
                    direction: "input",
                });
            }

            // Strict 1:1 on the input port (shared with add_edge).
            if !connected.insert((to.clone(), label.to_port)) {
                return Err(DagError::PortOverconnected {
                    node: to,
                    port: label.to_port,
                });
            }
        }

        // Completeness: every required declared input port must have an edge.
        for id in self.nodes.keys() {
            let meta = self.nodes[id].ports();
            for port in meta.input_ports().iter() {
                if port.required && !connected.contains(&((*id).clone(), port.index)) {
                    return Err(DagError::PortDisconnected {
                        node: id.clone(),
                        port: port.index,
                    });
                }
            }
        }
        Ok(())
    }

    /// Schema compatibility between connected ports (skipped when either side's
    /// schema is `None`). Iterates every edge and delegates the per-edge check
    /// to [`Self::validate_edge_schema`], which is also used at `add_edge` time.
    fn validate_schemas(&self) -> Result<()> {
        for edge in self.graph.edge_references() {
            let from = &self.graph[edge.source()];
            let to = &self.graph[edge.target()];
            let label = edge.weight();
            self.validate_edge_schema(from, label.from_port, to, label.to_port)?;
        }
        Ok(())
    }

    /// Validate payload-type and schema compatibility for a single edge.
    fn validate_edge_schema(&self, from: &str, from_port: u8, to: &str, to_port: u8) -> Result<()> {
        let (Some(from_node), Some(to_node)) = (self.nodes.get(from), self.nodes.get(to)) else {
            return Ok(());
        };
        let from_port_field = from_node.ports().output_port(from_port);
        let to_port_field = to_node.ports().input_port(to_port);
        let (Some(fp), Some(tp)) = (from_port_field, to_port_field) else {
            return Ok(());
        };
        if !fp.data_type.accepts(tp.data_type) {
            return Err(DagError::PortTypeMismatch {
                from_node: from.to_string(),
                from_port,
                to_node: to.to_string(),
                to_port,
                expected: tp.data_type.to_string(),
                actual: fp.data_type.to_string(),
            });
        }
        if !tp.accepts_format(fp.format.as_deref()) {
            return Err(DagError::PortFormatMismatch {
                from_node: from.to_string(),
                from_port,
                to_node: to.to_string(),
                to_port,
                expected: tp.format.clone().unwrap_or_else(|| "unspecified".into()),
                actual: fp.format.clone().unwrap_or_else(|| "unspecified".into()),
            });
        }
        let (PortType::DataFrame, PortType::DataFrame, Some(out_schema), Some(in_schema)) = (
            fp.data_type,
            tp.data_type,
            fp.schema.as_ref(),
            tp.schema.as_ref(),
        ) else {
            return Ok(());
        };
        if let Err(reason) = schema_compatible(out_schema, in_schema) {
            return Err(DagError::SchemaMismatch {
                from_node: from.to_string(),
                from_port,
                to_node: to.to_string(),
                to_port,
                reason,
            });
        }
        Ok(())
    }

    /// Topological order (predecessors before successors). Errors on a cycle.
    pub fn topo_order(&self) -> Result<Vec<NodeId>> {
        match toposort(&self.graph, None) {
            Ok(order) => Ok(order.iter().map(|i| self.graph[*i].clone()).collect()),
            Err(_) => Err(DagError::Cycle(self.cycle_node_names())),
        }
    }

    /// Borrow a node payload by id. Returns the trait object directly — no
    /// `Box` in the return type, since callers only want to call methods on
    /// the node (or take a fresh `Box<dyn DagNode>` themselves if they need
    /// ownership).
    pub fn get_node(&self, id: &str) -> Option<&dyn DagNode> {
        self.nodes.get(id).map(|b| b.as_ref())
    }

    /// Build an owned snapshot for interactive DAG consumers.
    ///
    /// Node order is id-sorted so repeated snapshots have stable selection and
    /// layout, independent of the graph payload HashMap's iteration order.
    pub fn tui_snapshot(&self) -> super::view::DagTuiSnapshot {
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for id in self.node_ids() {
            let Some(node) = self.nodes.get(&id) else {
                continue;
            };
            let ports = node.ports();
            nodes.push(super::view::DagNodeView {
                kind: node.kind().to_string(),
                status: self.status(&id).unwrap_or_default(),
                dirty: self.is_dirty(&id),
                inputs: ports
                    .input_ports()
                    .iter()
                    .map(|port| super::view::DagPortView {
                        index: port.index,
                        label: port.label.clone(),
                        data_type: port.data_type.to_string(),
                    })
                    .collect(),
                outputs: ports
                    .output_ports()
                    .iter()
                    .map(|port| super::view::DagPortView {
                        index: port.index,
                        label: port.label.clone(),
                        data_type: port.data_type.to_string(),
                    })
                    .collect(),
                id,
            });
        }
        nodes.sort_unstable_by(|a, b| a.id.cmp(&b.id));

        let edges = self
            .edges()
            .into_iter()
            .map(|edge| super::view::DagEdgeView {
                from: edge.from_node,
                from_port: edge.from_port,
                to: edge.to_node,
                to_port: edge.to_port,
            })
            .collect();

        super::view::DagTuiSnapshot { nodes, edges }
    }

    /// Build a human-readable cycle path like `A → B → C → A` from the first
    /// strongly-connected component that contains a cycle.
    ///
    /// Uses DFS within the SCC to recover an actual cycle (not just the node set).
    fn cycle_node_names(&self) -> String {
        let sccs = kosaraju_scc(&self.graph);
        for scc in sccs {
            let cyclic = scc.len() > 1
                || scc
                    .first()
                    .map(|&i| self.graph.neighbors(i).any(|j| j == i))
                    .unwrap_or(false);
            if !cyclic {
                continue;
            }
            // Collect node ids in this SCC and map from NodeIndex → node id.
            let ids: Vec<String> = scc.iter().map(|&i| self.graph[i].clone()).collect();
            let idx_set: std::collections::HashSet<NodeIndex> = scc.iter().copied().collect();
            // DFS to find an actual cycle path within the SCC.
            if let Some(path) = self.find_cycle_path(&idx_set) {
                let names: Vec<&str> = path.iter().map(|&i| self.graph[i].as_str()).collect();
                return names.join(" → ");
            }
            // Fallback: list the SCC members (shouldn't happen for a cyclic SCC).
            return ids.join(", ");
        }
        String::from("<unknown>")
    }

    /// DFS within a known SCC to recover one concrete cycle path.
    ///
    /// Returns a vec of [`NodeIndex`] forming a cycle (first element == last).
    fn find_cycle_path(
        &self,
        idx_set: &std::collections::HashSet<NodeIndex>,
    ) -> Option<Vec<NodeIndex>> {
        // Try DFS from each node in the SCC until we find a back-edge.
        let start = *idx_set.iter().next()?;
        let mut stack: Vec<NodeIndex> = vec![start];
        let mut on_stack: std::collections::HashSet<NodeIndex> =
            std::collections::HashSet::from([start]);
        let mut visited: std::collections::HashSet<NodeIndex> =
            std::collections::HashSet::from([start]);

        loop {
            let &current = stack.last()?;
            // Look for a successor that is still on the stack (back-edge = cycle).
            for neighbor in self.graph.neighbors_directed(current, Direction::Outgoing) {
                if !idx_set.contains(&neighbor) {
                    continue;
                }
                if on_stack.contains(&neighbor) {
                    // Found a cycle: extract the portion from `neighbor` to end.
                    let cycle_start = stack.iter().position(|&n| n == neighbor).unwrap();
                    let mut path: Vec<NodeIndex> = stack[cycle_start..].to_vec();
                    path.push(neighbor);
                    return Some(path);
                }
                if !visited.contains(&neighbor) {
                    visited.insert(neighbor);
                    on_stack.insert(neighbor);
                    stack.push(neighbor);
                    break; // continue DFS from the pushed neighbor
                }
            }
            // If no unvisited successor was pushed, backtrack.
            if *stack.last().unwrap() == current {
                // We processed all neighbors without finding a cycle — pop and try next.
                on_stack.remove(&current);
                stack.pop();
                if stack.is_empty() {
                    return None;
                }
            }
        }
    }

    /// Render DAG topology into dot code
    pub fn to_dot(&self) -> String {
        format!("{:?}", Dot::with_config(&self.graph, &[]))
    }

    /// Like [`Self::replace_node`] but also updates the retained spec, so the
    /// manifest stays in sync after an `update_node` operation.
    pub fn replace_node_with_spec(
        &mut self,
        id: &str,
        new_node: Box<dyn DagNode>,
        kind: String,
        spec: serde_json::Value,
    ) -> Result<()> {
        self.replace_node(id, new_node)?;
        self.specs.insert(id.to_string(), (kind, spec));
        Ok(())
    }

    /// Export the current DAG topology + node specs as a serializable manifest
    /// for snapshot persistence. Nodes without a retained spec are omitted
    /// (they were added via the raw [`Self::add_node`] path, not through the
    /// registry).
    pub fn to_manifest(&self) -> super::history::DagManifest {
        let mut nodes = Vec::new();
        for (id, (kind, spec)) in &self.specs {
            nodes.push(super::history::NodeEntry {
                id: id.clone(),
                kind: kind.clone(),
                spec: spec.clone(),
            });
        }

        let mut edges = Vec::new();
        for edge in self.graph.edge_references() {
            let from = &self.graph[edge.source()];
            let to = &self.graph[edge.target()];
            let label = edge.weight();
            edges.push(super::history::EdgeEntry {
                from: from.clone(),
                from_port: label.from_port,
                to: to.clone(),
                to_port: label.to_port,
            });
        }

        super::history::DagManifest { nodes, edges }
    }

    /// Whether a spec has been retained for `id` (i.e. the node was added via
    /// [`Self::add_node_with_spec`] or [`Self::replace_node_with_spec`]).
    pub fn has_spec(&self, id: &str) -> bool {
        self.specs.contains_key(id)
    }

    /// Get the retained `(kind, spec)` for a node id.
    ///
    /// Returns `None` if the node does not exist or was added via the raw
    /// [`Self::add_node`] path (no retained spec). Returned values are cloned
    /// so the caller owns them without borrowing `self`.
    pub fn node_spec(&self, id: &str) -> Option<(String, serde_json::Value)> {
        self.specs.get(id).cloned()
    }
}

/// Check that every field required by `input` is present in `output` with a
/// compatible type.
///
/// Compatibility rule: the output schema must contain, by name, every field the
/// input schema declares, and the types must match exactly. (Stricter than
/// "subtype"; deliberately conservative — if a transform needs looser rules it
/// can leave the port schema `None`.)
fn schema_compatible(
    output: &arrow_schema::SchemaRef,
    input: &arrow_schema::SchemaRef,
) -> std::result::Result<(), String> {
    use std::collections::HashMap as StdHashMap;
    let out_fields: StdHashMap<&str, &arrow_schema::Field> = output
        .fields()
        .iter()
        .map(|f| (f.name().as_str(), f.as_ref()))
        .collect();
    for in_field in input.fields() {
        match out_fields.get(in_field.name().as_str()) {
            None => {
                return Err(format!(
                    "input requires column `{}` which is absent from output",
                    in_field.name()
                ));
            }
            Some(out_field) if out_field.data_type() != in_field.data_type() => {
                return Err(format!(
                    "column `{}` type mismatch: output {:?} vs input {:?}",
                    in_field.name(),
                    out_field.data_type(),
                    in_field.data_type()
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn cached_file_changed(file: &FileRef) -> bool {
    let Some(expected) = file.fingerprint.as_ref() else {
        return false;
    };
    // Remote artifacts are immutable and addressed by a unique object path;
    // their content hash is authoritative across workers and local mtimes.
    if file.path.starts_with("vfs://") && expected.content_hash.is_some() {
        return false;
    }
    FileFingerprint::from_path(&file.path).as_ref() != Some(expected)
}

#[cfg(test)]
mod tests {
    use crate::dag::{NodeInput, NodePorts};
    use crate::value::{FileRef, PortType};
    use std::assert_matches;

    use super::*;

    /// Minimal echo node for graph tests — passes through inputs unchanged.
    #[derive(Clone)]
    struct EchoNode {
        meta: NodePorts,
    }

    impl Default for EchoNode {
        fn default() -> Self {
            Self {
                meta: NodePorts::new()
                    .add_output_port(None)
                    .set_fixed_input(false),
            }
        }
    }

    impl EchoNode {
        fn from_ports(ports: NodePorts) -> Self {
            Self { meta: ports }
        }
    }

    #[async_trait::async_trait]
    impl DagNode for EchoNode {
        fn ports(&self) -> &NodePorts {
            &self.meta
        }

        async fn execute(
            &mut self,
            ctx: &crate::registry::NodeCtx,
            inputs: &[NodeInput],
            _reporter: &crate::dag::node_event::NodeReporter,
        ) -> std::result::Result<PortOutputs, DagError> {
            let mut outputs = PortOutputs::new();
            if inputs.is_empty() {
                // Source mode: a declared output port must publish a value,
                // otherwise downstream dispatch fails with
                // MissingUpstreamOutput. Emit a one-column placeholder.
                for port in self.meta.output_ports().iter() {
                    let batch = arrow_array::RecordBatch::try_from_iter([(
                        "value",
                        std::sync::Arc::new(arrow_array::Int64Array::from(Vec::<i64>::new()))
                            as std::sync::Arc<dyn arrow_array::Array>,
                    )])
                    .map_err(|e| DagError::Schedule(e.to_string()))?;
                    let df = ctx
                        .session()
                        .read_batch(batch)
                        .map_err(|e| DagError::Schedule(e.to_string()))?;
                    outputs.insert(port.index, crate::value::NodeValue::DataFrame(df));
                }
                return Ok(outputs);
            }
            for inp in inputs {
                outputs.insert(inp.port, inp.data.clone());
            }
            Ok(outputs)
        }

        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }

        fn kind(&self) -> &'static str {
            "echo"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn tui_snapshot_is_stable_and_ports_are_preserved() {
        let mut dag = DAG::default();
        dag.add_node_with_spec(
            "b".to_string(),
            Box::new(EchoNode::from_ports(
                NodePorts::new()
                    .set_fixed_input(false)
                    .add_input_port_of_type_with_label(None, PortType::DataFrame, "frame"),
            )),
            "echo".into(),
            serde_json::json!({}),
        )
        .unwrap();
        dag.add_node_with_spec(
            "a".to_string(),
            Box::new(EchoNode::from_ports(
                NodePorts::new().add_output_port_of_type(None, PortType::DataFrame),
            )),
            "echo".into(),
            serde_json::json!({}),
        )
        .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();

        let snapshot = dag.tui_snapshot();
        assert_eq!(
            snapshot
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(snapshot.nodes.iter().all(|node| node.kind == "echo"));
        assert!(snapshot.nodes.iter().all(|node| node.dirty));
        assert_eq!(snapshot.nodes[1].inputs[0].label.as_deref(), Some("frame"));
        assert_eq!(
            snapshot.nodes[0].outputs[0].data_type,
            PortType::DataFrame.to_string()
        );
        assert_eq!(
            snapshot
                .edges
                .iter()
                .map(|edge| (
                    edge.from.as_str(),
                    edge.from_port,
                    edge.to.as_str(),
                    edge.to_port
                ))
                .collect::<Vec<_>>(),
            vec![("a", 0, "b", 0)]
        );
        assert_eq!(
            snapshot.status_count(RuntimeStatus::Pending),
            snapshot.nodes.len()
        );
    }

    fn get_diamond_dag() -> DAG {
        let mut dag = DAG::default();
        for id in ["a", "b", "c", "d"] {
            add(&mut dag, id);
        }

        dag.add_edge("a", "b", 0, 0).unwrap();
        dag.add_edge("a", "c", 0, 0).unwrap();
        dag.add_edge("b", "d", 0, 0).unwrap();
        dag.add_edge("c", "d", 0, 0).unwrap();
        dag
    }

    fn add(dag: &mut DAG, id: &str) {
        dag.add_node(id.into(), Box::new(EchoNode::default()))
            .unwrap();
    }

    /// A minimal `NodeCtx` for graph tests that only exercise scheduling with
    /// ctx-ignoring nodes. The real engine ingredients are wired by
    /// `DataEngine`; here a bare `RuntimeEnv` is sufficient.
    fn test_ctx() -> crate::registry::NodeCtx {
        use datafusion::prelude::SessionContext;
        crate::registry::NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    #[test]
    fn topo_order_diamond() {
        let dag = get_diamond_dag();
        let order = dag.topo_order().unwrap();
        dbg!(&order);
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn cycle_rejected() {
        let mut dag = DAG::default();
        add(&mut dag, "x");
        add(&mut dag, "y");
        dag.add_edge("x", "y", 0, 0).unwrap();
        // Closing the cycle (y -> x) is rejected at add_edge time.
        let err = dag.add_edge("y", "x", 0, 0).unwrap_err();
        assert!(matches!(err, DagError::Cycle(_)), "{err:?}");
    }

    #[test]
    fn self_loop_rejected() {
        // A self-edge (x -> x) is a trivial cycle — rejected at add_edge.
        let mut dag = DAG::default();
        add(&mut dag, "x");
        let err = dag.add_edge("x", "x", 0, 0).unwrap_err();
        assert!(matches!(err, DagError::Cycle(_)), "{err:?}");
    }

    #[test]
    fn unknown_and_duplicate() {
        let mut dag = DAG::default();
        add(&mut dag, "a");
        // edge to missing node
        assert!(matches!(
            dag.add_edge("a", "ghost", 0, 0),
            Err(DagError::UnknownNode(_))
        ));
        // duplicate id
        assert!(matches!(
            dag.add_node("a".into(), Box::new(EchoNode::default())),
            Err(DagError::DuplicateNode(_))
        ));
    }

    #[test]
    fn predecessors_and_incoming() {
        let mut dag = DAG::default();
        for id in ["src", "a", "b"] {
            add(&mut dag, id);
        }
        dag.add_edge("src", "a", 0, 0).unwrap();
        dag.add_edge("src", "b", 0, 0).unwrap();

        assert_eq!(dag.predecessors("a").len(), 1);
        assert_eq!(dag.predecessors("a")[0], "src");
        let mut succ = dag.successors("src");
        succ.sort_unstable();
        assert_eq!(succ, vec!["a", "b"]);
        let inc = dag.incoming_edges("a");
        assert_eq!(inc.len(), 1);
    }

    #[test]
    fn default_edge_uses_default_ports() {
        let mut dag = DAG::default();
        for id in ["src", "a"] {
            add(&mut dag, id);
        }
        dag.add_edge("src", "a", 0, 0).unwrap();

        let edges = dag.incoming_edges_with_ports("a");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].0, "src");
        assert_eq!(edges[0].1.from_port, 0);
        assert_eq!(edges[0].1.to_port, 0);
    }

    #[test]
    fn explicit_edge_ports() {
        let mut dag = DAG::default();
        dag.add_node(
            "x".into(),
            Box::new(EchoNode::from_ports(
                NodePorts::new().add_output_port(None).add_output_port(None),
            )),
        )
        .unwrap();
        add(&mut dag, "y");
        dag.add_edge("x", "y", 1, 0).unwrap();

        let edges = dag.incoming_edges_with_ports("y");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].1.from_port, 1);
        assert_eq!(edges[0].1.to_port, 0);
    }

    #[test]
    fn diamond_edge_ports() {
        let dag = get_diamond_dag();
        // Diamond: a→b, a→c, b→d, c→d — all default ports.
        let edges_d = dag.incoming_edges_with_ports("d");
        assert_eq!(edges_d.len(), 2);
        let from_nodes: Vec<&str> = edges_d.iter().map(|(n, _)| n.as_str()).collect();
        assert!(from_nodes.contains(&"b"));
        assert!(from_nodes.contains(&"c"));
        for (_, e) in &edges_d {
            assert_eq!(e.from_port, 0);
            assert_eq!(e.to_port, 0);
        }
    }

    /// A no-op node with caller-supplied port topology (for schema/port tests).
    #[derive(Clone)]
    struct PortedNode(NodePorts);

    #[async_trait::async_trait]
    impl super::DagNode for PortedNode {
        fn ports(&self) -> &NodePorts {
            &self.0
        }
        fn clone_box(&self) -> Box<dyn super::DagNode> {
            Box::new((*self).clone())
        }
        fn kind(&self) -> &'static str {
            "ported"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            _ctx: &crate::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> std::result::Result<PortOutputs, super::DagError> {
            Ok(PortOutputs::new())
        }
    }

    fn make_schema(cols: &[(&str, arrow_schema::DataType)]) -> arrow_schema::SchemaRef {
        std::sync::Arc::new(arrow_schema::Schema::new(
            cols.iter()
                .map(|(n, t)| arrow_schema::Field::new(*n, t.clone(), true))
                .collect::<Vec<_>>(),
        ))
    }

    #[test]
    fn port_payload_type_mismatch_rejected() {
        let mut dag = DAG::default();
        dag.add_node(
            "file".into(),
            Box::new(PortedNode(
                NodePorts::new().add_output_port_of_type(None, PortType::File),
            )),
        )
        .unwrap();
        dag.add_node(
            "df".into(),
            Box::new(PortedNode(NodePorts::new().add_input_port(None))),
        )
        .unwrap();

        let err = dag.add_edge("file", "df", 0, 0).unwrap_err();
        assert_matches!(err, DagError::PortTypeMismatch { .. });
    }

    #[test]
    fn add_edge_rejects_incompatible_port_formats() {
        let mut dag = DAG::default();
        dag.add_node(
            "munge".into(),
            Box::new(PortedNode(
                NodePorts::new().add_output_port_of_type_with_label_and_format(
                    None,
                    PortType::File,
                    "log",
                    "ldsc_log",
                ),
            )),
        )
        .unwrap();
        dag.add_node(
            "h2".into(),
            Box::new(PortedNode(
                NodePorts::new().add_input_port_of_type_with_label_and_format(
                    None,
                    PortType::File,
                    "sumstats",
                    "sumstats_gz",
                ),
            )),
        )
        .unwrap();

        let err = dag.add_edge("munge", "h2", 0, 0).unwrap_err();
        assert_matches!(
            err,
            DagError::PortFormatMismatch {
                expected, actual, ..
            } if expected == "sumstats_gz" && actual == "ldsc_log"
        );
    }

    #[test]
    fn add_edge_allows_declared_alternate_input_formats() {
        let mut dag = DAG::default();
        dag.add_node(
            "reference".into(),
            Box::new(PortedNode(
                NodePorts::new().add_output_port_of_type_with_label_and_format(
                    None,
                    PortType::File,
                    "sumstats",
                    "sumstats_tsv",
                ),
            )),
        )
        .unwrap();
        dag.add_node(
            "h2".into(),
            Box::new(PortedNode(
                NodePorts::new().add_input_port_of_type_with_accepted_formats(
                    None,
                    PortType::File,
                    "sumstats",
                    "sumstats_gz",
                    ["sumstats_gz", "sumstats_tsv"],
                ),
            )),
        )
        .unwrap();

        dag.add_edge("reference", "h2", 0, 0).unwrap();
    }

    #[test]
    fn add_edge_rejects_unknown_output_port_immediately() {
        let mut dag = DAG::default();
        add(&mut dag, "x");
        add(&mut dag, "y");
        // EchoNode declares exactly one output port (0) — port 1 does not
        // exist and must be rejected at add_edge time, not silently stored.
        let err = dag.add_edge("x", "y", 1, 0).unwrap_err();
        assert_matches!(
            err,
            DagError::PortNotFound {
                node,
                port: 1,
                direction: "output",
            } if node == "x"
        );
    }

    #[test]
    fn add_edge_rejects_unknown_input_port_on_fixed_nodes() {
        let mut dag = DAG::default();
        add(&mut dag, "x");
        dag.add_node(
            "fixed".into(),
            Box::new(EchoNode::from_ports(
                NodePorts::new().add_input_port(None).add_output_port(None),
            )),
        )
        .unwrap();
        let err = dag.add_edge("x", "fixed", 0, 1).unwrap_err();
        assert_matches!(
            err,
            DagError::PortNotFound {
                node,
                port: 1,
                direction: "input",
            } if node == "fixed"
        );
    }

    #[test]
    fn add_edge_still_allows_undeclared_port_on_variadic_input() {
        // EchoNode::default has variadic input — wiring to an undeclared
        // port index remains legal (e.g. `sql` fan-in).
        let mut dag = DAG::default();
        add(&mut dag, "x");
        add(&mut dag, "y");
        dag.add_edge("x", "y", 0, 3).unwrap();
        let edges = dag.incoming_edges_with_ports("y");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].1.to_port, 3);
    }

    #[tokio::test]
    async fn missing_upstream_output_fails_loudly_at_dispatch() {
        // PortedNode declares one DataFrame output port but executes to an
        // empty PortOutputs — the edge validates yet delivers nothing.
        let mut dag = DAG::default();
        dag.add_node(
            "src".into(),
            Box::new(PortedNode(NodePorts::new().add_output_port(None))),
        )
        .unwrap();
        dag.add_node(
            "sink".into(),
            Box::new(PortedNode(
                NodePorts::new().add_input_port(None).add_output_port(None),
            )),
        )
        .unwrap();
        dag.add_edge("src", "sink", 0, 0).unwrap();

        let err = dag
            .run(&SchedulerConfig::default(), &test_ctx(), None)
            .await
            .unwrap_err();
        assert_matches!(
            err,
            DagError::MissingUpstreamOutput {
                from_node,
                from_port: 0,
                to_node,
                to_port: 0,
            } if from_node == "src" && to_node == "sink"
        );
    }

    #[derive(Clone)]
    struct MisdeclaredOutputNode {
        ports: NodePorts,
    }

    impl Default for MisdeclaredOutputNode {
        fn default() -> Self {
            Self {
                ports: NodePorts::new().add_output_port(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl DagNode for MisdeclaredOutputNode {
        fn ports(&self) -> &NodePorts {
            &self.ports
        }

        async fn execute(
            &mut self,
            _ctx: &crate::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> std::result::Result<PortOutputs, DagError> {
            let mut outputs = PortOutputs::new();
            outputs.insert_file(0, FileRef::new("/tmp/runtime-type-mismatch", None));
            Ok(outputs)
        }

        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }

        fn kind(&self) -> &'static str {
            "misdeclared_output"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn runtime_output_type_mismatch_fails_node() {
        let mut dag = DAG::default();
        dag.add_node("bad".into(), Box::new(MisdeclaredOutputNode::default()))
            .unwrap();

        let report = dag
            .run(&SchedulerConfig::default(), &test_ctx(), None)
            .await
            .unwrap();

        assert!(!report.ok);
        assert_eq!(dag.status("bad"), Some(RuntimeStatus::Failed));
        assert_matches!(
            report.errors.get("bad"),
            Some(DagError::PortTypeMismatch { .. })
        );
    }

    #[derive(Clone)]
    struct MultiFileOutputNode {
        paths: [std::path::PathBuf; 2],
        ports: NodePorts,
    }

    #[async_trait::async_trait]
    impl DagNode for MultiFileOutputNode {
        fn ports(&self) -> &NodePorts {
            &self.ports
        }

        async fn execute(
            &mut self,
            _ctx: &crate::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> std::result::Result<PortOutputs, DagError> {
            let mut outputs = PortOutputs::new();
            for (port, (path, format)) in [
                (&self.paths[0], "sumstats_gz"),
                (&self.paths[1], "ldsc_log"),
            ]
            .into_iter()
            .enumerate()
            {
                std::fs::write(path, format)
                    .map_err(|error| DagError::Schedule(error.to_string()))?;
                let file = FileRef::local(path, Some(format.into()))
                    .map_err(|error| DagError::Schedule(error.to_string()))?;
                outputs.insert_file(port as u8, file);
            }
            Ok(outputs)
        }

        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }

        fn kind(&self) -> &'static str {
            "multi_file_output"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn run_report_assigns_files_by_declared_output_port() {
        let temp = tempfile::tempdir().unwrap();
        let mut dag = DAG::default();
        dag.add_node(
            "source".into(),
            Box::new(MultiFileOutputNode {
                paths: [
                    temp.path().join("munged.sumstats.gz"),
                    temp.path().join("munge_sumstats.log"),
                ],
                ports: NodePorts::new()
                    .add_output_port_of_type_with_label_and_format(
                        None,
                        PortType::File,
                        "sumstats",
                        "sumstats_gz",
                    )
                    .add_output_port_of_type_with_label_and_format(
                        None,
                        PortType::File,
                        "log",
                        "ldsc_log",
                    ),
            }),
        )
        .unwrap();

        let report = dag
            .run(&SchedulerConfig::default(), &test_ctx(), None)
            .await
            .unwrap();

        let node = report
            .nodes
            .iter()
            .find(|node| node.id == "source")
            .unwrap();
        assert_eq!(node.port_assignments.len(), 2);
        assert!(
            node.port_assignments[&0]
                .path
                .ends_with("munged.sumstats.gz")
        );
        assert_eq!(
            node.port_assignments[&0].format.as_deref(),
            Some("sumstats_gz")
        );
        assert!(
            node.port_assignments[&1]
                .path
                .ends_with("munge_sumstats.log")
        );
        assert_eq!(
            node.port_assignments[&1].format.as_deref(),
            Some("ldsc_log")
        );
    }

    #[derive(Clone)]
    struct FileOutputNode {
        path: std::path::PathBuf,
        runs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        ports: NodePorts,
    }

    impl FileOutputNode {
        fn new(path: std::path::PathBuf) -> Self {
            Self {
                path,
                runs: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
            }
        }
    }

    #[async_trait::async_trait]
    impl DagNode for FileOutputNode {
        fn ports(&self) -> &NodePorts {
            &self.ports
        }

        async fn execute(
            &mut self,
            _ctx: &crate::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> std::result::Result<PortOutputs, DagError> {
            let run = self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            std::fs::write(&self.path, format!("run {run}"))
                .map_err(|e| DagError::Schedule(e.to_string()))?;
            let file =
                FileRef::local(&self.path, None).map_err(|e| DagError::Schedule(e.to_string()))?;
            let mut outputs = PortOutputs::new();
            outputs.insert_file(0, file);
            Ok(outputs)
        }

        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }

        fn kind(&self) -> &'static str {
            "file_output"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn incremental_run_detects_changed_file_output() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dag-core-file-output-{}-{nanos}.txt",
            std::process::id()
        ));
        let node = FileOutputNode::new(path.clone());
        let runs = node.runs.clone();
        let mut dag = DAG::default();
        dag.add_node("writer".into(), Box::new(node)).unwrap();

        let mut cfg = SchedulerConfig::default();
        cfg.incremental = true;
        dag.run(&cfg, &test_ctx(), None).await.unwrap();
        dag.run(&cfg, &test_ctx(), None).await.unwrap();
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);

        std::fs::write(&path, "externally changed").unwrap();
        dag.run(&cfg, &test_ctx(), None).await.unwrap();
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn schema_compatible_passes() {
        // Output schema is a superset of input schema → OK.
        let out = make_schema(&[
            ("a", arrow_schema::DataType::Int32),
            ("b", arrow_schema::DataType::Utf8),
        ]);
        let inp = make_schema(&[("a", arrow_schema::DataType::Int32)]);
        assert!(schema_compatible(&out, &inp).is_ok());
    }

    #[test]
    fn schema_mismatch_rejected() {
        // Input requires a column the output lacks, and a type differs.
        let out = make_schema(&[("a", arrow_schema::DataType::Int32)]);
        let inp = make_schema(&[
            ("a", arrow_schema::DataType::Int64),
            ("b", arrow_schema::DataType::Utf8),
        ]);
        let err = schema_compatible(&out, &inp).unwrap_err();
        assert!(err.contains("a"), "{err}");
    }

    #[test]
    fn validate_schema_mismatch_between_ports() {
        let mut dag = DAG::default();
        let out_schema = make_schema(&[("a", arrow_schema::DataType::Int32)]);
        let in_schema = make_schema(&[("a", arrow_schema::DataType::Int64)]);
        dag.add_node(
            "src".into(),
            Box::new(PortedNode(
                NodePorts::new().add_output_port(Some(out_schema)),
            )),
        )
        .unwrap();
        dag.add_node(
            "dst".into(),
            Box::new(PortedNode(NodePorts::new().add_input_port(Some(in_schema)))),
        )
        .unwrap();
        // Schema mismatch is caught at `add_edge` time (via validate_edge_schema).
        let err = dag.add_edge("src", "dst", 0, 0).unwrap_err();
        assert!(
            matches!(err, DagError::SchemaMismatch { ref from_node, ref to_port, .. } if from_node == "src" && *to_port == 0),
            "expected SchemaMismatch, got {err:?}"
        );
    }

    #[test]
    fn render_into_dot() {
        let dag = get_diamond_dag();
        let dot = dag.to_dot();

        // DOT output must be a digraph declaration
        assert!(
            dot.contains("digraph"),
            "to_dot output should be a digraph declaration"
        );

        // All 4 diamond nodes must appear as labels in the output
        for node_id in ["a", "b", "c", "d"] {
            assert!(
                dot.contains(node_id),
                "DOT output should contain node '{node_id}'"
            );
        }

        // 4 edges: a→b, a→c, b→d, c→d — petgraph uses "N -> M" notation
        assert!(
            dot.contains("->"),
            "DOT output should contain directed edges"
        );

        // Smoke-check: non-trivial output (a 4-node DAG should be > 20 chars)
        assert!(dot.len() > 20, "DOT output seems too short, got: {dot}");
    }

    #[test]
    fn one_to_multi_wiring_accept() {
        let mut dag = DAG::default();
        let node_a_ports = NodePorts::new().add_output_port(None);
        let node_b_ports = NodePorts::new().add_input_port(None);
        let node_c_ports = NodePorts::new().add_input_port(None);

        let node_a = PortedNode(node_a_ports);
        let node_b = PortedNode(node_b_ports);
        let node_c = PortedNode(node_c_ports);

        dag.add_node("node_a_id".into(), Box::new(node_a)).unwrap();
        dag.add_node("node_b_id".into(), Box::new(node_b)).unwrap();
        dag.add_node("node_c_id".into(), Box::new(node_c)).unwrap();

        // Fan-out: node_a's single output 0 feeds both node_b and node_c.
        // Each input port must end up with exactly one incoming edge.
        dag.add_edge("node_a_id", "node_b_id", 0, 0).unwrap();
        dag.add_edge("node_a_id", "node_c_id", 0, 0).unwrap();

        dag.validate_port_wiring().unwrap();
    }

    #[test]
    fn variadic_node_allows_undeclared_optional_input_port() {
        // Declared ports stay required even when the input is variadic; the
        // undeclared extra port may be unwired, wired once, and is still
        // subject to the strict 1:1 rule once connected.
        let mut dag = DAG::default();
        for name in ["s0", "s1", "s2", "s3", "s4"] {
            dag.add_node(
                name.into(),
                Box::new(PortedNode(NodePorts::new().add_output_port(None))),
            )
            .unwrap();
        }
        let target_ports = NodePorts::new()
            .add_input_port(None)
            .add_input_port(None)
            .add_input_port(None)
            .set_fixed_input(false);
        dag.add_node("t".into(), Box::new(PortedNode(target_ports)))
            .unwrap();

        for i in 0u8..3 {
            dag.add_edge(format!("s{i}"), "t", 0, i).unwrap();
        }
        dag.validate_port_wiring().unwrap();

        dag.add_edge("s3", "t", 0, 3).unwrap();
        dag.validate_port_wiring().unwrap();

        dag.add_edge("s4", "t", 0, 3).unwrap();
        let err = dag.validate_port_wiring().unwrap_err();
        assert_matches!(
            err,
            DagError::PortOverconnected { node, port } if node == "t" && port == 3
        );
    }

    #[test]
    fn multi_to_one_wiring_reject() {
        let mut dag = DAG::default();
        let node_a_ports = NodePorts::new().add_output_port(None);
        let node_b_ports = NodePorts::new().add_input_port(None);
        let node_c_ports = NodePorts::new().add_output_port(None);

        dag.add_node("node_a_id".into(), Box::new(PortedNode(node_a_ports)))
            .unwrap();
        dag.add_node("node_b_id".into(), Box::new(PortedNode(node_b_ports)))
            .unwrap();
        dag.add_node("node_c_id".into(), Box::new(PortedNode(node_c_ports)))
            .unwrap();

        // Two edges to the same declared input port 0 — rejected at add_edge.
        dag.add_edge("node_a_id", "node_b_id", 0, 0).unwrap();
        let err = dag.add_edge("node_c_id", "node_b_id", 0, 0).unwrap_err();
        assert_matches!(
            err,
            DagError::PortOverconnected { node, port }
                if node == "node_b_id" && port == 0
        );
    }

    #[test]
    fn add_edge_rejects_overconnected_port() {
        // `add_edge` enforces the 1:1 rule on declared input ports at insertion
        // time. Two nodes (a, c) both trying to connect to b's declared input
        // port 0 — the second `add_edge` must reject immediately.
        let mut dag = DAG::default();
        let node_a_ports = NodePorts::new().add_output_port(None);
        let node_b_ports = NodePorts::new().add_input_port(None);
        let node_c_ports = NodePorts::new().add_output_port(None);

        dag.add_node("node_a_id".into(), Box::new(PortedNode(node_a_ports)))
            .unwrap();
        dag.add_node("node_b_id".into(), Box::new(PortedNode(node_b_ports)))
            .unwrap();
        dag.add_node("node_c_id".into(), Box::new(PortedNode(node_c_ports)))
            .unwrap();

        // First edge to b's declared input port 0 — OK.
        dag.add_edge("node_a_id", "node_b_id", 0, 0).unwrap();

        // Second edge to the same declared input port 0 — must reject at add_edge.
        let err = dag.add_edge("node_c_id", "node_b_id", 0, 0).unwrap_err();
        assert_matches!(
            err,
            DagError::PortOverconnected { node, port }
                if node == "node_b_id" && port == 0
        );
    }

    #[test]
    fn add_edge_allows_undeclared_port_fan_in() {
        // When the target node has NO declared input ports (empty Ports), `add_edge`
        // does NOT enforce 1:1 — multiple edges to the same port index are allowed.
        // This is what happens in diamond DAGs with EchoNode (default meta has
        // empty Ports).
        let mut dag = DAG::default();
        for id in ["a", "b", "c", "d"] {
            add(&mut dag, id);
        }

        // d has no declared ports, so both edges to port 0 are accepted.
        dag.add_edge("b", "d", 0, 0).unwrap();
        dag.add_edge("c", "d", 0, 0).unwrap();

        // The diamond edges are all present.
        assert_eq!(dag.incoming_edges_with_ports("d").len(), 2);
    }

    #[test]
    fn replace_node_preserves_edges() {
        let mut dag = get_diamond_dag(); // a→b, a→c, b→d, c→d
        // Replace node "b" with a new EchoNode (same port topology).
        let new_b = Box::new(EchoNode::default());
        dag.replace_node("b", new_b).unwrap();

        // All four nodes still present.
        assert_eq!(dag.node_ids().len(), 4);
        // Edge a→b preserved.
        assert_eq!(dag.predecessors("b"), vec!["a"]);
        assert_eq!(dag.successors("b"), vec!["d"]);
        // Edge b→d preserved.
        assert_eq!(dag.predecessors("d").len(), 2);
    }

    #[test]
    fn replace_node_clears_stale_outputs() {
        let mut dag = DAG::default();
        add(&mut dag, "x");
        // Insert a fake output.
        dag.outputs.insert("x".to_string(), PortOutputs::new());
        assert!(dag.output("x").is_some());

        dag.replace_node("x", Box::new(EchoNode::default()))
            .unwrap();
        // Output must be cleared after replacement.
        assert!(dag.output("x").is_none());
    }

    #[test]
    fn replace_node_unknown_id_rejected() {
        let mut dag = DAG::default();
        let err = dag
            .replace_node("ghost", Box::new(EchoNode::default()))
            .unwrap_err();
        assert!(matches!(err, DagError::UnknownNode(_)));
    }

    #[test]
    fn replace_node_rejects_incompatible_port() {
        let mut dag = DAG::default();
        let out_schema = make_schema(&[("a", arrow_schema::DataType::Int32)]);
        let in_schema = make_schema(&[("a", arrow_schema::DataType::Int32)]);

        dag.add_node(
            "src".into(),
            Box::new(PortedNode(
                NodePorts::new().add_output_port(Some(out_schema)),
            )),
        )
        .unwrap();
        dag.add_node(
            "dst".into(),
            Box::new(PortedNode(
                NodePorts::new().add_input_port(Some(in_schema.clone())),
            )),
        )
        .unwrap();
        dag.add_edge("src", "dst", 0, 0).unwrap();

        // Replace "dst" with a node that has NO declared input ports → the
        // existing edge references port 0 which no longer exists.
        let no_ports = Box::new(PortedNode(NodePorts::new()));
        let err = dag.replace_node("dst", no_ports).unwrap_err();
        assert!(
            matches!(err, DagError::PortNotFound { ref node, port, .. } if node == "dst" && port == 0),
            "expected PortNotFound for dst:0, got {err:?}"
        );
    }

    // ── delete_edge tests ──────────────────────────────────────────────

    #[test]
    fn delete_edge_removes_matching_edge() {
        let mut dag = get_diamond_dag();
        // Diamond: a→b(0,0), a→c(0,0), b→d(0,0), c→d(0,0)
        assert_eq!(dag.incoming_edges_with_ports("d").len(), 2);

        dag.delete_edge("b", "d", 0, 0).unwrap();

        let edges_d = dag.incoming_edges_with_ports("d");
        assert_eq!(edges_d.len(), 1);
        assert_eq!(edges_d[0].0, "c");
        // a→b, a→c still intact
        assert_eq!(dag.incoming_edges_with_ports("b").len(), 1);
        assert_eq!(dag.incoming_edges_with_ports("c").len(), 1);
    }

    #[test]
    fn delete_edge_wrong_port_rejected() {
        let mut dag = DAG::default();
        add(&mut dag, "x");
        add(&mut dag, "y");
        dag.add_edge("x", "y", 0, 0).unwrap();

        let err = dag.delete_edge("x", "y", 0, 1).unwrap_err();
        assert!(
            matches!(err, DagError::EdgeNotFound { .. }),
            "expected EdgeNotFound, got {err:?}"
        );
        // Original edge still intact
        assert_eq!(dag.incoming_edges_with_ports("y").len(), 1);
    }

    #[test]
    fn delete_edge_nonexistent_edge_rejected() {
        let mut dag = DAG::default();
        add(&mut dag, "x");
        add(&mut dag, "y");

        let err = dag.delete_edge("x", "y", 0, 0).unwrap_err();
        assert!(
            matches!(err, DagError::EdgeNotFound { .. }),
            "expected EdgeNotFound, got {err:?}"
        );
    }

    #[test]
    fn delete_edge_unknown_node_rejected() {
        let mut dag = DAG::default();
        add(&mut dag, "x");

        let err = dag.delete_edge("x", "ghost", 0, 0).unwrap_err();
        assert!(matches!(err, DagError::UnknownNode(_)), "{err:?}");
    }

    #[test]
    fn delete_edge_after_delete_node() {
        let mut dag = DAG::default();
        for id in ["a", "b", "c"] {
            add(&mut dag, id);
        }
        dag.add_edge("a", "b", 0, 0).unwrap();
        dag.add_edge("a", "c", 0, 0).unwrap();
        dag.delete_edge("a", "b", 0, 0).unwrap();
        assert_eq!(dag.incoming_edges_with_ports("c").len(), 1);

        // Now b has no incoming edges — safe to delete.
        dag.delete_node("b").unwrap();

        dbg!(dag.incoming_edges_with_ports("c"));
        // c is still connected to a.
        assert_eq!(dag.incoming_edges_with_ports("c").len(), 1);
    }

    #[tokio::test]
    async fn multi_input_tmp_df_register() {
        let mut dag = DAG::default();
        dag.add_node(
            "a".into(),
            Box::new(EchoNode::from_ports(NodePorts::new().add_output_port(None))),
        )
        .unwrap();
        dag.add_node(
            "b".into(),
            Box::new(EchoNode::from_ports(NodePorts::new().add_output_port(None))),
        )
        .unwrap();
        dag.add_node(
            "c".into(),
            Box::new(EchoNode::from_ports(
                NodePorts::new()
                    .add_input_port(None)
                    .add_input_port(None)
                    .set_fixed_input(true),
            )),
        )
        .unwrap();
        dag.add_edge("a", "c", 0, 0).unwrap();
        dag.add_edge("b", "c", 0, 1).unwrap();
        dag.validate().unwrap();
        assert_eq!(dag.node_ids().len(), 3);
        assert_eq!(dag.successors("a").len(), 1);
        assert_eq!(dag.predecessors("c").len(), 2);

        dag.run(&SchedulerConfig::default(), &test_ctx(), None)
            .await
            .unwrap();
        let output = dag.output("c").unwrap();
        dbg!(output);
    }

    /// When `run` is given an external event sink, it must forward a
    /// `Finished { status, elapsed_ms }` observation for every node that
    /// completes — without DataFrame payloads — so an observer (the run_dag
    /// tool) can report per-node timing while the run is in flight.
    #[tokio::test]
    async fn run_forwards_finished_events_to_sink() {
        let mut dag = DAG::default();
        // a (source) -> b (echo), two nodes.
        dag.add_node(
            "a".into(),
            Box::new(EchoNode::from_ports(NodePorts::new().add_output_port(None))),
        )
        .unwrap();
        dag.add_node(
            "b".into(),
            Box::new(EchoNode::from_ports(
                NodePorts::new().add_input_port(None).add_output_port(None),
            )),
        )
        .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();
        dag.validate().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<NodeEvent>(64);
        let report = dag
            .run(&SchedulerConfig::default(), &test_ctx(), Some(tx))
            .await
            .unwrap();
        assert!(report.ok, "diamond run should succeed");

        // Drain all forwarded events.
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }

        let finished: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.kind, NodeEventKind::Finished { .. }))
            .collect();
        assert_eq!(
            finished.len(),
            2,
            "expected one Finished event per node (a, b); got: {events:?}"
        );
        for ev in &finished {
            let NodeEventKind::Finished { status, elapsed_ms } = &ev.kind else {
                unreachable!()
            };
            assert_eq!(
                *status,
                RuntimeStatus::Success,
                "node should finish success"
            );
            // elapsed_ms may legitimately be 0 on trivially fast nodes; just
            // assert the field is present and finite (it is, by type).
            let _ = elapsed_ms;
            assert!(
                ev.node_id == "a" || ev.node_id == "b",
                "Finished event should name a real node; got {}",
                ev.node_id
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn memory_guard_cancels_run_and_reports_trigger() {
        let mut dag = DAG::default();
        dag.add_node(
            "sleep".into(),
            Box::new(EchoNode::from_ports(NodePorts::new().add_output_port(None))),
        )
        .unwrap();

        let cfg = SchedulerConfig {
            memory_guard: MemoryGuardConfig::new(
                f64::MIN_POSITIVE,
                std::time::Duration::from_millis(1),
            ),
            ..SchedulerConfig::default()
        };
        let report = dag.run(&cfg, &test_ctx(), None).await.unwrap();

        assert!(!report.ok, "memory-triggered run must not report success");
        assert_eq!(report.status("sleep"), Some(RuntimeStatus::Cancelled));
        let memory = report.resource.memory;
        assert!(memory.enabled);
        assert!(memory.trigger.is_some(), "trigger sample must be reported");
        assert_eq!(memory.sample_count, 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("memory guard")),
            "warnings: {:?}",
            report.warnings
        );
    }

    /// Regression for the cross-run `SessionContext` leak.
    ///
    /// Before the framework owned ctx lifecycle, nodes that stored a
    /// `SessionContext` field (the mixers) polluted their catalog across
    /// runs: the second `run` hit "table already exists" because
    /// `register_table` had left entries behind from the first run. The fix
    /// injects a fresh `&NodeCtx` per execution and every node builds an
    /// ephemeral `SessionContext` via `NodeCtx::session()`, so re-running the
    /// same DAG must always succeed. This pins that property at the scheduler
    /// level.
    #[tokio::test]
    async fn dag_can_be_rerun_without_state_leak() {
        let mut dag = DAG::default();
        // a (source) -> b (echo).
        dag.add_node(
            "a".into(),
            Box::new(EchoNode::from_ports(NodePorts::new().add_output_port(None))),
        )
        .unwrap();
        dag.add_node(
            "b".into(),
            Box::new(EchoNode::from_ports(
                NodePorts::new().add_input_port(None).add_output_port(None),
            )),
        )
        .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();
        dag.validate().unwrap();

        let ctx = test_ctx();

        // First run.
        let r1 = dag
            .run(&SchedulerConfig::default(), &ctx, None)
            .await
            .unwrap();
        assert!(r1.ok, "first run should succeed");
        assert_eq!(dag.status("b"), Some(RuntimeStatus::Success));

        // Second run on the SAME DAG instance — must not see leftover state.
        let r2 = dag
            .run(&SchedulerConfig::default(), &ctx, None)
            .await
            .unwrap();
        assert!(r2.ok, "re-run should succeed (no cross-run ctx leak)");
        assert_eq!(dag.status("b"), Some(RuntimeStatus::Success));
    }

    // ── incremental execution tests ────────────────────────────────────

    /// An EchoNode that counts how many times `execute` was called.
    /// Shared counter lets tests assert exactly which nodes were skipped.
    #[derive(Clone)]
    struct CountingEcho {
        meta: NodePorts,
        counter: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl DagNode for CountingEcho {
        fn ports(&self) -> &NodePorts {
            &self.meta
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }
        fn kind(&self) -> &'static str {
            "counting_echo"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            _ctx: &crate::registry::NodeCtx,
            inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> std::result::Result<PortOutputs, DagError> {
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut out: PortOutputs = PortOutputs::new();
            for inp in inputs {
                out.insert(inp.port, inp.data.clone());
            }
            Ok(out)
        }
    }

    impl CountingEcho {
        fn new(counter: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                meta: NodePorts::new()
                    .add_output_port(None)
                    .set_fixed_input(false),
                counter,
            }
        }
    }

    /// Shorthand to read an atomic counter's current value.
    fn cnt(ctr: &Arc<std::sync::atomic::AtomicUsize>) -> usize {
        ctr.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn incremental_cfg() -> SchedulerConfig {
        SchedulerConfig {
            incremental: true,
            ..SchedulerConfig::default()
        }
    }

    /// First run in incremental mode should execute all nodes (all are dirty
    /// because they were just added).
    #[tokio::test]
    async fn incremental_first_run_executes_all() {
        let mut dag = DAG::default();
        let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
            .unwrap();
        dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
            .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();
        dag.validate().unwrap();

        let ctx = test_ctx();
        let report = dag.run(&incremental_cfg(), &ctx, None).await.unwrap();
        assert!(report.ok);
        assert_eq!(cnt(&ctr_a), 1, "node a should execute once");
        assert_eq!(cnt(&ctr_b), 1, "node b should execute once");
        // After successful run, both should be clean.
        assert!(!dag.is_dirty("a"));
        assert!(!dag.is_dirty("b"));
    }

    /// A second incremental run with no changes should skip ALL nodes and reuse
    /// cached outputs.
    #[tokio::test]
    async fn incremental_rerun_no_changes_skips_all() {
        let mut dag = DAG::default();
        let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
            .unwrap();
        dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
            .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();

        let ctx = test_ctx();
        let cfg = incremental_cfg();

        // First run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 1);
        assert_eq!(cnt(&ctr_b), 1);

        // Second run — nothing changed, all clean.
        let r2 = dag.run(&cfg, &ctx, None).await.unwrap();
        assert!(r2.ok);
        assert_eq!(cnt(&ctr_a), 1, "node a should NOT re-execute");
        assert_eq!(cnt(&ctr_b), 1, "node b should NOT re-execute");
        assert_eq!(dag.status("a"), Some(RuntimeStatus::Success));
        assert_eq!(dag.status("b"), Some(RuntimeStatus::Success));
        // Output still cached.
        assert!(dag.output("a").is_some());
        assert!(dag.output("b").is_some());
    }

    /// After `replace_node` on `a`, only `a` and its descendant `b` should
    /// re-execute; `c` (an independent branch) should be skipped.
    #[tokio::test]
    async fn incremental_replace_reexecutes_only_descendants() {
        let mut dag = DAG::default();
        let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_c = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
            .unwrap();
        dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
            .unwrap();
        dag.add_node("c".into(), Box::new(CountingEcho::new(ctr_c.clone())))
            .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();
        // c is independent (no edge from a).

        let ctx = test_ctx();
        let cfg = incremental_cfg();

        // First run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 1);
        assert_eq!(cnt(&ctr_b), 1);
        assert_eq!(cnt(&ctr_c), 1);

        // Replace node "a" with a fresh CountingEcho (new counter).
        let ctr_a2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        dag.replace_node("a", Box::new(CountingEcho::new(ctr_a2.clone())))
            .unwrap();

        // "a" and "b" should be dirty; "c" should be clean.
        assert!(dag.is_dirty("a"), "a should be dirty after replace");
        assert!(dag.is_dirty("b"), "b (descendant) should be dirty");
        assert!(!dag.is_dirty("c"), "c (independent) should be clean");

        // Second run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a2), 1, "replaced a should execute once");
        assert_eq!(
            cnt(&ctr_b),
            2,
            "b should re-execute (descendant of replaced a)"
        );
        assert_eq!(
            cnt(&ctr_c),
            1,
            "c should NOT re-execute (independent branch)"
        );
    }

    /// After `add_edge`, the target node and its descendants should be dirty.
    #[tokio::test]
    async fn incremental_add_edge_marks_target_dirty() {
        let mut dag = DAG::default();
        let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
            .unwrap();
        dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
            .unwrap();

        let ctx = test_ctx();
        let cfg = incremental_cfg();

        // First run — two independent nodes.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 1);
        assert_eq!(cnt(&ctr_b), 1);

        // Connect a → b.
        dag.add_edge("a", "b", 0, 0).unwrap();
        assert!(dag.is_dirty("b"), "b should be dirty after add_edge");
        assert!(!dag.is_dirty("a"), "a should remain clean");

        // Second run — only b should re-execute.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 1, "a should NOT re-execute");
        assert_eq!(cnt(&ctr_b), 2, "b should re-execute");
    }

    /// After `delete_edge`, the target node and its descendants should be dirty.
    #[tokio::test]
    async fn incremental_delete_edge_marks_target_dirty() {
        let mut dag = DAG::default();
        let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_c = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
            .unwrap();
        dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
            .unwrap();
        dag.add_node("c".into(), Box::new(CountingEcho::new(ctr_c.clone())))
            .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();
        dag.add_edge("b", "c", 0, 0).unwrap();

        let ctx = test_ctx();
        let cfg = incremental_cfg();

        // First run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 1);
        assert_eq!(cnt(&ctr_b), 1);
        assert_eq!(cnt(&ctr_c), 1);

        // Delete edge a → b.
        dag.delete_edge("a", "b", 0, 0).unwrap();
        assert!(dag.is_dirty("b"), "b should be dirty after delete_edge");
        assert!(dag.is_dirty("c"), "c (descendant) should be dirty");
        assert!(!dag.is_dirty("a"), "a should remain clean");

        // Second run — only b and c should re-execute.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 1, "a should NOT re-execute");
        assert_eq!(cnt(&ctr_b), 2, "b should re-execute");
        assert_eq!(cnt(&ctr_c), 2, "c should re-execute");
    }

    /// Manual `mark_dirty` propagates to all transitive descendants.
    #[tokio::test]
    async fn incremental_manual_mark_dirty_propagates() {
        let mut dag = DAG::default();
        // a → b → c → d (linear chain)
        let ctrs: Vec<Arc<std::sync::atomic::AtomicUsize>> = (0..4)
            .map(|_| Arc::new(std::sync::atomic::AtomicUsize::new(0)))
            .collect();
        for (i, id) in ["a", "b", "c", "d"].iter().enumerate() {
            dag.add_node((*id).into(), Box::new(CountingEcho::new(ctrs[i].clone())))
                .unwrap();
        }
        dag.add_edge("a", "b", 0, 0).unwrap();
        dag.add_edge("b", "c", 0, 0).unwrap();
        dag.add_edge("c", "d", 0, 0).unwrap();

        let ctx = test_ctx();
        let cfg = incremental_cfg();

        // First run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        for ctr in &ctrs {
            assert_eq!(ctr.load(std::sync::atomic::Ordering::SeqCst), 1);
        }

        // Manually mark "b" dirty — should propagate to c and d, not a.
        dag.mark_dirty("b");
        assert!(!dag.is_dirty("a"));
        assert!(dag.is_dirty("b"));
        assert!(dag.is_dirty("c"));
        assert!(dag.is_dirty("d"));

        // Second run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(
            ctrs[0].load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a should NOT re-execute"
        );
        assert_eq!(
            ctrs[1].load(std::sync::atomic::Ordering::SeqCst),
            2,
            "b should re-execute"
        );
        assert_eq!(
            ctrs[2].load(std::sync::atomic::Ordering::SeqCst),
            2,
            "c should re-execute"
        );
        assert_eq!(
            ctrs[3].load(std::sync::atomic::Ordering::SeqCst),
            2,
            "d should re-execute"
        );
    }

    /// `mark_all_dirty` forces a full re-run even in incremental mode.
    #[tokio::test]
    async fn incremental_mark_all_forces_full_rerun() {
        let mut dag = DAG::default();
        let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
            .unwrap();
        dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
            .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();

        let ctx = test_ctx();
        let cfg = incremental_cfg();

        // First run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 1);
        assert_eq!(cnt(&ctr_b), 1);

        // Mark all dirty.
        dag.mark_all_dirty();
        assert!(dag.is_dirty("a"));
        assert!(dag.is_dirty("b"));

        // Second run — everything re-executes.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 2);
        assert_eq!(cnt(&ctr_b), 2);
    }

    /// Incremental mode with a diamond DAG: marking one branch dirty re-runs
    /// only that branch + the merge node, not the other branch.
    #[tokio::test]
    async fn incremental_diamond_partial_rerun() {
        let mut dag = DAG::default();
        // Diamond: a → b → d, a → c → d
        let ctrs: Vec<Arc<std::sync::atomic::AtomicUsize>> = (0..4)
            .map(|_| Arc::new(std::sync::atomic::AtomicUsize::new(0)))
            .collect();
        for (i, id) in ["a", "b", "c", "d"].iter().enumerate() {
            dag.add_node((*id).into(), Box::new(CountingEcho::new(ctrs[i].clone())))
                .unwrap();
        }
        dag.add_edge("a", "b", 0, 0).unwrap();
        dag.add_edge("a", "c", 0, 0).unwrap();
        // Use distinct input ports on "d" (0 and 1) — strict 1:1 validation
        // rejects two edges to the same port even on variadic nodes.
        dag.add_edge("b", "d", 0, 0).unwrap();
        dag.add_edge("c", "d", 0, 1).unwrap();

        let ctx = test_ctx();
        let cfg = incremental_cfg();

        // First run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        for ctr in &ctrs {
            assert_eq!(ctr.load(std::sync::atomic::Ordering::SeqCst), 1);
        }

        // Replace "b" — should mark b + d dirty (not a, not c).
        let ctr_b2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        dag.replace_node("b", Box::new(CountingEcho::new(ctr_b2.clone())))
            .unwrap();

        assert!(!dag.is_dirty("a"));
        assert!(dag.is_dirty("b"));
        assert!(!dag.is_dirty("c"));
        assert!(dag.is_dirty("d"));

        // Second run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(
            ctrs[0].load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a should NOT re-execute"
        );
        assert_eq!(cnt(&ctr_b2), 1, "replaced b should execute once");
        assert_eq!(
            ctrs[2].load(std::sync::atomic::Ordering::SeqCst),
            1,
            "c should NOT re-execute"
        );
        assert_eq!(
            ctrs[3].load(std::sync::atomic::Ordering::SeqCst),
            2,
            "d should re-execute (merge of dirty b + clean c)"
        );
    }

    /// Non-incremental mode ignores dirty marks and always re-runs everything.
    #[tokio::test]
    async fn non_incremental_ignores_dirty_marks() {
        let mut dag = DAG::default();
        let ctr_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ctr_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        dag.add_node("a".into(), Box::new(CountingEcho::new(ctr_a.clone())))
            .unwrap();
        dag.add_node("b".into(), Box::new(CountingEcho::new(ctr_b.clone())))
            .unwrap();
        dag.add_edge("a", "b", 0, 0).unwrap();

        let ctx = test_ctx();
        let cfg = SchedulerConfig::default(); // incremental = false

        // First run.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 1);
        assert_eq!(cnt(&ctr_b), 1);

        // Second run in non-incremental mode — everything re-executes.
        dag.run(&cfg, &ctx, None).await.unwrap();
        assert_eq!(cnt(&ctr_a), 2, "a should re-execute (non-incremental)");
        assert_eq!(cnt(&ctr_b), 2, "b should re-execute (non-incremental)");
    }
}
