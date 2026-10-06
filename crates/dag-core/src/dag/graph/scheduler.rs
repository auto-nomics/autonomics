//! The async readiness scheduler: dispatch nodes as their predecessors
//! complete, stream channel items, expand dynamic fanout, honour the memory
//! guard and cancellation, and assemble the run report.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use datafusion::common::HashMap;
use futures::FutureExt;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info_span, warn};

use super::memory::{AbortOnDropHandle, MemoryGuardState, run_memory_monitor};
use super::streaming::StreamRuntimeState;
use super::{DAG, EdgeLabel, Result};
use crate::dag::NodeId;
use crate::dag::error::DagError;
use crate::dag::logical::LogicalExecutionStrategy;
use crate::dag::node_event::{JobResult, NodeEvent, NodeEventKind, NodeReporter};
use crate::dag::runtime::{
    InputHashing, ResourceRunReport, RunReport, RuntimeStatus, SchedulerConfig,
};
use crate::dag::utils::{build_input_bindings, build_inputs, cascade_skip};
use crate::dag::{
    NodeInput, TaskExecution, TaskInputBinding, TaskInputSource, TaskOutputBinding, TaskSpec,
    TaskSubmission,
};
use crate::resource::{MemoryObservation, MemorySample, sample_memory_usage};

impl DAG {
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

    fn input_name(&self, id: &str, port: u8) -> String {
        self.nodes
            .get(id)
            .and_then(|node| node.ports().input_port(port))
            .and_then(|port| port.label.clone())
            .unwrap_or_else(|| format!("port_{port}"))
    }

    fn output_name(&self, id: &str, port: u8) -> String {
        self.nodes
            .get(id)
            .and_then(|node| node.ports().output_port(port))
            .and_then(|port| port.label.clone())
            .unwrap_or_else(|| format!("port_{port}"))
    }

    fn build_task_submission(
        &self,
        id: &str,
        kind: &str,
        spec: serde_json::Value,
        inputs: &[NodeInput],
        incoming: &HashMap<NodeId, Vec<(NodeId, EdgeLabel)>>,
    ) -> TaskSubmission {
        let mut task_inputs = build_input_bindings(id, incoming, &self.outputs)
            .into_iter()
            .map(|binding| TaskInputBinding {
                name: self.input_name(id, binding.to_port),
                port: binding.to_port,
                source: TaskInputSource::UpstreamPort {
                    from: binding.from,
                    from_port: binding.from_port,
                },
                payload: binding.kind,
                path: binding.path,
                fingerprint: binding.fingerprint,
                staged_paths: Vec::new(),
            })
            .collect::<Vec<_>>();

        let physical_job = self.physical_jobs.get(id);
        if task_inputs.is_empty()
            && let Some(job) = physical_job
            && job.item.is_some()
        {
            let axis = job.axis.clone().unwrap_or_default();
            let dynamic = self.logical_graphs.iter().any(|graph| {
                graph.nodes().iter().any(|node| {
                    node.id == job.logical_node
                        && matches!(
                            node.strategy,
                            LogicalExecutionStrategy::DynamicForEach { .. }
                        )
                })
            });
            let source = if dynamic {
                TaskInputSource::DynamicFanoutItem { axis }
            } else {
                TaskInputSource::ScatterItem { axis }
            };
            task_inputs.push(TaskInputBinding {
                name: "item".into(),
                port: 0,
                source,
                payload: "json".into(),
                path: None,
                fingerprint: None,
                staged_paths: Vec::new(),
            });
        }

        let physical_job = physical_job.cloned();
        let logical_node = physical_job.as_ref().map(|job| job.logical_node.as_str());
        let resources = self
            .task_resources
            .get(id)
            .or_else(|| logical_node.and_then(|node| self.logical_task_resources.get(node)))
            .cloned()
            .unwrap_or_else(|| self.default_task_resources.clone());
        let task_outputs = self
            .nodes
            .get(id)
            .map(|node| {
                node.ports()
                    .output_ports()
                    .iter()
                    .map(|port| TaskOutputBinding {
                        name: self.output_name(id, port.index),
                        port: port.index,
                        payload: port.data_type.to_string(),
                        artifact_paths: Vec::new(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        TaskSubmission {
            spec: TaskSpec {
                id: id.to_string(),
                logical_node: physical_job.as_ref().map(|job| job.logical_node.clone()),
                kind: kind.to_string(),
                spec,
                axis: physical_job.as_ref().and_then(|job| job.axis.clone()),
                item_key: physical_job.as_ref().and_then(|job| job.item_key.clone()),
                item: physical_job.and_then(|job| job.item),
                inputs: task_inputs,
                outputs: task_outputs,
            },
            inputs: inputs.to_vec(),
            resources,
        }
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

        // Audit state is per-run in both modes: a report must express what
        // *this* run injected and what *this* run's nodes reported — clean
        // nodes skipped by an incremental run contribute neither.
        self.input_bindings.clear();
        self.node_run_details.clear();

        if !incremental {
            // Full re-run: clear all cached state. Recorded fingerprints must
            // not survive — every node re-executes.
            self.outputs.clear();
            self.statuses.clear();
            self.fingerprints.clear();
            tracing::info!("Full re-run");
        }
        // In incremental mode, keep cached outputs + fingerprints; the
        // staleness check below drops fingerprints whose cached file outputs
        // no longer match their recorded identity, and the dispatch-time
        // fingerprint comparison decides reuse per node.
        if incremental {
            // Research-grade posture note (F15): fingerprint reuse is only
            // as trustworthy as the identity evidence under it. Metadata
            // identity (size + mtime) misses in-place content edits, so a
            // cache hit can silently reuse stale outputs. The default
            // stays Metadata (zero extra I/O) — this warning makes the
            // trade-off visible on every affected run instead of changing
            // behaviour.
            if cfg.input_hashing != InputHashing::Content {
                tracing::warn!(
                    input_hashing = ?cfg.input_hashing,
                    "incremental execution without content-level input hashing: \
                     cached outputs may be reused across undetected in-place \
                     input edits (set SchedulerConfig::input_hashing = \
                     InputHashing::Content for research-grade reuse)"
                );
            }
            self.invalidate_stale_file_outputs(engine_ctx.opendal.as_deref())
                .await;
            tracing::info!("Incremental execution");
        }

        self.validate()?;
        // Topological order is computed mainly to validate the graph and to seed a
        // deterministic processing order for the ready queue.
        let _topo = self.topo_order()?;

        let mut dynamic_controls: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        for (coordinator, (coordinator_kind, _)) in &self.specs {
            if coordinator_kind != "dynamic_fanout" {
                continue;
            }
            let Some(coordinator_job) = self.physical_jobs.get(coordinator) else {
                continue;
            };
            let actual_jobs = self
                .physical_jobs
                .iter()
                .filter(|(actual, job)| {
                    actual.as_str() != coordinator.as_str()
                        && job.logical_node == coordinator_job.logical_node
                })
                .map(|(actual, _)| actual.clone())
                .collect::<Vec<_>>();
            if !actual_jobs.is_empty() {
                dynamic_controls.insert(coordinator.clone(), actual_jobs);
            }
        }

        let mut all_ids = self.node_ids();
        let mut streaming_expanded = BTreeSet::<NodeId>::new();
        let mut stream = StreamRuntimeState::default();

        // Precompute adjacency + per-node port assignment so the dispatch loop only
        // needs a single mutable borrow of `self`.
        let mut successors: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        // (predecessor id, edge port label) per node, in declared edge order
        let mut incoming: HashMap<NodeId, Vec<(NodeId, EdgeLabel)>> = HashMap::new();
        // unresolved-predecessor count per node. Every predecessor counts:
        // each completes either by executing or by fingerprint reuse, and
        // unblocks its successors.
        let mut pending: HashMap<NodeId, usize> = HashMap::new();
        for id in &all_ids {
            let mut node_successors = self.successors(id);
            let extra_pending = dynamic_controls
                .get(id.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for actual in extra_pending {
                if !node_successors.contains(actual) {
                    node_successors.push(actual.clone());
                }
            }
            successors.insert(id.clone(), node_successors);
            let preds = self.predecessors(id);
            pending.insert(id.clone(), preds.len() + extra_pending.len());
            let inc = self.incoming_edges_with_ports(id);
            incoming.insert(id.clone(), inc);
        }

        if !incremental {
            for id in &all_ids {
                if self.streaming_operator(id)?.is_none() {
                    continue;
                }
                let Some(inputs) = incoming.get(id) else {
                    continue;
                };
                if inputs.is_empty()
                    || !inputs.iter().all(|(from, _)| {
                        self.specs
                            .get(from.as_str())
                            .is_some_and(|(kind, _)| kind == "channel")
                    })
                {
                    continue;
                }
                stream.expected_inputs.insert(
                    id.clone(),
                    inputs
                        .iter()
                        .map(|(from, label)| (from.clone(), label.from_port))
                        .collect::<BTreeSet<_>>(),
                );
            }
        }

        // Every node starts Pending; a reused node flips back to Success at
        // its dispatch turn.
        self.statuses.clear();
        for id in &all_ids {
            self.statuses.insert(id.clone(), RuntimeStatus::Pending);
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
        let event_capacity = all_ids.len().saturating_mul(128).max(128);
        let (tx, mut rx) = mpsc::channel::<NodeEvent>(event_capacity);

        // Per-node execution duration and skip root-cause tracking.
        let mut durations: HashMap<NodeId, std::time::Duration> = HashMap::new();
        let mut skipped_because: HashMap<NodeId, NodeId> = HashMap::new();
        // Dispatch-turn sequence: the scheduler's serialization of a
        // concurrent run (assigned to executed and fingerprint-reused nodes
        // alike), recorded per node so the run report carries the true
        // execution order — `node_ids()` is a HashMap, so no array order is.
        let mut next_dispatch_seq: u64 = 0;
        let mut dispatch_order: HashMap<NodeId, u64> = HashMap::new();

        // Seed the ready queue with source nodes; every other node enters as
        // its predecessors complete (by execution or fingerprint reuse).
        let mut ready: VecDeque<NodeId> = all_ids
            .iter()
            .filter(|id| pending[*id] == 0)
            .cloned()
            .collect();
        let mut in_flight: usize = 0;
        // Nodes actually dispatched for execution this run — the rollback set
        // when a cancellation must release this run's outputs. Reused nodes
        // keep their cached outputs and fingerprints.
        let mut executed_ids: Vec<NodeId> = Vec::new();
        let mut job_handles: Vec<AbortOnDropHandle> = Vec::new();
        let mut external_cancellation = false;

        loop {
            if memory_triggered.is_some() {
                break;
            }

            // Dispatch every currently-ready node.
            while let Some(id) = ready.pop_front() {
                if self
                    .specs
                    .get(&id)
                    .is_some_and(|(kind, _)| kind == "dynamic_fanout")
                {
                    let actual_ids = self.expand_dynamic_fanout(
                        &id,
                        &mut incoming,
                        &mut successors,
                        &mut pending,
                        &mut ready,
                        &mut streaming_expanded,
                    )?;
                    for actual_id in actual_ids {
                        if !all_ids.contains(&actual_id) {
                            all_ids.push(actual_id);
                        }
                    }
                    continue;
                }
                if self.statuses[&id] != RuntimeStatus::Pending {
                    // Already skipped/finished by a cascade — don't dispatch.
                    continue;
                }
                dispatch_order.insert(id.clone(), next_dispatch_seq);
                next_dispatch_seq += 1;
                // Borrow the node payload, then clone it into an owned Box so it
                // can be moved into the 'static future. The original stays in
                // `self` for re-runs / iterative optimisation.
                let Some(node_box) = self.get_node(&id).map(|n| n.clone_box()) else {
                    warn!(node = %id, "scheduler: node payload missing");
                    continue;
                };
                let inputs = build_inputs(&id, &incoming, &self.outputs);
                let bindings = build_input_bindings(&id, &incoming, &self.outputs);
                self.input_bindings.insert(id.clone(), bindings);
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

                // ── Fingerprint gate ─────────────────────────────────────
                // Compute this execution's identity from the current spec,
                // wiring, and upstream values. In incremental mode a node
                // whose candidate fingerprint matches the one recorded at its
                // last successful execution — and whose cached outputs are
                // still present — is reused without re-executing. This is
                // the hash comparison that replaces dirty-mark propagation:
                // spec edits, rewiring, and upstream changes all yield a
                // different fingerprint and force execution.
                let mut identities = crate::fingerprint::collect_input_identities(
                    &id,
                    &incoming,
                    &self.outputs,
                    &self.fingerprints,
                );
                if cfg.input_hashing == InputHashing::Content {
                    crate::fingerprint::upgrade_identities_with_content_hashes(
                        &mut identities,
                        engine_ctx.opendal.as_deref(),
                    )
                    .await;
                }
                let (kind, spec) = self.specs.get(&id).cloned().unwrap_or_else(|| {
                    (
                        self.nodes
                            .get(&id)
                            .map(|node| node.kind().to_string())
                            .unwrap_or_default(),
                        serde_json::Value::Null,
                    )
                });
                // Plugin identity comes from the node payload (WO-R09): for
                // plugin-backed nodes the loaded family's manifest/script/
                // image determine the output just as much as spec and inputs,
                // so they belong in the reuse key. `None` for built-ins.
                let plugin_identity = node_box.plugin_identity().cloned();
                let candidate = crate::fingerprint::compute_node_fingerprint(
                    &kind,
                    Some(&spec),
                    crate::engine_version(),
                    &identities,
                    plugin_identity.as_ref(),
                );
                if incremental
                    && self.fingerprints.get(&id) == Some(&candidate)
                    && self.outputs.contains_key(&id)
                {
                    debug!(node = %id, "fingerprint unchanged; reusing cached output");
                    self.statuses.insert(id.clone(), RuntimeStatus::Success);
                    for succ in &successors[&id] {
                        let left = {
                            let count = pending.entry(succ.clone()).or_insert(0);
                            *count = count.saturating_sub(1);
                            *count
                        };
                        if left == 0 && self.statuses[succ] == RuntimeStatus::Pending {
                            ready.push_back(succ.clone());
                        }
                    }
                    continue;
                }
                // Provisional: replaced by the outputs of this execution on
                // Success, removed again on failure/cancellation.
                self.fingerprints.insert(id.clone(), candidate);
                executed_ids.push(id.clone());

                self.statuses.insert(id.clone(), RuntimeStatus::Running);
                in_flight += 1;
                let submission = self.build_task_submission(&id, &kind, spec, &inputs, &incoming);
                let task_id = submission.spec.id.clone();
                let executor = std::sync::Arc::clone(&self.task_executor.executor);
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
                    let execution = TaskExecution {
                        submission,
                        node: node_box,
                        engine_ctx,
                        reporter,
                        cancellation: task_cancel,
                    };
                    let result = AssertUnwindSafe(executor.run(execution))
                        .catch_unwind()
                        .await;
                    let res = match result {
                        Ok(result) => result,
                        Err(panic_payload) => {
                            let message = panic_payload
                                .downcast_ref::<&str>()
                                .map(|value| (*value).to_string())
                                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| {
                                    "executor panicked with non-string payload".to_string()
                                });
                            warn!(node = %task_id, panic = %message, "task executor panicked");
                            JobResult::Failed {
                                id: task_id,
                                error: DagError::Schedule(format!(
                                    "task executor panicked: {message}"
                                )),
                                duration: std::time::Duration::default(),
                                details: None,
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
                job_handles.push(AbortOnDropHandle(Some(handle)));
            }

            let streaming_active = stream
                .node_counts
                .keys()
                .any(|node| self.statuses.get(node) == Some(&RuntimeStatus::Running))
                || !stream.pending_closes.is_empty();
            if in_flight == 0 && !streaming_active {
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
                        NodeEventKind::ChannelItem {
                            port,
                            sequence: _,
                            item,
                        } => {
                            let source = msg.node_id.clone();
                            self.propagate_stream_item(
                                &source,
                                port,
                                item,
                                &mut incoming,
                                &mut successors,
                                &mut pending,
                                &mut ready,
                                &mut all_ids,
                                &mut streaming_expanded,
                                &mut stream,
                                tx.clone(),
                            )
                            .await?;
                            continue;
                        }
                        NodeEventKind::ChannelClosed { .. } => {
                            stream.pending_closes.remove(&msg.node_id);
                            let NodeEventKind::ChannelClosed {
                                port: from_port, ..
                            } = msg.kind
                            else {
                                unreachable!("matched ChannelClosed");
                            };
                            self.close_stream_successors(
                                &msg.node_id,
                                from_port,
                                &mut successors,
                                &mut pending,
                                &mut ready,
                                &mut stream,
                                tx.clone(),
                            )
                            .await?;
                            continue;
                        }
                    };
                    in_flight -= 1;

                    match res {
                        JobResult::Success {
                            id,
                            outputs: outs,
                            duration,
                            details,
                        } => {
                            // Record execution evidence before the port-validation
                            // branch below: a node whose declared ports reject its own
                            // output is re-classified as Failed, but the evidence of
                            // what actually ran must survive that re-classification.
                            if let Some(details) = details {
                                self.node_run_details.insert(id.clone(), details);
                            }
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
                            details,
                        } => {
                            if let Some(details) = details {
                                self.node_run_details.insert(id.clone(), details);
                            }
                            self.statuses.insert(id.clone(), RuntimeStatus::Failed);
                            // A failed execution produces no valid outputs: drop the
                            // provisional fingerprint so the node re-executes next run.
                            self.fingerprints.remove(&id);
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
        // Give cancelled node tasks a chance to actually unwind so any
        // Arrow buffers they hold locally get dropped before the scheduler
        // returns. Without this grace, dropped `AbortOnDropHandle`s fire
        // `.abort()` and immediately detach — the cancelled futures keep
        // their Arrow `RecordBatch` `Arc`s alive out-of-band and the next
        // `run()` starts with the previous run's memory still pinned
        // (which is why the guard re-trips at the first sample on retry).
        const ABORT_GRACE: Duration = Duration::from_secs(2);
        let pending_joins: Vec<JoinHandle<()>> =
            job_handles.drain(..).map(|h| h.into_join()).collect();
        for join in &pending_joins {
            join.abort();
        }
        for join in pending_joins {
            let _ = tokio::time::timeout(ABORT_GRACE, join).await;
        }

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

            // Drop outputs that this run produced. `executed_ids` tracks
            // exactly the nodes dispatched for execution this run; reused
            // nodes (fingerprint match) keep their cached outputs and
            // fingerprints for the next incremental attempt. Nodes that were
            // cancelled mid-flight get their provisional fingerprints
            // dropped along with any outputs they managed to publish.
            for id in &executed_ids {
                self.outputs.remove(id);
                self.fingerprints.remove(id);
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
            None => ResourceRunReport::default(),
        };

        // Build per-node reports for the agent-friendly result.
        let node_reports = self
            .build_node_reports(
                &all_ids,
                &durations,
                &skipped_because,
                &dispatch_order,
                cfg.compute_row_counts && memory_trigger.is_none(),
            )
            .await;
        let logical_run_summaries = self.build_logical_run_summaries(&node_reports);

        Ok(RunReport {
            ok,
            warnings,
            snapshot_id: None,
            resource,
            nodes: node_reports,
            logical_nodes: logical_run_summaries,
            statuses: self.statuses.clone(),
            errors: self.errors.drain().collect(),
        })
    }
}
