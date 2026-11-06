//! Instance execution: drive an [`ExecutionPlan`] through the executor stack.
//!
//! The scheduler consumes a plan (already materialized and validated at run
//! start) and executes every instance through the **existing** dag-core
//! [`TaskExecutor`] contract — `bind_run` pins the compute target once per
//! run, `preflight` checks every instance's container images before anything
//! dispatches, and `run` carries the node payload built from the reused node
//! registry. Nothing here touches authored state: instances, their statuses
//! and their outputs live only in this run's structures.
//!
//! # What is deliberately v1
//!
//! - No incremental reuse yet (the instance result cache is a later step);
//!   every instance executes on every run.
//! - `Collect` ports receive one [`NodeInput`] per source instance, matching
//!   dag-core's `build_inputs` semantics; value aggregation (FileSet folding)
//!   is left to the consuming node.
//! - Remote dispatch staging (`staged_paths`) is not populated; instances
//!   bound for remote executors will need it when HPC placement is wired.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use dag_core::dag::execution::{TaskExecution, TaskExecutor, TaskSubmission};
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::node_event::{JobResult, NodeEvent, NodeReporter};
use dag_core::dag::{DagError, NodeInput, RuntimeStatus};
use dag_core::node::DagNode;
use dag_core::registry::NodeCtx;
use dag_core::value::{FileRef, NodeValue};
use futures::FutureExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::cache::{InstanceCache, plan_fingerprint};
use crate::error::Error;
use crate::instance::InstanceId;
use crate::model::{ErrorMode, ProcessName, WireOp};
use crate::plan::ExecutionPlan;

/// Builds the executable node payload for one instance from its process kind
/// and rendered spec. Production wiring closes over dag-core's `NodeRegistry`
/// (so every registered kind — including containerized ones — works); tests
/// install their own.
pub type NodeBuilder =
    Arc<dyn Fn(&str, serde_json::Value) -> Result<Box<dyn DagNode>, Error> + Send + Sync>;

/// Host-provided evaluator for persisted channel functions.
pub type ChannelFunction =
    Arc<dyn Fn(&serde_json::Value, Option<&NodeValue>) -> Result<NodeValue, String> + Send + Sync>;
pub type ChannelBinaryFunction = Arc<
    dyn Fn(&serde_json::Value, &NodeValue, &NodeValue) -> Result<NodeValue, String> + Send + Sync,
>;

/// Spec key carrying a per-instance execution timeout in milliseconds
/// (`node('x', 'kind', {'timeout_ms': 5000})`). Without it the node
/// declaration's own budget — by default none — applies.
pub(crate) const SPEC_TIMEOUT_MS_KEY: &str = "timeout_ms";

/// Executor resources for one instance: the node declaration's baseline
/// overlaid with the author's spec-declared `timeout_ms` budget. The spec key
/// is the explicit per-node override and wins over the declaration; the
/// executor stack rejects a zero budget via `TaskResources::validate`.
pub(crate) fn declared_resources(
    node: &dyn DagNode,
    spec: &serde_json::Value,
) -> dag_core::TaskResources {
    let mut resources = node.task_resources().unwrap_or_default();
    if let Some(millis) = spec.get(SPEC_TIMEOUT_MS_KEY).and_then(|value| value.as_u64()) {
        resources.max_duration_ms = Some(millis);
    }
    resources
}

/// Tuning knobs for one flow run.
#[derive(Debug, Clone)]
pub struct FlowRunConfig {
    /// Maximum instances executing concurrently. `0` disables this gate (the
    /// executor stack's own admission — e.g. `LimitedTaskExecutor` — still
    /// applies independently, exactly like dag-core's scheduler).
    pub max_concurrency: usize,
    /// Reuse instances whose recomputed fingerprint matches a cached result
    /// whose files are still valid (the flow engine's `-resume`).
    pub resume: bool,
}

impl Default for FlowRunConfig {
    fn default() -> Self {
        Self {
            max_concurrency: std::thread::available_parallelism()
                .map(|cpus| cpus.get())
                .unwrap_or(1),
            resume: false,
        }
    }
}

/// Terminal record of one instance: the drill-down view.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub id: InstanceId,
    pub process: ProcessName,
    pub item_key: Option<String>,
    pub status: RuntimeStatus,
    /// True when the result came from the instance cache, not the executor.
    pub reused: bool,
    /// True when a `Failed` record was tolerated by `error_strategy: ignore`
    /// (it does not fail the run).
    pub ignored: bool,
    /// Executor attempts, including retries (`1` unless a retry happened).
    pub attempts: usize,
    pub outputs: BTreeMap<u8, NodeValue>,
    /// Failure text (`DagError` display) for `Failed` instances.
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub execution: Option<dag_core::dag::runtime::NodeRunDetails>,
}

/// Per-process rollup — the default observation granularity (Nextflow's
/// process-level monitor view; per-instance detail is the drill-down).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProcessRunSummary {
    pub total: usize,
    pub succeeded: usize,
    /// Succeeded instances served from the cache (a subset of `succeeded`).
    pub reused: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelled: usize,
    /// Failures tolerated by `error_strategy: ignore`.
    pub ignored: usize,
    pub summed_elapsed_ms: u64,
}

/// Grouped run outcome.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FlowRunReport {
    pub run_id: Option<String>,
    pub audit_persisted: bool,
    /// True when every materialized instance ended `Success` (vacuously true
    /// for a run whose fanouts resolved to zero items — matching the "no
    /// data, nothing to do" posture).
    pub ok: bool,
    /// The caller requested cancellation, even if no task was instantiated.
    pub cancelled: bool,
    /// Frozen executor binding for this invocation (from `bind_run`), when
    /// the executor provides one.
    pub execution_binding: Option<serde_json::Value>,
    pub processes: BTreeMap<ProcessName, ProcessRunSummary>,
    #[serde(rename = "nodes", serialize_with = "serialize_instance_records")]
    pub instances: Vec<InstanceRecord>,
}

/// One flow scheduler: executor stack + node builder + engine context. Cheap
/// to construct; holds no mutable graph state.
pub struct FlowScheduler {
    pub(crate) executor: Arc<dyn TaskExecutor>,
    pub(crate) node_builder: NodeBuilder,
    pub(crate) engine_ctx: Arc<NodeCtx>,
    pub(crate) channel_function: Option<ChannelFunction>,
    pub(crate) channel_binary_function: Option<ChannelBinaryFunction>,
}

impl FlowScheduler {
    pub fn new(
        executor: Arc<dyn TaskExecutor>,
        node_builder: NodeBuilder,
        engine_ctx: Arc<NodeCtx>,
    ) -> Self {
        Self {
            executor,
            node_builder,
            engine_ctx,
            channel_function: None,
            channel_binary_function: None,
        }
    }

    pub fn with_channel_function(mut self, evaluator: ChannelFunction) -> Self {
        self.channel_function = Some(evaluator);
        self
    }

    pub fn with_channel_binary_function(mut self, evaluator: ChannelBinaryFunction) -> Self {
        self.channel_binary_function = Some(evaluator);
        self
    }

    /// Execute a plan. Fails (before any dispatch) when a node cannot be
    /// built or the bound executor's preflight rejects the instance set —
    /// e.g. a remote target without an admitted image mapping.
    pub async fn run(
        &self,
        plan: &ExecutionPlan,
        cfg: &FlowRunConfig,
        mut cache: Option<&mut InstanceCache>,
        external_cancel: Option<CancellationToken>,
    ) -> Result<FlowRunReport, Error> {
        // ── Build every instance's node payload up front ─────────────────
        // A broken kind/spec must fail the whole run before anything
        // dispatches.
        let mut nodes = BTreeMap::new();
        for process_plan in plan.processes.values() {
            for instance in &process_plan.instances {
                let node = (self.node_builder)(&process_plan.kind, instance.rendered_spec.clone())?;
                nodes.insert(instance.id.clone(), node);
            }
        }

        // ── Bind the executor for this run ────────────────────────────────
        let run_executor = self
            .executor
            .bind_run()
            .await
            .map_err(|error| Error::Execution(error.to_string()))?
            .unwrap_or_else(|| Arc::clone(&self.executor));

        // Preflight covers every instance of this run, so a remote target
        // reports its blocker before the first dispatch, not at the Nth
        // instance.
        let preflight: Vec<(&str, &dyn DagNode)> = nodes
            .iter()
            .map(|(id, node)| (id.as_str(), node.as_ref()))
            .collect();
        run_executor
            .preflight(&preflight)
            .map_err(|error| Error::Execution(error.to_string()))?;

        // ── Instance graph: pending counts and successor edges ────────────
        let mut successors: BTreeMap<InstanceId, BTreeSet<InstanceId>> = BTreeMap::new();
        let mut pending: BTreeMap<InstanceId, usize> = BTreeMap::new();
        for id in nodes.keys() {
            successors.insert(id.clone(), BTreeSet::new());
            pending.insert(id.clone(), 0);
        }
        for route in &plan.routes {
            successors
                .entry(route.from.clone())
                .or_default()
                .insert(route.to.clone());
        }
        for id in nodes.keys().cloned().collect::<Vec<_>>() {
            let predecessors: BTreeSet<InstanceId> = plan
                .routes
                .iter()
                .filter(|route| route.to == id)
                .map(|route| route.from.clone())
                .collect();
            pending.insert(id, predecessors.len());
        }

        let run_cancel = CancellationToken::new();
        if let Some(external) = &external_cancel {
            let internal = run_cancel.clone();
            let external = external.clone();
            tokio::spawn(async move {
                external.cancelled().await;
                internal.cancel();
            });
        }

        let mut statuses: BTreeMap<InstanceId, RuntimeStatus> = nodes
            .keys()
            .map(|id| (id.clone(), RuntimeStatus::Pending))
            .collect();
        let mut outputs: BTreeMap<InstanceId, PortOutputs> = BTreeMap::new();
        // Fingerprint of every settled instance (reused or executed) — the
        // identity downstream instances hash into their own fingerprints.
        let mut effective: BTreeMap<InstanceId, String> = BTreeMap::new();
        // Executor attempts per instance (retries included).
        let mut attempts: BTreeMap<InstanceId, usize> = BTreeMap::new();
        // Outcomes that fail the run: fatal failures, their cascaded skips,
        // and cancellations. Ignored failures never enter this set.
        let mut fatal: BTreeSet<InstanceId> = BTreeSet::new();
        // Delayed retry re-enqueue: a sleeping task sends the instance id
        // back when its backoff elapses.
        let (requeue_tx, mut requeue_rx) = mpsc::unbounded_channel::<InstanceId>();
        // Backoffs in flight: the loop must stay alive while retries are
        // sleeping even though nothing is ready or executing.
        let mut delayed_retries = 0usize;
        let mut records: BTreeMap<InstanceId, InstanceRecord> = BTreeMap::new();

        let (event_tx, mut event_rx) = mpsc::channel::<NodeEvent>(64);
        let (result_tx, mut result_rx) = mpsc::channel::<JobResult>(16);

        let mut ready: VecDeque<InstanceId> = pending
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(id, _)| id.clone())
            .collect();
        // Source deliveries are pre-settled: sources never execute — the
        // item IS the output (Nextflow: a channel value arriving).
        for source_plan in plan.sources.values() {
            for item in &source_plan.items {
                let mut delivery = PortOutputs::new();
                delivery.insert_file(
                    0,
                    FileRef {
                        path: item.path.clone(),
                        format: None,
                        fingerprint: None,
                    },
                );
                statuses.insert(item.id.clone(), RuntimeStatus::Success);
                outputs.insert(item.id.clone(), delivery);
                effective.insert(item.id.clone(), item.fingerprint.clone());
                release_successors(
                    &item.id,
                    &successors,
                    &mut pending,
                    &mut statuses,
                    &mut ready,
                );
            }
        }

        let mut in_flight = 0usize;
        let limit = cfg.max_concurrency;

        // ── Dispatch / completion loop ────────────────────────────────────
        while in_flight > 0 || !ready.is_empty() || delayed_retries > 0 {
            // Dispatch while capacity remains.
            while let Some(id) = ready.pop_front() {
                // Resume short-circuit: a fingerprint hit with still-valid
                // outputs never reaches the executor.
                if cfg.resume
                    && let Some(cache) = cache.as_deref_mut()
                    && let Some(fingerprint) = self
                        .try_reuse(
                            plan,
                            &id,
                            cache,
                            &effective,
                            &mut statuses,
                            &mut outputs,
                            &mut records,
                        )
                        .await
                {
                    effective.insert(id.clone(), fingerprint);
                    release_successors(&id, &successors, &mut pending, &mut statuses, &mut ready);
                    continue;
                }
                if limit != 0 && in_flight >= limit {
                    ready.push_front(id);
                    break;
                }
                *attempts.entry(id.clone()).or_insert(0) += 1;
                self.dispatch(
                    plan,
                    &id,
                    &nodes,
                    &outputs,
                    &run_executor,
                    &event_tx,
                    &result_tx,
                    &run_cancel,
                );
                statuses.insert(id, RuntimeStatus::Running);
                in_flight += 1;
            }

            // Nothing executing and nothing sleeping: a pure-reuse (or
            // drained) pass cannot be woken by any event — re-check the
            // loop condition instead of parking. A pending backoff CAN wake
            // the select, so it falls through.
            if in_flight == 0 && delayed_retries == 0 {
                continue;
            }
            tokio::select! {
                event = event_rx.recv() => {
                    // Lightweight node telemetry; the live view hookup
                    // consumes this stream later.
                    drop(event);
                }
                result = result_rx.recv() => {
                    let Some(result) = result else { continue };
                    in_flight -= 1;
                    self.complete(
                        plan, result, &mut statuses, &mut outputs, &mut records,
                        &mut effective, cache.as_deref_mut(),
                        &mut attempts, &mut fatal,
                        &successors, &mut pending, &mut ready, &requeue_tx,
                        &mut delayed_retries,
                    );
                }
                // A delayed retry's backoff elapsed: put the instance back
                // on the ready queue.
                requeued = requeue_rx.recv() => {
                    if let Some(id) = requeued {
                        delayed_retries = delayed_retries.saturating_sub(1);
                        statuses.insert(id.clone(), RuntimeStatus::Pending);
                        ready.push_back(id);
                    }
                }
                _ = run_cancel.cancelled() => {
                    break;
                }
            }
        }

        // The plan-based API also has to wait for remote cancellation: its
        // dispatched tasks are detached, but each sends one terminal result.
        let mut unconfirmed_stops = BTreeMap::new();
        if run_cancel.is_cancelled() && run_executor.requires_cancel_join() {
            while in_flight > 0 {
                let Some(result) = result_rx.recv().await else {
                    break;
                };
                in_flight -= 1;
                if let JobResult::Failed { id, error, .. } = result {
                    let error = error.to_string();
                    if error.contains("remote_stop_unconfirmed") {
                        unconfirmed_stops.insert(InstanceId(id), error);
                    }
                }
            }
        }

        // Compose the terminal report from statuses so instances that never
        // dispatched (skipped by cascade, cancelled by abort) are present.
        // After an abort, everything not yet terminal reads as cancelled.
        if run_cancel.is_cancelled() {
            for (id, status) in statuses.iter_mut() {
                if matches!(status, RuntimeStatus::Pending | RuntimeStatus::Running) {
                    *status = if unconfirmed_stops.contains_key(id) {
                        RuntimeStatus::Failed
                    } else {
                        RuntimeStatus::Cancelled
                    };
                    fatal.insert(id.clone());
                }
            }
        }
        for (id, status) in &statuses {
            // Source pseudo-instances are deliveries, not runs.
            if plan.is_source_item(id) {
                continue;
            }
            let record = records.entry(id.clone()).or_insert_with(|| InstanceRecord {
                execution: None,
                reused: false,
                ignored: false,
                attempts: 1,
                id: id.clone(),
                process: plan
                    .processes
                    .values()
                    .find(|process_plan| process_plan.instances.iter().any(|i| &i.id == id))
                    .map(|process_plan| process_plan.process.clone())
                    .unwrap_or_else(|| id.parts().0.to_string()),
                item_key: id.parts().1.map(str::to_string),
                status: *status,
                outputs: BTreeMap::new(),
                error: None,
                elapsed_ms: 0,
            });
            record.status = *status;
            if let Some(error) = unconfirmed_stops.remove(id) {
                record.error = Some(error);
            }
        }

        // A run is ok when nothing fatal happened: every instance either
        // succeeded or was an explicitly tolerated failure (Nextflow: an
        // `ignore`d error does not fail the pipeline).
        let cancelled = external_cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled);
        let ok = fatal.is_empty() && !cancelled;
        let mut report = FlowRunReport {
            run_id: None,
            audit_persisted: false,
            ok,
            cancelled,
            execution_binding: run_executor.execution_binding(),
            processes: plan
                .processes
                .keys()
                .map(|name| (name.clone(), ProcessRunSummary::default()))
                .collect(),
            instances: records.into_values().collect(),
        };
        for record in &report.instances {
            let summary = report.processes.get_mut(&record.process).expect(
                "every instance belongs to a plan process; summary rows were seeded from the plan",
            );
            summary.total += 1;
            if record.reused {
                summary.reused += 1;
            }
            if record.ignored {
                summary.ignored += 1;
            }
            match record.status {
                RuntimeStatus::Success => summary.succeeded += 1,
                RuntimeStatus::Failed => summary.failed += 1,
                RuntimeStatus::Skipped => summary.skipped += 1,
                RuntimeStatus::Cancelled => summary.cancelled += 1,
                _ => {}
            }
            summary.summed_elapsed_ms += record.elapsed_ms;
        }
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &self,
        plan: &ExecutionPlan,
        id: &InstanceId,
        nodes: &BTreeMap<InstanceId, Box<dyn DagNode>>,
        outputs: &BTreeMap<InstanceId, PortOutputs>,
        executor: &Arc<dyn TaskExecutor>,
        event_tx: &mpsc::Sender<NodeEvent>,
        result_tx: &mpsc::Sender<JobResult>,
        run_cancel: &CancellationToken,
    ) {
        let node = nodes
            .get(id)
            .expect("dispatched ids come from the node set")
            .clone_box();
        let submission = self.build_submission(plan, id, node.as_ref(), outputs);
        let reporter = NodeReporter::new(id.as_str().to_string(), event_tx.clone());
        let execution = TaskExecution {
            submission,
            node,
            engine_ctx: Arc::clone(&self.engine_ctx),
            reporter,
            cancellation: run_cancel.clone(),
        };
        let task_id = execution.submission.spec.id.clone();
        let executor = Arc::clone(executor);
        let result_tx = result_tx.clone();
        tokio::spawn(async move {
            let outcome = AssertUnwindSafe(executor.run(execution))
                .catch_unwind()
                .await;
            let result = match outcome {
                Ok(result) => result,
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<&str>()
                        .map(|value| (*value).to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "instance panicked".into());
                    JobResult::Failed {
                        id: task_id,
                        error: DagError::Schedule(if executor.requires_cancel_join() {
                            format!("remote_stop_unconfirmed: task executor panicked: {message}")
                        } else {
                            message
                        }),
                        duration: std::time::Duration::ZERO,
                        details: None,
                    }
                }
            };
            let _ = result_tx.send(result).await;
        });
    }

    /// Attempt a cache hit for one instance. On success the instance is
    /// settled as `Success` with the cached outputs (marked `reused`) and
    /// its fingerprint is returned for downstream identity. Any doubt —
    /// missing fingerprint inputs, stale entry, invalidated files,
    /// non-representable outputs — is a miss that dispatches normally.
    #[allow(clippy::too_many_arguments)]
    async fn try_reuse(
        &self,
        plan: &ExecutionPlan,
        id: &InstanceId,
        cache: &mut InstanceCache,
        effective: &BTreeMap<InstanceId, String>,
        statuses: &mut BTreeMap<InstanceId, RuntimeStatus>,
        outputs: &mut BTreeMap<InstanceId, PortOutputs>,
        records: &mut BTreeMap<InstanceId, InstanceRecord>,
    ) -> Option<String> {
        let fingerprint = plan_fingerprint(plan, id, effective)?;
        let (process, item_key) = provenance(plan, id);
        let cached = cache.lookup(&process, item_key.as_deref())?;
        if cached.fingerprint != fingerprint {
            return None;
        }
        if !cached.still_valid(self.engine_ctx.opendal.as_deref()).await {
            return None;
        }
        let port_outputs = cached.to_port_outputs()?;
        statuses.insert(id.clone(), RuntimeStatus::Success);
        outputs.insert(id.clone(), port_outputs);
        records.insert(
            id.clone(),
            record_for(
                plan,
                id,
                RuntimeStatus::Success,
                outputs[id]
                    .iter()
                    .map(|(port, value)| (*port, value.clone()))
                    .collect(),
                None,
                std::time::Duration::ZERO,
                true,
                false,
                1,
            ),
        );
        Some(fingerprint)
    }

    /// Assemble the executor submission for one instance: inputs from the
    /// upstream instances' outputs on their route ports, the outputs contract
    /// from the node's declared ports, resources from the node's declaration
    /// overlaid with the spec-declared execution timeout.
    fn build_submission(
        &self,
        plan: &ExecutionPlan,
        id: &InstanceId,
        node: &dyn DagNode,
        outputs: &BTreeMap<InstanceId, PortOutputs>,
    ) -> TaskSubmission {
        let (process_name, kind, instance) = plan
            .processes
            .values()
            .find_map(|process_plan| {
                process_plan
                    .instances
                    .iter()
                    .find(|instance| &instance.id == id)
                    .map(|instance| {
                        (
                            process_plan.process.clone(),
                            process_plan.kind.clone(),
                            instance,
                        )
                    })
            })
            .expect("dispatched instances come from the plan");

        let mut inputs = Vec::new();
        let mut bindings = Vec::new();
        for route in plan.routes_to(id) {
            let Some(upstream) = outputs.get(&route.from) else {
                continue;
            };
            let Some(value) = upstream.get(&route.from_port) else {
                continue;
            };
            inputs.push(NodeInput {
                port: route.to_port,
                data: value.clone(),
            });
            bindings.push(dag_core::dag::execution::TaskInputBinding {
                name: node
                    .ports()
                    .input_port(route.to_port)
                    .and_then(|port| port.label.clone())
                    .unwrap_or_else(|| format!("port_{}", route.to_port)),
                port: route.to_port,
                source: dag_core::dag::execution::TaskInputSource::UpstreamPort {
                    from: route.from.as_str().to_string(),
                    from_port: route.from_port,
                },
                payload: value.data_type().to_string(),
                path: match value {
                    NodeValue::File(file) => Some(file.path.clone()),
                    NodeValue::FileSet(files) => files.first().map(|file| file.path.clone()),
                    _ => None,
                },
                fingerprint: match value {
                    NodeValue::File(file) => file.fingerprint.clone(),
                    _ => None,
                },
                staged_paths: Vec::new(),
            });
        }

        let outputs_contract = node
            .ports()
            .output_ports()
            .iter()
            .map(|port| dag_core::dag::execution::TaskOutputBinding {
                name: port
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("port_{}", port.index)),
                port: port.index,
                payload: port.data_type.to_string(),
                artifact_paths: Vec::new(),
            })
            .collect();

        TaskSubmission {
            spec: dag_core::dag::execution::TaskSpec {
                id: id.as_str().to_string(),
                logical_node: Some(process_name),
                kind,
                spec: instance.rendered_spec.clone(),
                item_key: instance.item_key.clone(),
                item: instance.item.clone(),
                inputs: bindings,
                outputs: outputs_contract,
            },
            inputs,
            resources: declared_resources(node, &instance.rendered_spec),
        }
    }

    /// Record one terminal result and release (or skip) dependents.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn complete(
        &self,
        plan: &ExecutionPlan,
        result: JobResult,
        statuses: &mut BTreeMap<InstanceId, RuntimeStatus>,
        outputs: &mut BTreeMap<InstanceId, PortOutputs>,
        records: &mut BTreeMap<InstanceId, InstanceRecord>,
        effective: &mut BTreeMap<InstanceId, String>,
        mut cache: Option<&mut InstanceCache>,
        attempts: &mut BTreeMap<InstanceId, usize>,
        fatal: &mut BTreeSet<InstanceId>,
        successors: &BTreeMap<InstanceId, BTreeSet<InstanceId>>,
        pending: &mut BTreeMap<InstanceId, usize>,
        ready: &mut VecDeque<InstanceId>,
        requeue_tx: &mpsc::UnboundedSender<InstanceId>,
        delayed_retries: &mut usize,
    ) {
        let evidence = match &result {
            JobResult::Success { id, details, .. } | JobResult::Failed { id, details, .. } => {
                (InstanceId::adopt(id.clone()), details.clone())
            }
        };
        match result {
            JobResult::Success {
                id,
                outputs: outs,
                duration,
                ..
            } => {
                let id = InstanceId::adopt(id);
                statuses.insert(id.clone(), RuntimeStatus::Success);
                outputs.insert(id.clone(), outs);
                records.insert(
                    id.clone(),
                    record_for(
                        plan,
                        &id,
                        RuntimeStatus::Success,
                        outputs[&id]
                            .iter()
                            .map(|(port, value)| (*port, value.clone()))
                            .collect(),
                        None,
                        duration,
                        false,
                        false,
                        *attempts.get(&id).unwrap_or(&1),
                    ),
                );
                // Identity settles after execution: reused and freshly
                // executed instances are indistinguishable downstream.
                if let Some(fingerprint) = plan_fingerprint(plan, &id, effective) {
                    if let Some(cache) = cache.as_deref_mut() {
                        let (process, item_key) = provenance(plan, &id);
                        cache.record(
                            &process,
                            item_key.as_deref(),
                            fingerprint.clone(),
                            &outputs[&id],
                        );
                    }
                    effective.insert(id.clone(), fingerprint);
                }
                release_successors(&id, successors, pending, statuses, ready);
            }
            JobResult::Failed {
                id,
                error,
                duration,
                ..
            } => {
                let id = InstanceId::adopt(id);
                let (process, item_key) = provenance(plan, &id);
                let strategy = plan
                    .processes
                    .get(&process)
                    .map(|process_plan| process_plan.error_strategy.clone())
                    .unwrap_or_default();
                let made_attempts = *attempts.get(&id).unwrap_or(&1);
                let stop_unconfirmed = error.to_string().contains("remote_stop_unconfirmed");

                // Retry: re-dispatch until the budget is exhausted, then
                // degrade to terminal failure handling.
                if !stop_unconfirmed
                    && strategy.mode == ErrorMode::Retry
                    && made_attempts <= strategy.max_retries
                {
                    statuses.insert(id.clone(), RuntimeStatus::Pending);
                    if strategy.retry_delay_ms == 0 {
                        ready.push_back(id);
                    } else {
                        *delayed_retries += 1;
                        let delay = std::time::Duration::from_millis(strategy.retry_delay_ms);
                        let requeue_tx = requeue_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            let _ = requeue_tx.send(id);
                        });
                    }
                    return;
                }

                let ignored = !stop_unconfirmed && strategy.mode == ErrorMode::Ignore;
                statuses.insert(id.clone(), RuntimeStatus::Failed);
                records.insert(
                    id.clone(),
                    record_for(
                        plan,
                        &id,
                        RuntimeStatus::Failed,
                        BTreeMap::new(),
                        Some(error.to_string()),
                        duration,
                        false,
                        ignored,
                        made_attempts,
                    ),
                );
                // A failed re-execution must never leave an older success
                // servable.
                if let Some(cache) = cache {
                    cache.invalidate(&process, item_key.as_deref());
                }
                if ignored {
                    // The item was never delivered: Direct dependents skip,
                    // Collect consumers run on the surviving bindings.
                    release_ignored(plan, &id, statuses, records, pending, ready);
                } else {
                    fatal.insert(id.clone());
                    cascade_skip(plan, &id, successors, pending, statuses, records, fatal);
                }
            }
        }
        if let Some(record) = records.get_mut(&evidence.0) {
            record.execution = evidence.1;
        }
    }
}

use std::panic::AssertUnwindSafe;

impl InstanceId {
    fn adopt(id: String) -> Self {
        debug_assert!(id.contains(crate::instance::INSTANCE_ID_SEPARATOR));
        Self(id)
    }
}

#[allow(clippy::too_many_arguments)]
fn record_for(
    plan: &ExecutionPlan,
    id: &InstanceId,
    status: RuntimeStatus,
    outputs: BTreeMap<u8, NodeValue>,
    error: Option<String>,
    duration: std::time::Duration,
    reused: bool,
    ignored: bool,
    attempts: usize,
) -> InstanceRecord {
    let (process, item_key) = plan
        .processes
        .values()
        .find_map(|process_plan| {
            process_plan
                .instances
                .iter()
                .find(|instance| &instance.id == id)
                .map(|instance| (process_plan.process.clone(), instance.item_key.clone()))
        })
        .unwrap_or_else(|| (id.parts().0.to_string(), id.parts().1.map(str::to_string)));
    InstanceRecord {
        execution: None,
        id: id.clone(),
        process,
        item_key,
        status,
        reused,
        ignored,
        attempts,
        outputs,
        error,
        elapsed_ms: duration.as_millis() as u64,
    }
}

/// `(process, item_key)` of one instance id, from the plan.
fn provenance(plan: &ExecutionPlan, id: &InstanceId) -> (String, Option<String>) {
    plan.processes
        .values()
        .find_map(|process_plan| {
            process_plan
                .instances
                .iter()
                .find(|instance| &instance.id == id)
                .map(|instance| (process_plan.process.clone(), instance.item_key.clone()))
        })
        .unwrap_or_else(|| {
            let (process, key) = id.parts();
            (process.to_string(), key.map(str::to_string))
        })
}

/// Decrement distinct successors of a finished instance and enqueue the ones
/// that became ready.
fn release_successors(
    id: &InstanceId,
    successors: &BTreeMap<InstanceId, BTreeSet<InstanceId>>,
    pending: &mut BTreeMap<InstanceId, usize>,
    statuses: &mut BTreeMap<InstanceId, RuntimeStatus>,
    ready: &mut VecDeque<InstanceId>,
) {
    for successor in successors.get(id).cloned().unwrap_or_default() {
        if statuses.get(&successor) != Some(&RuntimeStatus::Pending) {
            continue;
        }
        if let Some(count) = pending.get_mut(&successor) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                ready.push_back(successor);
            }
        }
    }
}

/// Mark all transitive dependents of a fatally failed instance as skipped.
/// Skipped instances are themselves fatal: they never ran because the run's
/// contract was broken, not because an upstream error was tolerated.
fn cascade_skip(
    plan: &ExecutionPlan,
    id: &InstanceId,
    successors: &BTreeMap<InstanceId, BTreeSet<InstanceId>>,
    pending: &mut BTreeMap<InstanceId, usize>,
    statuses: &mut BTreeMap<InstanceId, RuntimeStatus>,
    records: &mut BTreeMap<InstanceId, InstanceRecord>,
    fatal: &mut BTreeSet<InstanceId>,
) {
    let mut queue = VecDeque::from([id.clone()]);
    while let Some(current) = queue.pop_front() {
        for successor in successors.get(&current).cloned().unwrap_or_default() {
            if statuses.get(&successor) != Some(&RuntimeStatus::Pending) {
                continue;
            }
            statuses.insert(successor.clone(), RuntimeStatus::Skipped);
            pending.remove(&successor);
            fatal.insert(successor.clone());
            records.insert(
                successor.clone(),
                record_for(
                    plan,
                    &successor,
                    RuntimeStatus::Skipped,
                    BTreeMap::new(),
                    None,
                    std::time::Duration::ZERO,
                    false,
                    false,
                    0,
                ),
            );
            queue.push_back(successor);
        }
    }
}

/// Release the dependents of a tolerated (`ignore`) failure:
/// - `Direct` dependents skip — the item was never delivered (Nextflow:
///   the downstream process simply never fires for that item). The skip is
///   benign: it does not fail the run, and it propagates with the same
///   ignore semantics.
/// - `Collect` consumers treat the failure as a settled-no-output
///   predecessor: their pending count drops and they run on the surviving
///   bindings once every other predecessor settled.
fn release_ignored(
    plan: &ExecutionPlan,
    id: &InstanceId,
    statuses: &mut BTreeMap<InstanceId, RuntimeStatus>,
    records: &mut BTreeMap<InstanceId, InstanceRecord>,
    pending: &mut BTreeMap<InstanceId, usize>,
    ready: &mut VecDeque<InstanceId>,
) {
    let mut direct_skips = VecDeque::from([id.clone()]);
    // Collect successors of the failed instance settle immediately.
    for route in plan
        .routes
        .iter()
        .filter(|route| &route.from == id && route.op == WireOp::Collect)
    {
        if let Some(count) = pending.get_mut(&route.to) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                ready.push_back(route.to.clone());
            }
        }
    }
    // Direct successors skip, and their own successors inherit the same
    // semantics (a skipped instance delivers nothing on its edges).
    while let Some(current) = direct_skips.pop_front() {
        for route in plan.routes.iter().filter(|route| route.from == current) {
            let successor = &route.to;
            match route.op {
                WireOp::Direct => {
                    if statuses.get(successor) != Some(&RuntimeStatus::Pending) {
                        continue;
                    }
                    statuses.insert(successor.clone(), RuntimeStatus::Skipped);
                    pending.remove(successor);
                    records.insert(
                        successor.clone(),
                        record_for(
                            plan,
                            successor,
                            RuntimeStatus::Skipped,
                            BTreeMap::new(),
                            None,
                            std::time::Duration::ZERO,
                            false,
                            false,
                            0,
                        ),
                    );
                    direct_skips.push_back(successor.clone());
                }
                WireOp::Collect => {
                    if &current == id {
                        continue; // handled above
                    }
                    if let Some(count) = pending.get_mut(successor) {
                        *count = count.saturating_sub(1);
                        if *count == 0 && statuses.get(successor) == Some(&RuntimeStatus::Pending) {
                            ready.push_back(successor.clone());
                        }
                    }
                }
            }
        }
    }
}

/// Convenience for tests and embedders: a file-carrying value.
#[allow(dead_code)]
fn file_value(path: impl Into<String>) -> NodeValue {
    NodeValue::File(FileRef {
        path: path.into(),
        format: None,
        fingerprint: None,
    })
}

fn serialize_instance_records<S: serde::Serializer>(
    records: &[InstanceRecord],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut sequence = serializer.serialize_seq(Some(records.len()))?;
    for record in records {
        sequence.serialize_element(&record.audit_json())?;
    }
    sequence.end()
}

impl InstanceRecord {
    /// Durable metadata only: never collect Arrow data into an audit report.
    pub fn audit_json(&self) -> serde_json::Value {
        let mut files = Vec::new();
        let mut ports = BTreeMap::new();
        for (port, value) in &self.outputs {
            match value {
                NodeValue::File(file) => {
                    files.push(file.clone());
                    ports.insert(*port, file.clone());
                }
                NodeValue::FileSet(values) => files.extend(values.iter().cloned()),
                _ => {}
            }
        }
        if let Some(execution) = &self.execution {
            for artifact in &execution.output_artifacts {
                if !files.iter().any(|file| file.path == artifact.path) {
                    files.push(artifact.clone());
                }
            }
        }
        serde_json::json!({
            "id": self.id.as_str(), "logical_node": self.process, "process": self.process,
            "item_key": self.item_key, "status": self.status,
            "reused": self.reused, "ignored": self.ignored, "attempts": self.attempts,
            "elapsed_ms": self.elapsed_ms, "output_files": files, "port_assignments": ports,
            "error": self.error.as_ref().map(|message| serde_json::json!({"message":message})),
            "execution": self.execution,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FlowGraph, FlowGraphBuilder, Process, Source, Wire};
    use crate::plan::{SourceResolution, build_plan};

    /// Echo node writing REAL files (the cache validates size/mtime, so
    /// fake paths would always miss). Output path derives from the tag plus
    /// every input file's basename, so it is deterministic per instance and
    /// distinct across items. Content = `tag|input contents`. `fail_on`
    /// fails the exact matching tag. `EXECUTIONS` counts real executions so
    /// resume tests can assert that cache hits never reach a node.
    static UNIQUENESS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Per-thread execution counter: each `#[tokio::test]` runs on its own
    /// current-thread runtime, so counts stay test-local even in parallel.
    thread_local! {
        static EXECUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn unique_tag(prefix: &str) -> String {
        format!(
            "{prefix}{}",
            UNIQUENESS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        )
    }

    thread_local! {
        static FAIL_BUDGETS: std::cell::RefCell<std::collections::BTreeMap<String, usize>> =
            std::cell::RefCell::new(std::collections::BTreeMap::new());
    }

    #[derive(Clone)]
    struct EchoNode {
        meta: dag_core::NodePorts,
        tag: String,
        fail_on: Option<String>,
        fail_times: Option<usize>,
    }

    impl EchoNode {
        fn build(kind: &str, spec: serde_json::Value) -> Result<Box<dyn DagNode>, Error> {
            assert_eq!(kind, "echo");
            Ok(Box::new(Self {
                meta: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::File)
                    .add_output_port_of_type(None, dag_core::value::PortType::File),
                tag: spec
                    .get("tag")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("echo")
                    .to_string(),
                fail_on: spec
                    .get("fail_on")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                fail_times: spec
                    .get("fail_times")
                    .and_then(serde_json::Value::as_u64)
                    .map(|times| times as usize),
            }))
        }
    }

    #[async_trait::async_trait]
    impl DagNode for EchoNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.meta
        }

        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            EXECUTIONS.with(|counter| counter.set(counter.get() + 1));
            if let Some(needle) = &self.fail_on {
                if *needle == self.tag {
                    return Err(DagError::Schedule("echo failed by request".into()));
                }
            }
            // Transient failure budget: the first `fail_times` executions of
            // this tag fail, later ones succeed — retry semantics under test.
            if let Some(budget) = self.fail_times {
                let tag = self.tag.clone();
                let remaining = FAIL_BUDGETS.with(|budgets| {
                    let mut budgets = budgets.borrow_mut();
                    let remaining = budgets.entry(tag).or_insert(budget);
                    let value = *remaining;
                    *remaining = remaining.saturating_sub(1);
                    value
                });
                if remaining > 0 {
                    return Err(DagError::Schedule("echo transient failure".into()));
                }
            }
            let mut name_parts = vec![self.tag.clone()];
            let mut content = String::new();
            for input in inputs {
                let NodeValue::File(file) = &input.data else {
                    continue;
                };
                let base = std::path::Path::new(&file.path)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("in");
                name_parts.push(base.to_string());
                if let Ok(text) =
                    std::fs::read_to_string(file.path.strip_prefix("vfs://").unwrap_or(&file.path))
                {
                    content.push_str(&text);
                }
            }
            let safe: String = name_parts
                .join("__")
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = std::env::temp_dir().join(format!("dag-engine-echo-{safe}.out"));
            std::fs::write(&path, format!("{}|{}", self.tag, content))
                .map_err(|error| DagError::Schedule(format!("echo write failed: {error}")))?;
            let mut outputs = PortOutputs::new();
            outputs.insert_file(0, FileRef::local(&path, None)?);
            Ok(outputs)
        }

        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }

        fn kind(&self) -> &'static str {
            "echo"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// Pass-through executor running the node inline — the local executor
    /// minus resource admission, receipts and workspaces.
    struct InlineExecutor;

    #[derive(Clone)]
    struct ContractExecutor {
        specs: Arc<std::sync::Mutex<Vec<dag_core::TaskSpec>>>,
        resources: Arc<std::sync::Mutex<Vec<dag_core::TaskResources>>>,
        bind_count: Arc<std::sync::atomic::AtomicUsize>,
        preflights: Arc<std::sync::atomic::AtomicUsize>,
        bound: bool,
    }
    #[async_trait::async_trait]
    impl TaskExecutor for ContractExecutor {
        fn name(&self) -> &'static str {
            "contract-test"
        }
        async fn bind_run(&self) -> Result<Option<Arc<dyn TaskExecutor>>, DagError> {
            self.bind_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut bound = self.clone();
            bound.bound = true;
            Ok(Some(Arc::new(bound)))
        }
        fn execution_binding(&self) -> Option<serde_json::Value> {
            self.bound.then(|| serde_json::json!({"target":"test"}))
        }
        fn preflight(&self, nodes: &[(&str, &dyn DagNode)]) -> Result<(), DagError> {
            assert!(self.bound);
            self.preflights
                .fetch_add(nodes.len(), std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn run(&self, execution: TaskExecution) -> JobResult {
            assert!(self.bound);
            self.specs
                .lock()
                .unwrap()
                .push(execution.submission.spec.clone());
            self.resources
                .lock()
                .unwrap()
                .push(execution.submission.resources.clone());
            InlineExecutor.run(execution).await
        }
    }

    #[derive(Clone)]
    struct NamedPortNode {
        ports: dag_core::NodePorts,
    }
    #[async_trait::async_trait]
    impl DagNode for NamedPortNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            Ok(PortOutputs::new())
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "named_port"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn dataflow_submits_named_ports_and_channel_provenance_to_bound_executor() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "reads",
                vec![serde_json::json!("a"), serde_json::json!("b")],
            ))
            .source(Source::value("config", serde_json::json!(42)))
            .process(Process::new("worker", "named_port", serde_json::json!({})))
            .wire(Wire::collect("reads", 0, "worker", 0))
            .wire(Wire::direct("config", 0, "worker", 1))
            .build()
            .unwrap();
        let specs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let binds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let preflights = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let executor = ContractExecutor {
            specs: Arc::clone(&specs),
            resources: Arc::new(std::sync::Mutex::new(Vec::new())),
            bind_count: Arc::clone(&binds),
            preflights: Arc::clone(&preflights),
            bound: false,
        };
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(NamedPortNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type_with_label(
                        None,
                        dag_core::value::PortType::Channel,
                        "reads",
                    )
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel)
                    .add_output_port_of_type_with_label(
                        None,
                        dag_core::value::PortType::File,
                        "result",
                    ),
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(executor), builder, engine_ctx());
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok);
        assert_eq!(
            report.execution_binding,
            Some(serde_json::json!({"target":"test"}))
        );
        assert_eq!(binds.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(preflights.load(std::sync::atomic::Ordering::SeqCst), 1);
        let specs = specs.lock().unwrap();
        assert_eq!(specs.len(), 1);
        let task = &specs[0];
        assert_eq!(task.inputs[0].name, "reads");
        assert_eq!(
            task.inputs[0].source,
            dag_core::TaskInputSource::Channel {
                from: "reads".into(),
                from_port: 0,
                collected: true
            }
        );
        assert_eq!(task.inputs[1].name, "port_1");
        assert_eq!(
            task.inputs[1].source,
            dag_core::TaskInputSource::Channel {
                from: "config".into(),
                from_port: 0,
                collected: false
            }
        );
        assert_eq!(task.outputs[0].name, "result");
        let restored: dag_core::TaskSpec =
            serde_json::from_value(serde_json::to_value(task).unwrap()).unwrap();
        assert_eq!(restored.inputs[0].source, task.inputs[0].source);
        assert_eq!(restored.inputs[1].source, task.inputs[1].source);
    }

    #[tokio::test]
    async fn spec_declared_timeout_reaches_executor_resources() {
        let graph = FlowGraphBuilder::default()
            .source(Source::value("cfg", serde_json::json!(42)))
            .process(Process::new(
                "timed",
                "named_port",
                serde_json::json!({"timeout_ms": 1500}),
            ))
            .process(Process::new(
                "untimed",
                "named_port",
                serde_json::json!({"tag": "plain"}),
            ))
            .wire(Wire::direct("cfg", 0, "timed", 0))
            .wire(Wire::direct("cfg", 0, "untimed", 0))
            .build()
            .unwrap();
        let specs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let resources: Arc<std::sync::Mutex<Vec<dag_core::TaskResources>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let executor = ContractExecutor {
            specs: Arc::clone(&specs),
            resources: Arc::clone(&resources),
            bind_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            preflights: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            bound: false,
        };
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(NamedPortNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel)
                    .add_output_port_of_type(None, dag_core::value::PortType::File),
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(executor), builder, engine_ctx());
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok);
        let specs = specs.lock().unwrap();
        let resources = resources.lock().unwrap();
        assert_eq!(specs.len(), 2);
        for (spec, resources) in specs.iter().zip(resources.iter()) {
            match spec.logical_node.as_deref() {
                Some("timed") => assert_eq!(resources.max_duration_ms, Some(1500)),
                Some("untimed") => assert_eq!(resources.max_duration_ms, None),
                other => panic!("unexpected process {other:?}"),
            }
        }
    }

    #[derive(Clone)]
    struct BudgetedNode {
        meta: dag_core::NodePorts,
    }
    #[async_trait::async_trait]
    impl DagNode for BudgetedNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.meta
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            Ok(PortOutputs::new())
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "budgeted"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn task_resources(&self) -> Result<dag_core::TaskResources, DagError> {
            Ok(dag_core::TaskResources {
                cpus: Some(2),
                memory_bytes: None,
                max_duration_ms: Some(60_000),
            })
        }
    }

    #[test]
    fn declared_resources_spec_overrides_node_budget() {
        let node = BudgetedNode {
            meta: dag_core::NodePorts::new(),
        };
        // Spec absent: the node declaration passes through untouched.
        let inherited = declared_resources(&node, &serde_json::json!({}));
        assert_eq!(inherited.cpus, Some(2));
        assert_eq!(inherited.max_duration_ms, Some(60_000));
        // An explicit spec budget wins over the declaration.
        let overridden = declared_resources(&node, &serde_json::json!({"timeout_ms": 1500}));
        assert_eq!(overridden.cpus, Some(2));
        assert_eq!(overridden.max_duration_ms, Some(1500));
        // Non-numeric values (including string "null" from lax callers)
        // are ignored, never coerced.
        let nullish = declared_resources(&node, &serde_json::json!({"timeout_ms": null}));
        assert_eq!(nullish.max_duration_ms, Some(60_000));
        let textual = declared_resources(&node, &serde_json::json!({"timeout_ms": "1500"}));
        assert_eq!(textual.max_duration_ms, Some(60_000));
    }

    #[derive(Clone)]
    struct HangingNode {
        ports: dag_core::NodePorts,
    }
    #[async_trait::async_trait]
    impl DagNode for HangingNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            Ok(PortOutputs::new())
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "hanging"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// End to end through the real local executor: a spec-declared
    /// `timeout_ms` must kill a hung node instead of leasing it forever.
    #[tokio::test]
    async fn spec_timeout_kills_a_hanging_node_through_the_local_executor() {
        let flow = FlowGraphBuilder::default()
            .process(Process::new(
                "hang",
                "hanging",
                serde_json::json!({"timeout_ms": 150}),
            ))
            .build()
            .unwrap();
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(HangingNode {
                ports: dag_core::NodePorts::new(),
            }))
        });
        let workspace = tempfile::tempdir().unwrap();
        let scheduler = FlowScheduler::new(
            Arc::new(dag_core::LocalTaskExecutor::with_workspace_root(
                workspace.path(),
            )),
            builder,
            engine_ctx(),
        );
        let started = std::time::Instant::now();
        let report = scheduler
            .run_dataflow(&flow, &resolved(&flow), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(!report.ok, "{:?}", report.instances);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the run must return at the timeout, not after the node body"
        );
        let record = report
            .instances
            .iter()
            .find(|record| record.process == "hang")
            .expect("the hanging process materialized one instance");
        assert!(
            record
                .error
                .as_deref()
                .is_some_and(|error| error.contains("exceeded max_duration_ms")),
            "{:?}",
            record.error
        );
    }

    #[async_trait::async_trait]
    impl TaskExecutor for InlineExecutor {
        fn name(&self) -> &'static str {
            "inline-test"
        }

        async fn run(&self, mut execution: TaskExecution) -> JobResult {
            let started = std::time::Instant::now();
            match execution
                .node
                .execute(
                    &execution.engine_ctx,
                    &execution.submission.inputs,
                    &execution.reporter,
                )
                .await
            {
                Ok(outputs) => JobResult::Success {
                    id: execution.submission.spec.id,
                    outputs,
                    duration: started.elapsed(),
                    details: None,
                },
                Err(error) => JobResult::Failed {
                    id: execution.submission.spec.id,
                    error,
                    duration: started.elapsed(),
                    details: None,
                },
            }
        }
    }

    fn engine_ctx() -> Arc<NodeCtx> {
        Arc::new(NodeCtx::new(
            Arc::new(datafusion::execution::runtime_env::RuntimeEnv::default()),
            None,
        ))
    }

    fn scheduler() -> FlowScheduler {
        FlowScheduler::new(
            Arc::new(InlineExecutor),
            Arc::new(EchoNode::build),
            engine_ctx(),
        )
    }

    /// Builds the canonical reads -> qc -> align -> multiqc flow with
    /// globally unique source/process ids so parallel tests never share
    /// temp output files. The source drives qc; align consumes qc's items
    /// (NOT re-declared); multiqc collects.
    fn flow(fail_first_item: bool) -> FlowGraphBuilder {
        let tag = unique_tag("qc");
        let fail_on = fail_first_item.then(|| format!("{tag}:a"));
        FlowGraphBuilder::default()
            .source(Source::items(
                "reads",
                vec![serde_json::json!("a"), serde_json::json!("b")],
            ))
            .process(Process::new(
                "qc",
                "echo",
                serde_json::json!({ "tag": format!("{tag}:{{{{item}}}}"), "fail_on": fail_on }),
            ))
            .process(Process::new(
                "align",
                "echo",
                serde_json::json!({ "tag": tag }),
            ))
            .process(Process::new(
                "multiqc",
                "echo",
                serde_json::json!({ "tag": tag }),
            ))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::direct("qc", 0, "align", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
    }

    fn flow_with_strategy(
        fail_first_item: bool,
        qc_strategy: crate::model::ErrorStrategy,
    ) -> FlowGraphBuilder {
        let qc_tag = unique_tag("qc");
        let mut qc = Process::new(
            "qc",
            "echo",
            serde_json::json!({
                "tag": format!("{qc_tag}:{{{{item}}}}"),
                "fail_on": fail_first_item.then(|| format!("{qc_tag}:a")),
            }),
        );
        qc.error_strategy = qc_strategy;
        let tag = unique_tag("echo");
        FlowGraphBuilder::default()
            .source(Source::items(
                "reads",
                vec![serde_json::json!("a"), serde_json::json!("b")],
            ))
            .process(qc)
            .process(Process::new(
                "align",
                "echo",
                serde_json::json!({ "tag": tag }),
            ))
            .process(Process::new(
                "multiqc",
                "echo",
                serde_json::json!({ "tag": tag }),
            ))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::direct("qc", 0, "align", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
    }

    fn resolved(graph: &FlowGraph) -> SourceResolution {
        let mut resolution = SourceResolution::new();
        for source in graph.sources() {
            match &source.spec {
                crate::model::SourceSpec::Items { values } => {
                    resolution.resolve(source.id.clone(), values.clone())
                }
                crate::model::SourceSpec::Value { value } => {
                    resolution.resolve(source.id.clone(), vec![value.clone()])
                }
                crate::model::SourceSpec::Glob { .. }
                | crate::model::SourceSpec::FilePairs { .. } => {}
            }
        }
        resolution
    }

    fn content_of(record: &InstanceRecord) -> String {
        match record.outputs.get(&0) {
            Some(NodeValue::File(file)) => {
                std::fs::read_to_string(file.path.strip_prefix("vfs://").unwrap_or(&file.path))
                    .unwrap_or_else(|error| panic!("read {}: {error}", file.path))
            }
            other => panic!("expected a file output, got {other:?}"),
        }
    }

    fn executions() -> usize {
        EXECUTIONS.with(std::cell::Cell::get)
    }

    fn reset_executions() {
        EXECUTIONS.with(|counter| counter.set(0));
    }

    #[tokio::test]
    async fn pipeline_executes_with_grouped_report() {
        let graph = flow(false).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let report = scheduler()
            .run(
                &plan,
                &FlowRunConfig {
                    max_concurrency: 2,
                    resume: false,
                },
                None,
                None,
            )
            .await
            .unwrap();

        assert!(report.ok, "{:?}", report.processes);
        assert_eq!(report.processes["qc"].succeeded, 2);
        assert_eq!(report.processes["align"].succeeded, 2);
        assert_eq!(report.processes["multiqc"].succeeded, 1);

        // Inputs flowed per key: align#a received exactly qc#a's output.
        let align_a = report
            .instances
            .iter()
            .find(|record| record.id.as_str() == "align#a")
            .unwrap();
        let qc_a = content_of(
            report
                .instances
                .iter()
                .find(|record| record.id.as_str() == "qc#a")
                .unwrap(),
        );
        let align_a_content = content_of(align_a);
        // align#a consumed exactly qc#a's output (tag prefix + qc#a payload).
        assert!(
            align_a_content.ends_with(&qc_a),
            "align#a content `{align_a_content}` must end with qc#a output `{qc_a}`"
        );
    }

    #[tokio::test]
    async fn failure_skips_only_its_own_dependents() {
        let graph = flow(true).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let report = scheduler()
            .run(&plan, &FlowRunConfig::default(), None, None)
            .await
            .unwrap();

        assert!(!report.ok);
        assert_eq!(report.processes["qc"].succeeded, 1);
        assert_eq!(report.processes["qc"].failed, 1);
        // align#a skipped (its key-matched upstream failed); align#b ran.
        assert_eq!(report.processes["align"].skipped, 1);
        assert_eq!(report.processes["align"].succeeded, 1);
        // v1 is fail-closed: a failed item also skips its Collect consumer,
        // even when sibling items survived (Nextflow's default posture before
        // `errorStrategy 'ignore'`; a tolerant collect is a later knob).
        assert_eq!(report.processes["multiqc"].skipped, 1);
        assert_eq!(report.processes["multiqc"].succeeded, 0);
    }

    /// Never completes before cancellation: after the run-cancel fires it
    /// parks forever, so the abort path is observed deterministically
    /// instead of racing the (instant) inline executor.
    struct HangingExecutor;

    #[async_trait::async_trait]
    impl TaskExecutor for HangingExecutor {
        fn name(&self) -> &'static str {
            "hanging-test"
        }

        async fn run(&self, execution: TaskExecution) -> JobResult {
            execution.cancellation.cancelled().await;
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancellation_marks_everything_cancelled() {
        let graph = flow(false).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let hanging = FlowScheduler::new(
            Arc::new(HangingExecutor),
            Arc::new(EchoNode::build),
            engine_ctx(),
        );
        let token = CancellationToken::new();
        // Let the qc instances dispatch first, then abort.
        let cancel_handle = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            cancel_handle.cancel();
        });
        let report = hanging
            .run(&plan, &FlowRunConfig::default(), None, Some(token))
            .await
            .unwrap();
        assert!(!report.ok);
        assert_eq!(report.processes["qc"].cancelled, 2);
        assert_eq!(report.processes["align"].cancelled, 2);
        assert_eq!(report.processes["multiqc"].cancelled, 1);
    }

    #[tokio::test]
    async fn broken_kind_fails_before_dispatch() {
        let graph = flow(false).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let broken = FlowScheduler::new(
            Arc::new(InlineExecutor),
            Arc::new(|kind, _| {
                Err(crate::model::Error::Invalid(format!("unknown kind `{kind}`")).into())
            }),
            engine_ctx(),
        );
        let error = broken
            .run(&plan, &FlowRunConfig::default(), None, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown kind"), "{error}");
    }

    #[tokio::test]
    async fn resume_reuses_every_unchanged_instance() {
        reset_executions();
        let graph = flow(false).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let mut cache = InstanceCache::new();
        let cfg = FlowRunConfig {
            resume: true,
            ..FlowRunConfig::default()
        };

        let before = executions();
        let first = scheduler()
            .run(&plan, &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        let first_executions = executions() - before;
        assert_eq!(first_executions, 5, "qc x2 + align x2 + multiqc");
        assert_eq!(first.processes["qc"].reused, 0);
        assert_eq!(cache.len(), 5);

        let before = executions();
        let second = scheduler()
            .run(&plan, &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(
            executions() - before,
            0,
            "a fingerprint hit must never reach a node"
        );
        assert!(second.ok);
        assert_eq!(second.processes["qc"].reused, 2);
        assert_eq!(second.processes["align"].reused, 2);
        assert_eq!(second.processes["multiqc"].reused, 1);
        assert_eq!(second.processes["qc"].succeeded, 2);

        // Reused outputs are byte-identical to the first run's.
        for id in ["qc#a", "align#b", "multiqc#0"] {
            let first_content = content_of(
                first
                    .instances
                    .iter()
                    .find(|r| r.id.as_str() == id)
                    .unwrap(),
            );
            let second_content = content_of(
                second
                    .instances
                    .iter()
                    .find(|r| r.id.as_str() == id)
                    .unwrap(),
            );
            assert_eq!(first_content, second_content, "{id}");
        }
    }

    #[tokio::test]
    async fn spec_edit_invalidates_the_whole_downstream_chain() {
        reset_executions();
        let graph = flow(false).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let mut cache = InstanceCache::new();
        let cfg = FlowRunConfig {
            resume: true,
            ..FlowRunConfig::default()
        };
        scheduler()
            .run(&plan, &cfg, Some(&mut cache), None)
            .await
            .unwrap();

        // Rebuild the flow with an edited qc spec template.
        let mut edited = flow(false);
        edited = {
            let rebuilt = edited.build().unwrap();
            // Rebuild via builder mutation is not exposed; construct a fresh
            // graph by cloning processes and editing the qc spec directly.
            let sources: Vec<Source> = rebuilt.sources().to_vec();
            let mut processes: Vec<Process> = rebuilt.processes().to_vec();
            for process in &mut processes {
                if process.id == "qc" {
                    process.spec["extra"] = serde_json::json!("edited");
                }
            }
            let wires: Vec<Wire> = rebuilt.wires().to_vec();
            let mut builder = FlowGraphBuilder::default();
            for source in sources {
                builder = builder.source(source);
            }
            for process in processes {
                builder = builder.process(process);
            }
            for wire in wires {
                builder = builder.wire(wire);
            }
            builder
        };
        let edited_graph = edited.build().unwrap();
        let edited_plan = build_plan(&edited_graph, &resolved(&edited_graph)).unwrap();

        let before = executions();
        let report = scheduler()
            .run(&edited_plan, &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        // A qc template edit invalidates qc (spec), align (upstream fp) and
        // multiqc (upstream fp) — the full chain re-executes.
        assert_eq!(executions() - before, 5);
        assert_eq!(report.processes["qc"].reused, 0);
        assert_eq!(report.processes["align"].reused, 0);
        assert_eq!(report.processes["multiqc"].reused, 0);
    }

    #[tokio::test]
    async fn deleted_output_file_reexecutes_only_that_instance() {
        reset_executions();
        let graph = flow(false).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let mut cache = InstanceCache::new();
        let cfg = FlowRunConfig {
            resume: true,
            ..FlowRunConfig::default()
        };
        let first = scheduler()
            .run(&plan, &cfg, Some(&mut cache), None)
            .await
            .unwrap();

        // Delete one align instance's output file on disk.
        let align_a = first
            .instances
            .iter()
            .find(|record| record.id.as_str() == "align#a")
            .unwrap();
        let path = match align_a.outputs.get(&0) {
            Some(NodeValue::File(file)) => file.path.clone(),
            other => panic!("expected a file output, got {other:?}"),
        };
        std::fs::remove_file(&path).unwrap();

        let before = executions();
        let second = scheduler()
            .run(&plan, &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        // Only align#a re-executed: its cached file vanished. Everything
        // else — including multiqc, whose upstream identities are unchanged —
        // is served from the cache.
        assert_eq!(executions() - before, 1);
        assert_eq!(second.processes["qc"].reused, 2);
        assert_eq!(second.processes["align"].reused, 1);
        assert_eq!(second.processes["align"].succeeded, 2);
        assert_eq!(second.processes["multiqc"].reused, 1);
        assert!(second.ok);
    }

    #[tokio::test]
    async fn ignore_tolerates_item_failures_and_collects_survivors() {
        let graph = flow_with_strategy(true, crate::model::ErrorStrategy::ignore())
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let report = scheduler()
            .run(&plan, &FlowRunConfig::default(), None, None)
            .await
            .unwrap();

        // The failed item is recorded but tolerated; the run stays ok.
        assert!(report.ok, "{:?}", report.processes);
        assert_eq!(report.processes["qc"].succeeded, 1);
        assert_eq!(report.processes["qc"].failed, 1);
        assert_eq!(report.processes["qc"].ignored, 1);
        // Direct key-matched dependent skipped (item never delivered).
        assert_eq!(report.processes["align"].skipped, 1);
        assert_eq!(report.processes["align"].succeeded, 1);
        // The Collect consumer ran on the surviving binding.
        assert_eq!(report.processes["multiqc"].succeeded, 1);
        let multiqc = report
            .instances
            .iter()
            .find(|record| record.id.as_str() == "multiqc#0")
            .unwrap();
        let qc_b = report
            .instances
            .iter()
            .find(|record| record.id.as_str() == "qc#b")
            .unwrap();
        // Exactly one input reached the collector.
        assert!(content_of(multiqc).ends_with(&content_of(qc_b)));
    }

    #[tokio::test]
    async fn retry_exhaustion_degrades_to_fail_closed() {
        let graph = flow_with_strategy(true, crate::model::ErrorStrategy::retry(2, 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        reset_executions();
        let report = scheduler()
            .run(&plan, &FlowRunConfig::default(), None, None)
            .await
            .unwrap();

        assert!(!report.ok);
        let qc_a = report
            .instances
            .iter()
            .find(|record| record.id.as_str() == "qc#a")
            .unwrap();
        assert_eq!(qc_a.attempts, 3, "1 initial + 2 retries");
        assert_eq!(qc_a.status, RuntimeStatus::Failed);
        // Exhausted retry is fatal: dependents skip, collect consumer skips.
        assert_eq!(report.processes["align"].skipped, 1);
        assert_eq!(report.processes["multiqc"].skipped, 1);
        // 3 attempts on qc#a + 1 on qc#b + 1 on align#b.
        assert_eq!(executions(), 5);
    }

    #[tokio::test]
    async fn retry_recovers_from_transient_failures() {
        // qc#a fails once (transient budget), succeeds on the second attempt.
        let qc_tag = unique_tag("qc");
        let tag = unique_tag("echo");
        let mut qc = Process::new(
            "qc",
            "echo",
            serde_json::json!({ "tag": format!("{}:{{{{item}}}}", qc_tag), "fail_times": 1 }),
        );
        qc.error_strategy = crate::model::ErrorStrategy::retry(2, 0);
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "reads",
                vec![serde_json::json!("a"), serde_json::json!("b")],
            ))
            .process(qc)
            .process(Process::new(
                "align",
                "echo",
                serde_json::json!({ "tag": tag }),
            ))
            .process(Process::new(
                "multiqc",
                "echo",
                serde_json::json!({ "tag": tag }),
            ))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::direct("qc", 0, "align", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        reset_executions();
        let report = scheduler()
            .run(&plan, &FlowRunConfig::default(), None, None)
            .await
            .unwrap();

        assert!(report.ok, "{:?}", report.processes);
        let qc_a = report
            .instances
            .iter()
            .find(|record| record.id.as_str() == "qc#a")
            .unwrap();
        assert_eq!(qc_a.attempts, 2, "failed once, succeeded on retry");
        assert_eq!(qc_a.status, RuntimeStatus::Success);
        assert_eq!(report.processes["align"].succeeded, 2);
        assert_eq!(report.processes["multiqc"].succeeded, 1);
        assert_eq!(
            executions(),
            7,
            "2 qc#a + 1 qc#b + 2 align + 1 multiqc + 1 (qc#a first fail)"
        );
    }

    #[tokio::test]
    async fn delayed_retry_uses_the_backoff_channel() {
        let qc_tag = unique_tag("qc");
        let tag = unique_tag("echo");
        let mut qc = Process::new(
            "qc",
            "echo",
            serde_json::json!({ "tag": qc_tag, "fail_times": 1 }),
        );
        qc.error_strategy = crate::model::ErrorStrategy::retry(1, 20);
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![serde_json::json!("a")]))
            .process(qc)
            .process(Process::new(
                "multiqc",
                "echo",
                serde_json::json!({ "tag": tag }),
            ))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let started = std::time::Instant::now();
        let report = scheduler()
            .run(&plan, &FlowRunConfig::default(), None, None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.processes);
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(20),
            "the 20ms backoff must actually elapse"
        );
        assert_eq!(report.processes["multiqc"].succeeded, 1);
    }

    #[tokio::test]
    async fn empty_source_run_is_ok() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![]))
            .process(Process::new(
                "qc",
                "echo",
                serde_json::json!({ "tag": "qc" }),
            ))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let report = scheduler()
            .run(&plan, &FlowRunConfig::default(), None, None)
            .await
            .unwrap();
        assert!(report.ok);
        assert_eq!(report.processes["qc"].total, 0);
    }
    #[tokio::test]
    async fn dataflow_dispatches_actual_output_to_downstream() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "reads",
                vec![serde_json::json!("a"), serde_json::json!("b")],
            ))
            .process(Process::new(
                "qc",
                "echo",
                serde_json::json!({"tag":"qc-{{item}}"}),
            ))
            .process(Process::new(
                "next",
                "echo",
                serde_json::json!({"tag":"next"}),
            ))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::direct("qc", 0, "next", 0))
            .build()
            .unwrap();
        let report = scheduler()
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok);
        assert_eq!(report.processes["qc"].succeeded, 2);
        assert_eq!(report.processes["next"].succeeded, 2);
        assert!(report.instances.iter().filter(|r| r.process == "next").all(
            |r| match &r.outputs[&0] {
                NodeValue::File(f) => f.path.contains("qc-"),
                _ => false,
            }
        ));
    }

    #[tokio::test]
    async fn dataflow_empty_queue_does_not_trigger_collect() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![]))
            .process(Process::new("qc", "echo", serde_json::json!({})))
            .process(Process::new("sink", "echo", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::collect("qc", 0, "sink", 0))
            .build()
            .unwrap();
        let report = scheduler()
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok);
        assert_eq!(report.processes["qc"].total, 0);
        assert_eq!(report.processes["sink"].total, 0);
    }

    #[derive(Clone)]
    struct StreamingTestNode {
        ports: dag_core::NodePorts,
        producer: bool,
        consumed: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl DagNode for StreamingTestNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            let mut out = PortOutputs::new();
            if self.producer {
                reporter
                    .emit_channel_item(0, 0, serde_json::json!("streamed-file"))
                    .await;
                tokio::time::timeout(std::time::Duration::from_secs(2), self.consumed.notified())
                    .await
                    .map_err(|_| {
                        DagError::Schedule(
                            "consumer did not start while producer was running".into(),
                        )
                    })?;
                reporter.close_channel(0, 1).await;
                out.insert(
                    0,
                    NodeValue::Channel(dag_core::value::ChannelValue {
                        items: vec![serde_json::json!("streamed-file")],
                    }),
                );
            } else {
                self.consumed.notify_one();
            }
            Ok(out)
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "stream_test"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn dataflow_streamed_item_starts_consumer_before_producer_finishes() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("one")]))
            .process(Process::new(
                "producer",
                "stream_test",
                serde_json::json!({"producer":true}),
            ))
            .process(Process::new(
                "consumer",
                "stream_test",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "producer", 0))
            .wire(Wire::direct("producer", 0, "consumer", 0))
            .build()
            .unwrap();
        let consumed = Arc::new(tokio::sync::Notify::new());
        let builder: NodeBuilder = Arc::new(move |_, spec| {
            Ok(Box::new(StreamingTestNode {
                ports: dag_core::NodePorts::new(),
                producer: spec["producer"].as_bool().unwrap_or(false),
                consumed: Arc::clone(&consumed),
            }))
        });
        let sched = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let cfg = FlowRunConfig {
            max_concurrency: 2,
            resume: false,
        };
        let report = sched
            .run_dataflow(&graph, &resolved(&graph), &cfg, None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["producer"].succeeded, 1);
        assert_eq!(report.processes["consumer"].succeeded, 1);
    }

    #[derive(Clone)]
    struct BackpressureNode {
        ports: dag_core::NodePorts,
        producer: bool,
        sent: Arc<std::sync::atomic::AtomicUsize>,
        consumer_started: Arc<tokio::sync::Notify>,
        gate: Arc<tokio::sync::Semaphore>,
    }
    #[async_trait::async_trait]
    impl DagNode for BackpressureNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            if self.producer {
                for index in 0..500 {
                    reporter
                        .emit_channel_item(0, index, serde_json::json!(index))
                        .await;
                    self.sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                reporter.close_channel(0, 500).await;
            } else {
                self.consumer_started.notify_one();
                self.gate
                    .acquire()
                    .await
                    .map_err(|e| DagError::Schedule(e.to_string()))?
                    .forget();
            }
            Ok(PortOutputs::new())
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "backpressure_test"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn dataflow_applies_backpressure_to_streaming_task_outputs() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("start")]))
            .process(Process::new(
                "producer",
                "backpressure_test",
                serde_json::json!({"producer":true}),
            ))
            .process(Process::new(
                "consumer",
                "backpressure_test",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "producer", 0))
            .wire(Wire::direct("producer", 0, "consumer", 0))
            .build()
            .unwrap();
        let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let builder: NodeBuilder = {
            let sent = Arc::clone(&sent);
            let started = Arc::clone(&started);
            let gate = Arc::clone(&gate);
            Arc::new(move |_, spec| {
                Ok(Box::new(BackpressureNode {
                    ports: dag_core::NodePorts::new(),
                    producer: spec["producer"].as_bool().unwrap_or(false),
                    sent: Arc::clone(&sent),
                    consumer_started: Arc::clone(&started),
                    gate: Arc::clone(&gate),
                }))
            })
        };
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let run = tokio::spawn(async move {
            let config = FlowRunConfig {
                max_concurrency: 2,
                resume: false,
            };
            scheduler
                .run_dataflow(&graph, &resolved(&graph), &config, None)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            sent.load(std::sync::atomic::Ordering::SeqCst) < 500,
            "producer must wait for the slow consumer"
        );
        gate.add_permits(500);
        let report = tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["consumer"].succeeded, 500);
    }

    #[tokio::test]
    async fn first_function_stops_open_source_after_match() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new("first", "channel", serde_json::json!({"operator":"first", "criteria":{"__dag_shell_function__":{"source":"def f(x): return x", "captures":{}}}})))
            .wire(Wire::direct("s", 0, "first", 0)).build().unwrap();
        let (sender, reader) = tokio::sync::mpsc::channel(2);
        sender.send(Ok(serde_json::json!(1))).await.unwrap();
        sender.send(Ok(serde_json::json!(2))).await.unwrap();
        let streams = crate::source::SourceStreams::from_test_reader("s", reader);
        let builder: NodeBuilder = Arc::new(|_, _| panic!("channel-only graph"));
        let evaluator: ChannelFunction = Arc::new(|_, item| {
            Ok(channel_value(serde_json::json!(
                matches!(item, Some(NodeValue::Channel(channel)) if channel.items == vec![serde_json::json!(2)])
            )))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
            .with_channel_function(evaluator);
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            scheduler.run_dataflow_streamed_cached(
                &graph,
                streams,
                &FlowRunConfig::default(),
                None,
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert!(sender.is_closed());
    }

    #[tokio::test]
    async fn until_emits_prior_items_and_detaches_an_open_source() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new("until", "channel", serde_json::json!({"operator":"until", "function":{"__dag_shell_function__":{"source":"def f(x): return x == 2", "captures":{}}}})))
            .process(Process::new("sink", "inspect_list", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "until", 0)).wire(Wire::direct("until", 0, "sink", 0)).build().unwrap();
        let (sender, reader) = tokio::sync::mpsc::channel(4);
        for n in 1..=3 {
            sender.send(Ok(serde_json::json!(n))).await.unwrap();
        }
        let streams = crate::source::SourceStreams::from_test_reader("s", reader);
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(ListInspectNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                expected: serde_json::json!(1),
            }))
        });
        let evaluator: ChannelFunction = Arc::new(|_, input| {
            Ok(channel_value(serde_json::json!(
                matches!(input, Some(NodeValue::Channel(channel)) if channel.items == vec![serde_json::json!(2)])
            )))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
            .with_channel_function(evaluator);
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            scheduler.run_dataflow_streamed_cached(
                &graph,
                streams,
                &FlowRunConfig::default(),
                None,
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].succeeded, 1);
        assert!(sender.is_closed());
    }

    #[tokio::test]
    async fn first_matching_item_detaches_an_open_source() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new(
                "first",
                "channel",
                serde_json::json!({"operator":"first", "criteria":2}),
            ))
            .wire(Wire::direct("s", 0, "first", 0))
            .build()
            .unwrap();
        let (sender, reader) = tokio::sync::mpsc::channel(2);
        sender.send(Ok(serde_json::json!(1))).await.unwrap();
        sender.send(Ok(serde_json::json!(2))).await.unwrap();
        let streams = crate::source::SourceStreams::from_test_reader("s", reader);
        let builder: NodeBuilder = Arc::new(|_, _| panic!("first is a channel operator"));
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            scheduler.run_dataflow_streamed_cached(
                &graph,
                streams,
                &FlowRunConfig::default(),
                None,
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert!(sender.is_closed());
    }

    #[tokio::test]
    async fn take_stops_map_and_detaches_an_open_source() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new(
                "map",
                "channel",
                serde_json::json!({"operator":"map", "template":"{{item}}"}),
            ))
            .process(Process::new(
                "take",
                "channel",
                serde_json::json!({"operator":"take", "count":1}),
            ))
            .wire(Wire::direct("s", 0, "map", 0))
            .wire(Wire::direct("map", 0, "take", 0))
            .build()
            .unwrap();
        let (sender, reader) = tokio::sync::mpsc::channel(1);
        sender.send(Ok(serde_json::json!("first"))).await.unwrap();
        let streams = crate::source::SourceStreams::from_test_reader("s", reader);
        let builder: NodeBuilder = Arc::new(|_, spec| {
            dag_core::ChannelNode::from_spec(&spec)
                .map(|node| Box::new(node) as Box<dyn DagNode>)
                .map_err(|error| crate::Error::Execution(error.to_string()))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            scheduler.run_dataflow_streamed_cached(
                &graph,
                streams,
                &FlowRunConfig::default(),
                None,
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert!(sender.is_closed());
    }

    #[tokio::test]
    async fn take_stops_transpose_and_detaches_an_open_source() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new(
                "transpose",
                "channel",
                serde_json::json!({"operator":"transpose"}),
            ))
            .process(Process::new(
                "take",
                "channel",
                serde_json::json!({"operator":"take", "count":1}),
            ))
            .wire(Wire::direct("s", 0, "transpose", 0))
            .wire(Wire::direct("transpose", 0, "take", 0))
            .build()
            .unwrap();
        let (sender, reader) = tokio::sync::mpsc::channel(1);
        sender
            .send(Ok(serde_json::json!([
                "sample",
                (0..500).collect::<Vec<_>>()
            ])))
            .await
            .unwrap();
        let streams = crate::source::SourceStreams::from_test_reader("s", reader);
        let builder: NodeBuilder = Arc::new(|_, _| panic!("all stages are channel operators"));
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let config = FlowRunConfig {
            max_concurrency: 2,
            resume: false,
        };
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            scheduler.run_dataflow_streamed_cached(&graph, streams, &config, None, None),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(report.ok);
        assert!(sender.is_closed());
    }

    #[tokio::test]
    async fn group_finalization_resumes_after_slow_downstream() {
        let values: Vec<_> = (0..500).map(|n| serde_json::json!([n, n])).collect();
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", values))
            .process(Process::new(
                "group",
                "channel",
                serde_json::json!({"operator":"group_tuple"}),
            ))
            .process(Process::new(
                "consumer",
                "backpressure_test",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "group", 0))
            .wire(Wire::direct("group", 0, "consumer", 0))
            .build()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let builder: NodeBuilder = {
            let started = Arc::clone(&started);
            let gate = Arc::clone(&gate);
            Arc::new(move |_, _| {
                Ok(Box::new(BackpressureNode {
                    ports: dag_core::NodePorts::new(),
                    producer: false,
                    sent: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    consumer_started: Arc::clone(&started),
                    gate: Arc::clone(&gate),
                }))
            })
        };
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let run = tokio::spawn(async move {
            scheduler
                .run_dataflow(
                    &graph,
                    &resolved(&graph),
                    &FlowRunConfig {
                        max_concurrency: 2,
                        resume: false,
                    },
                    None,
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        gate.add_permits(500);
        let report = tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["consumer"].succeeded, 500);
    }

    #[tokio::test]
    async fn transpose_resumes_after_slow_downstream_and_closes() {
        let values: Vec<_> = (0..500).collect();
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "s",
                vec![serde_json::json!(["sample", values])],
            ))
            .process(Process::new(
                "transpose",
                "channel",
                serde_json::json!({"operator":"transpose"}),
            ))
            .process(Process::new(
                "consumer",
                "backpressure_test",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "transpose", 0))
            .wire(Wire::direct("transpose", 0, "consumer", 0))
            .build()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let builder: NodeBuilder = {
            let started = Arc::clone(&started);
            let gate = Arc::clone(&gate);
            Arc::new(move |_, _| {
                Ok(Box::new(BackpressureNode {
                    ports: dag_core::NodePorts::new(),
                    producer: false,
                    sent: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    consumer_started: Arc::clone(&started),
                    gate: Arc::clone(&gate),
                }))
            })
        };
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let run = tokio::spawn(async move {
            scheduler
                .run_dataflow(
                    &graph,
                    &resolved(&graph),
                    &FlowRunConfig {
                        max_concurrency: 2,
                        resume: false,
                    },
                    None,
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        gate.add_permits(500);
        let report = tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["consumer"].succeeded, 500);
    }

    #[tokio::test]
    async fn streaming_producer_completes_when_concurrency_is_one() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("start")]))
            .process(Process::new(
                "producer",
                "backpressure_test",
                serde_json::json!({"producer":true}),
            ))
            .process(Process::new(
                "consumer",
                "backpressure_test",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "producer", 0))
            .wire(Wire::direct("producer", 0, "consumer", 0))
            .build()
            .unwrap();
        let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(tokio::sync::Semaphore::new(500));
        let builder: NodeBuilder = {
            let sent = Arc::clone(&sent);
            Arc::new(move |_, spec| {
                Ok(Box::new(BackpressureNode {
                    ports: dag_core::NodePorts::new(),
                    producer: spec["producer"].as_bool().unwrap_or(false),
                    sent: Arc::clone(&sent),
                    consumer_started: Arc::clone(&started),
                    gate: Arc::clone(&gate),
                }))
            })
        };
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let config = FlowRunConfig {
            max_concurrency: 1,
            resume: false,
        };
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            scheduler.run_dataflow(&graph, &resolved(&graph), &config, None),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 500);
        assert_eq!(report.processes["consumer"].succeeded, 500);
    }

    #[derive(Clone)]
    struct TwoPortStreamNode {
        ports: dag_core::NodePorts,
    }
    #[async_trait::async_trait]
    impl DagNode for TwoPortStreamNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            for port in [0, 1] {
                for index in 0..300 {
                    reporter
                        .emit_channel_item(port, index, serde_json::json!(index))
                        .await;
                }
                reporter.close_channel(port, 300).await;
            }
            Ok(PortOutputs::new())
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "two_port_stream"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn backpressure_does_not_deadlock_sequential_outputs_joined_by_one_consumer() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("start")]))
            .process(Process::new(
                "producer",
                "two_port_stream",
                serde_json::json!({}),
            ))
            .process(Process::new(
                "consumer",
                "backpressure_test",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "producer", 0))
            .wire(Wire::direct("producer", 0, "consumer", 0))
            .wire(Wire::direct("producer", 1, "consumer", 1))
            .build()
            .unwrap();
        let gate = Arc::new(tokio::sync::Semaphore::new(300));
        let builder: NodeBuilder = Arc::new(move |kind, _| {
            if kind == "two_port_stream" {
                return Ok(Box::new(TwoPortStreamNode {
                    ports: dag_core::NodePorts::new(),
                }));
            }
            Ok(Box::new(BackpressureNode {
                ports: dag_core::NodePorts::new(),
                producer: false,
                sent: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                consumer_started: Arc::new(tokio::sync::Notify::new()),
                gate: Arc::clone(&gate),
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let cfg = FlowRunConfig {
            max_concurrency: 2,
            resume: false,
        };
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            scheduler.run_dataflow(&graph, &resolved(&graph), &cfg, None),
        )
        .await
        .expect("outputs needed for an input tuple must not block each other")
        .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["consumer"].succeeded, 300);
    }

    #[tokio::test]
    async fn blocked_stream_does_not_block_an_independent_producer() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("start")]))
            .process(Process::new(
                "slow_producer",
                "backpressure_test",
                serde_json::json!({"producer":true}),
            ))
            .process(Process::new(
                "slow_consumer",
                "backpressure_test",
                serde_json::json!({}),
            ))
            .process(Process::new(
                "fast_producer",
                "two_port_stream",
                serde_json::json!({}),
            ))
            .process(Process::new(
                "fast_consumer",
                "backpressure_test",
                serde_json::json!({"fast":true}),
            ))
            .wire(Wire::direct("s", 0, "slow_producer", 0))
            .wire(Wire::direct("s", 0, "fast_producer", 0))
            .wire(Wire::direct("slow_producer", 0, "slow_consumer", 0))
            .wire(Wire::direct("fast_producer", 0, "fast_consumer", 0))
            .build()
            .unwrap();
        let slow_gate = Arc::new(tokio::sync::Semaphore::new(0));
        let fast_gate = Arc::new(tokio::sync::Semaphore::new(300));
        let fast_started = Arc::new(tokio::sync::Notify::new());
        let builder: NodeBuilder = {
            let slow_gate = Arc::clone(&slow_gate);
            let fast_started = Arc::clone(&fast_started);
            Arc::new(move |kind, spec| {
                if kind == "two_port_stream" {
                    return Ok(Box::new(TwoPortStreamNode {
                        ports: dag_core::NodePorts::new(),
                    }));
                }
                let fast = spec["fast"].as_bool().unwrap_or(false);
                Ok(Box::new(BackpressureNode {
                    ports: dag_core::NodePorts::new(),
                    producer: spec["producer"].as_bool().unwrap_or(false),
                    sent: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    consumer_started: if fast {
                        Arc::clone(&fast_started)
                    } else {
                        Arc::new(tokio::sync::Notify::new())
                    },
                    gate: if fast {
                        Arc::clone(&fast_gate)
                    } else {
                        Arc::clone(&slow_gate)
                    },
                }))
            })
        };
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let run = tokio::spawn(async move {
            let config = FlowRunConfig {
                max_concurrency: 4,
                resume: false,
            };
            scheduler
                .run_dataflow(&graph, &resolved(&graph), &config, None)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), fast_started.notified())
            .await
            .expect("independent producer must progress while another stream is blocked");
        slow_gate.add_permits(500);
        let report = tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["slow_consumer"].succeeded, 500);
        assert_eq!(report.processes["fast_consumer"].succeeded, 300);
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_backpressured_producer() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("start")]))
            .process(Process::new(
                "producer",
                "backpressure_test",
                serde_json::json!({"producer":true}),
            ))
            .process(Process::new(
                "consumer",
                "backpressure_test",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "producer", 0))
            .wire(Wire::direct("producer", 0, "consumer", 0))
            .build()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let builder: NodeBuilder = {
            let started = Arc::clone(&started);
            let sent = Arc::clone(&sent);
            Arc::new(move |_, spec| {
                Ok(Box::new(BackpressureNode {
                    ports: dag_core::NodePorts::new(),
                    producer: spec["producer"].as_bool().unwrap_or(false),
                    sent: Arc::clone(&sent),
                    consumer_started: Arc::clone(&started),
                    gate: Arc::new(tokio::sync::Semaphore::new(0)),
                }))
            })
        };
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let token = CancellationToken::new();
        let run_token = token.clone();
        let run = tokio::spawn(async move {
            let config = FlowRunConfig {
                max_concurrency: 2,
                resume: false,
            };
            scheduler
                .run_dataflow(&graph, &resolved(&graph), &config, Some(run_token))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(sent.load(std::sync::atomic::Ordering::SeqCst) < 500);
        token.cancel();
        let report = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancellation must not wait for downstream capacity")
            .unwrap()
            .unwrap();
        assert!(!report.ok);
    }

    #[tokio::test]
    async fn dataflow_runs_tasks_before_source_enumeration_finishes() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new(
                "consumer",
                "stream_test",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "consumer", 0))
            .build()
            .unwrap();
        let consumed = Arc::new(tokio::sync::Notify::new());
        let notify = Arc::clone(&consumed);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let producer = tokio::spawn(async move {
            tx.send(Ok(serde_json::json!("first"))).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), notify.notified())
                .await
                .expect("consumer must run before the source finishes");
            tx.send(Ok(serde_json::json!("second"))).await.unwrap();
        });
        let builder: NodeBuilder = Arc::new(move |_, _| {
            Ok(Box::new(StreamingTestNode {
                ports: dag_core::NodePorts::new(),
                producer: false,
                consumed: Arc::clone(&consumed),
            }))
        });
        let sched = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let streams = crate::source::SourceStreams::from_test_reader("s", rx);
        let report = sched
            .run_dataflow_streamed_cached(&graph, streams, &FlowRunConfig::default(), None, None)
            .await
            .unwrap();
        producer.await.unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["consumer"].succeeded, 2);
    }

    #[tokio::test]
    async fn dataflow_reports_source_errors() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new("consumer", "echo", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "consumer", 0))
            .build()
            .unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(Err(crate::Error::Execution("source failed".into())))
            .await
            .unwrap();
        drop(tx);
        let streams = crate::source::SourceStreams::from_test_reader("s", rx);
        let error = scheduler()
            .run_dataflow_streamed_cached(&graph, streams, &FlowRunConfig::default(), None, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("source failed"), "{error}");
    }

    struct CancellationObservingExecutor {
        started: Arc<tokio::sync::Notify>,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    }

    struct DelayedRemoteCancellation {
        started: Arc<tokio::sync::Notify>,
    }

    struct UnconfirmedRemoteStop(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl TaskExecutor for UnconfirmedRemoteStop {
        fn name(&self) -> &'static str {
            "unconfirmed-remote-stop"
        }
        async fn run(&self, execution: TaskExecution) -> JobResult {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            JobResult::Failed {
                id: execution.submission.spec.id,
                error: DagError::Schedule("remote_stop_unconfirmed: synthetic lease".into()),
                duration: std::time::Duration::ZERO,
                details: None,
            }
        }
    }

    #[tokio::test]
    async fn unconfirmed_remote_stop_is_never_retried_or_ignored() {
        for strategy in [
            crate::model::ErrorStrategy::retry(2, 0),
            crate::model::ErrorStrategy::ignore(),
        ] {
            for reactive in [false, true] {
                let mut process = Process::new("worker", "echo", serde_json::json!({}));
                process.error_strategy = strategy.clone();
                let graph = FlowGraphBuilder::default()
                    .source(Source::items("s", vec![serde_json::json!("a")]))
                    .process(process)
                    .wire(Wire::direct("s", 0, "worker", 0))
                    .build()
                    .unwrap();
                let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let scheduler = FlowScheduler::new(
                    Arc::new(UnconfirmedRemoteStop(attempts.clone())),
                    Arc::new(EchoNode::build),
                    engine_ctx(),
                );
                let report = if reactive {
                    scheduler
                        .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
                        .await
                        .unwrap()
                } else {
                    let plan = build_plan(&graph, &resolved(&graph)).unwrap();
                    scheduler
                        .run(&plan, &FlowRunConfig::default(), None, None)
                        .await
                        .unwrap()
                };
                assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
                assert!(!report.ok);
                assert_eq!(report.processes["worker"].ignored, 0);
                assert_eq!(report.processes["worker"].failed, 1);
            }
        }
    }

    #[async_trait::async_trait]
    impl TaskExecutor for DelayedRemoteCancellation {
        fn name(&self) -> &'static str {
            "delayed-remote-cancellation"
        }
        fn requires_cancel_join(&self) -> bool {
            true
        }
        async fn run(&self, execution: TaskExecution) -> JobResult {
            self.started.notify_one();
            execution.cancellation.cancelled().await;
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            JobResult::Failed {
                id: execution.submission.spec.id,
                error: DagError::Schedule("remote_stop_unconfirmed: fake remote job".into()),
                duration: std::time::Duration::from_millis(1200),
                details: None,
            }
        }
    }

    #[tokio::test]
    async fn cancellation_waits_for_remote_stop_and_reports_uncertainty() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("a")]))
            .process(Process::new("worker", "echo", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "worker", 0))
            .build()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let scheduler = FlowScheduler::new(
            Arc::new(DelayedRemoteCancellation {
                started: started.clone(),
            }),
            Arc::new(EchoNode::build),
            engine_ctx(),
        );
        let token = CancellationToken::new();
        let run_token = token.clone();
        let run = tokio::spawn(async move {
            scheduler
                .run_dataflow(
                    &graph,
                    &resolved(&graph),
                    &FlowRunConfig::default(),
                    Some(run_token),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        token.cancel();
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(report.processes["worker"].failed, 1);
        assert!(
            report.instances[0]
                .error
                .as_ref()
                .unwrap()
                .contains("remote_stop_unconfirmed")
        );
    }

    #[tokio::test]
    async fn plan_cancellation_waits_for_remote_stop_and_reports_uncertainty() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("a")]))
            .process(Process::new("worker", "echo", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "worker", 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let scheduler = FlowScheduler::new(
            Arc::new(DelayedRemoteCancellation {
                started: started.clone(),
            }),
            Arc::new(EchoNode::build),
            engine_ctx(),
        );
        let token = CancellationToken::new();
        let run_token = token.clone();
        let run = tokio::spawn(async move {
            scheduler
                .run(&plan, &FlowRunConfig::default(), None, Some(run_token))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        token.cancel();
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(report.processes["worker"].failed, 1);
        assert!(
            report.instances[0]
                .error
                .as_ref()
                .unwrap()
                .contains("remote_stop_unconfirmed")
        );
    }
    #[async_trait::async_trait]
    impl TaskExecutor for CancellationObservingExecutor {
        fn name(&self) -> &'static str {
            "cancel-observer"
        }
        async fn run(&self, execution: TaskExecution) -> JobResult {
            self.started.notify_one();
            execution.cancellation.cancelled().await;
            self.cancelled
                .store(true, std::sync::atomic::Ordering::SeqCst);
            JobResult::Failed {
                id: execution.submission.spec.id,
                error: DagError::Schedule("cancelled".into()),
                duration: std::time::Duration::ZERO,
                details: None,
            }
        }
    }

    #[tokio::test]
    async fn source_error_cancels_and_joins_inflight_task_without_cancelling_caller() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new("consumer", "echo", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "consumer", 0))
            .build()
            .unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let streams = crate::source::SourceStreams::from_test_reader("s", rx);
        let started = Arc::new(tokio::sync::Notify::new());
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let executor = CancellationObservingExecutor {
            started: Arc::clone(&started),
            cancelled: Arc::clone(&cancelled),
        };
        let scheduler =
            FlowScheduler::new(Arc::new(executor), Arc::new(EchoNode::build), engine_ctx());
        let caller = CancellationToken::new();
        let task_token = caller.clone();
        let run = tokio::spawn(async move {
            scheduler
                .run_dataflow_streamed_cached(
                    &graph,
                    streams,
                    &FlowRunConfig::default(),
                    None,
                    Some(task_token),
                )
                .await
        });
        tx.send(Ok(serde_json::json!("first"))).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        tx.send(Err(crate::Error::Execution("source failed".into())))
            .await
            .unwrap();
        let error = tokio::time::timeout(std::time::Duration::from_secs(3), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("source failed"));
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!caller.is_cancelled());
    }

    struct FailingAndWaitingExecutor {
        ready: Arc<tokio::sync::Barrier>,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait::async_trait]
    impl TaskExecutor for FailingAndWaitingExecutor {
        fn name(&self) -> &'static str {
            "failure-and-wait"
        }
        async fn run(&self, execution: TaskExecution) -> JobResult {
            self.ready.wait().await;
            if execution.submission.spec.logical_node.as_deref() != Some("fail") {
                execution.cancellation.cancelled().await;
                self.cancelled
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            JobResult::Failed {
                id: execution.submission.spec.id,
                error: DagError::Schedule("failed".into()),
                duration: std::time::Duration::ZERO,
                details: None,
            }
        }
    }

    #[tokio::test]
    async fn fatal_task_failure_notifies_running_peers_without_cancelling_caller() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("item")]))
            .process(Process::new("fail", "echo", serde_json::json!({})))
            .process(Process::new("wait", "echo", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "fail", 0))
            .wire(Wire::direct("s", 0, "wait", 0))
            .build()
            .unwrap();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let executor = FailingAndWaitingExecutor {
            ready: Arc::new(tokio::sync::Barrier::new(2)),
            cancelled: Arc::clone(&cancelled),
        };
        let scheduler =
            FlowScheduler::new(Arc::new(executor), Arc::new(EchoNode::build), engine_ctx());
        let caller = CancellationToken::new();
        let config = FlowRunConfig {
            max_concurrency: 2,
            resume: false,
        };
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            scheduler.run_dataflow(&graph, &resolved(&graph), &config, Some(caller.clone())),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!report.ok);
        assert_eq!(report.processes["fail"].failed, 1);
        assert!(
            !report.cancelled,
            "internal failure is distinct from user cancellation"
        );
        assert_eq!(report.processes["wait"].cancelled, 1);
        assert_eq!(report.processes["wait"].total, 1);
        assert!(
            report
                .instances
                .iter()
                .any(|r| r.process == "wait" && r.status == RuntimeStatus::Cancelled)
        );
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!caller.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_inflight_instance_is_in_flow_report() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("item")]))
            .process(Process::new("worker", "echo", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "worker", 0))
            .build()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let scheduler = FlowScheduler::new(
            Arc::new(CancellationObservingExecutor {
                started: Arc::clone(&started),
                cancelled: Arc::clone(&cancelled),
            }),
            Arc::new(EchoNode::build),
            engine_ctx(),
        );
        let token = CancellationToken::new();
        let child = token.clone();
        let run = tokio::spawn(async move {
            scheduler
                .run_dataflow(
                    &graph,
                    &resolved(&graph),
                    &FlowRunConfig::default(),
                    Some(child),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        token.cancel();
        let report = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!report.ok);
        assert_eq!(report.processes["worker"].total, 1);
        assert_eq!(report.processes["worker"].cancelled, 1);
        assert_eq!(report.instances[0].status, RuntimeStatus::Cancelled);
        assert_eq!(report.instances[0].attempts, 1);
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn dataflow_cancellation_interrupts_waiting_for_source() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new("consumer", "echo", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "consumer", 0))
            .build()
            .unwrap();
        let (_sender, receiver) = tokio::sync::mpsc::channel(1);
        let streams = crate::source::SourceStreams::from_test_reader("s", receiver);
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            cancel.cancel();
        });
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            scheduler().run_dataflow_streamed_cached(
                &graph,
                streams,
                &FlowRunConfig::default(),
                None,
                Some(token),
            ),
        )
        .await
        .expect("cancellation must interrupt source wait")
        .unwrap();
        assert!(!report.ok);
        assert!(report.cancelled);
        assert_eq!(report.processes["consumer"].total, 0);
    }

    #[derive(Clone)]
    struct FailingStreamNode {
        ports: dag_core::NodePorts,
    }
    #[async_trait::async_trait]
    impl DagNode for FailingStreamNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            reporter
                .emit_channel_item(0, 0, serde_json::json!("already-delivered"))
                .await;
            Err(DagError::Schedule("producer failed".into()))
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "failing_stream"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn dataflow_does_not_retry_after_stream_delivery() {
        let mut producer = Process::new("producer", "failing_stream", serde_json::json!({}));
        producer.error_strategy = crate::model::ErrorStrategy::retry(3, 0);
        let graph = FlowGraphBuilder::default()
            .process(producer)
            .build()
            .unwrap();
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(FailingStreamNode {
                ports: dag_core::NodePorts::new(),
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let report = scheduler
            .run_dataflow(
                &graph,
                &SourceResolution::new(),
                &FlowRunConfig::default(),
                None,
            )
            .await
            .unwrap();
        assert!(!report.ok);
        assert_eq!(report.instances.len(), 1);
        assert_eq!(report.instances[0].attempts, 1);
        assert!(
            report.instances[0]
                .error
                .as_deref()
                .unwrap()
                .contains("cannot retry")
        );
    }

    #[tokio::test]
    async fn dataflow_resume_uses_actual_input_files_and_invalidates_changes() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("a.txt");
        std::fs::write(&input, "first").unwrap();
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "s",
                vec![serde_json::json!(input.to_string_lossy())],
            ))
            .process(Process::new(
                "qc",
                "echo",
                serde_json::json!({"tag":unique_tag("cache")}),
            ))
            .wire(Wire::direct("s", 0, "qc", 0))
            .build()
            .unwrap();
        let mut cache = InstanceCache::new();
        let cfg = FlowRunConfig {
            resume: true,
            max_concurrency: 2,
        };
        let scheduler = scheduler();
        let first = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(first.processes["qc"].reused, 0);
        let second = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(second.processes["qc"].reused, 1);
        std::fs::write(&input, "a changed input").unwrap();
        let third = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(third.processes["qc"].reused, 0);
    }

    #[tokio::test]
    async fn dataflow_resume_tracks_files_nested_in_channel_values() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("a.txt");
        std::fs::write(&input, "first").unwrap();
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "s",
                vec![serde_json::json!({"sample":"x", "reads":[{"path":input.to_string_lossy()}]})],
            ))
            .process(Process::new(
                "qc",
                "echo",
                serde_json::json!({"tag":unique_tag("nested-cache")}),
            ))
            .wire(Wire::direct("s", 0, "qc", 0))
            .build()
            .unwrap();
        let mut cache = InstanceCache::new();
        let cfg = FlowRunConfig {
            resume: true,
            max_concurrency: 2,
        };
        let scheduler = scheduler();
        let first = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(first.processes["qc"].reused, 0);
        let second = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(second.processes["qc"].reused, 1);
        std::fs::write(&input, "changed input size").unwrap();
        let third = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(third.processes["qc"].reused, 0);
    }

    #[tokio::test]
    async fn dataflow_resume_does_not_cache_unverifiable_nested_remote_paths() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "s",
                vec![serde_json::json!({"reads":["vfs://data/a.fastq"]})],
            ))
            .process(Process::new(
                "qc",
                "echo",
                serde_json::json!({"tag":unique_tag("remote-cache")}),
            ))
            .wire(Wire::direct("s", 0, "qc", 0))
            .build()
            .unwrap();
        let mut cache = InstanceCache::new();
        let cfg = FlowRunConfig {
            resume: true,
            max_concurrency: 2,
        };
        let scheduler = scheduler();
        for _ in 0..2 {
            let report = scheduler
                .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
                .await
                .unwrap();
            assert!(report.ok);
            assert_eq!(report.processes["qc"].reused, 0);
        }
    }

    #[derive(Clone)]
    struct CacheContractNode {
        inner: Box<dyn DagNode>,
        allow: bool,
        dependency: std::path::PathBuf,
        referenced: std::path::PathBuf,
        plugin: Option<dag_core::fingerprint::PluginIdentity>,
    }
    #[async_trait::async_trait]
    impl DagNode for CacheContractNode {
        fn ports(&self) -> &dag_core::NodePorts {
            self.inner.ports()
        }
        async fn execute(
            &mut self,
            ctx: &NodeCtx,
            inputs: &[NodeInput],
            reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            self.inner.execute(ctx, inputs, reporter).await
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "cache_contract"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn allows_cache_reuse(&self) -> bool {
            self.allow
        }
        fn referenced_file_paths(&self) -> Vec<String> {
            vec![self.referenced.to_string_lossy().into_owned()]
        }
        fn plugin_identity(&self) -> Option<&dag_core::fingerprint::PluginIdentity> {
            self.plugin.as_ref()
        }
        async fn cache_dependency_state(
            &self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
        ) -> Result<Option<serde_json::Value>, DagError> {
            let value = std::fs::read_to_string(&self.dependency)
                .map_err(|e| DagError::Schedule(e.to_string()))?;
            Ok(Some(serde_json::json!(value)))
        }
    }

    #[tokio::test]
    async fn dataflow_resume_honors_node_cache_contracts() {
        let directory = tempfile::tempdir().unwrap();
        let dependency = directory.path().join("dependency");
        let referenced = directory.path().join("reference");
        std::fs::write(&dependency, "version-a").unwrap();
        std::fs::write(&referenced, "ref-a").unwrap();
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!(1)]))
            .process(Process::new(
                "qc",
                "cache_contract",
                serde_json::json!({"tag":unique_tag("contract")}),
            ))
            .wire(Wire::direct("s", 0, "qc", 0))
            .build()
            .unwrap();
        let cfg = FlowRunConfig {
            resume: true,
            max_concurrency: 2,
        };
        for allow in [true, false] {
            let dependency = dependency.clone();
            let referenced = referenced.clone();
            let builder: NodeBuilder = Arc::new(move |_, spec| {
                Ok(Box::new(CacheContractNode {
                    inner: EchoNode::build("echo", spec)?,
                    allow,
                    dependency: dependency.clone(),
                    referenced: referenced.clone(),
                    plugin: None,
                }))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
            let mut cache = InstanceCache::new();
            let first = scheduler
                .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
                .await
                .unwrap();
            assert_eq!(first.processes["qc"].reused, 0);
            let second = scheduler
                .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
                .await
                .unwrap();
            assert_eq!(second.processes["qc"].reused, usize::from(allow));
            if allow {
                std::fs::write(directory.path().join("dependency"), "version-b").unwrap();
                let changed = scheduler
                    .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
                    .await
                    .unwrap();
                assert_eq!(changed.processes["qc"].reused, 0);
                std::fs::write(directory.path().join("reference"), "changed-reference-size")
                    .unwrap();
                let changed = scheduler
                    .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
                    .await
                    .unwrap();
                assert_eq!(changed.processes["qc"].reused, 0);
            }
        }
    }

    #[tokio::test]
    async fn dataflow_resume_invalidates_when_plugin_identity_changes() {
        let directory = tempfile::tempdir().unwrap();
        let dependency = directory.path().join("dependency");
        let referenced = directory.path().join("reference");
        std::fs::write(&dependency, "same").unwrap();
        std::fs::write(&referenced, "same").unwrap();
        let revision = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let builder: NodeBuilder = {
            let revision = Arc::clone(&revision);
            Arc::new(move |_, spec| {
                Ok(Box::new(CacheContractNode {
                    inner: EchoNode::build("echo", spec)?,
                    allow: true,
                    dependency: dependency.clone(),
                    referenced: referenced.clone(),
                    plugin: Some(dag_core::fingerprint::PluginIdentity {
                        manifest_sha256: format!(
                            "sha256:{}",
                            revision.load(std::sync::atomic::Ordering::SeqCst)
                        ),
                        script_sha256: None,
                        image_reference: "image@sha256:abc".into(),
                        panels: vec![],
                    }),
                }))
            })
        };
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!(1)]))
            .process(Process::new(
                "qc",
                "cache_contract",
                serde_json::json!({"tag":unique_tag("plugin")}),
            ))
            .wire(Wire::direct("s", 0, "qc", 0))
            .build()
            .unwrap();
        let cfg = FlowRunConfig {
            resume: true,
            max_concurrency: 2,
        };
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let mut cache = InstanceCache::new();
        scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        let reused = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(reused.processes["qc"].reused, 1);
        revision.store(2, std::sync::atomic::Ordering::SeqCst);
        let changed = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap();
        assert_eq!(changed.processes["qc"].reused, 0);
    }

    #[tokio::test]
    async fn cache_dependency_errors_abort_instead_of_reusing() {
        let directory = tempfile::tempdir().unwrap();
        let dependency = directory.path().join("missing");
        let referenced = directory.path().join("reference");
        std::fs::write(&referenced, "valid").unwrap();
        let builder: NodeBuilder = Arc::new(move |_, spec| {
            Ok(Box::new(CacheContractNode {
                inner: EchoNode::build("echo", spec)?,
                allow: true,
                dependency: dependency.clone(),
                referenced: referenced.clone(),
                plugin: None,
            }))
        });
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!(1)]))
            .process(Process::new(
                "qc",
                "cache_contract",
                serde_json::json!({"tag":unique_tag("error")}),
            ))
            .wire(Wire::direct("s", 0, "qc", 0))
            .build()
            .unwrap();
        let cfg = FlowRunConfig {
            resume: true,
            max_concurrency: 2,
        };
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let mut cache = InstanceCache::new();
        let error = scheduler
            .run_dataflow_cached(&graph, &resolved(&graph), &cfg, Some(&mut cache), None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("cache dependency for `qc`"),
            "{error}"
        );
        assert!(cache.is_empty());
    }

    #[derive(Clone)]
    struct BatchChannelNode {
        ports: dag_core::NodePorts,
        producer: bool,
    }
    #[async_trait::async_trait]
    impl DagNode for BatchChannelNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            let mut outputs = PortOutputs::new();
            if self.producer {
                outputs.insert(
                    0,
                    NodeValue::Channel(dag_core::value::ChannelValue {
                        items: vec![serde_json::json!("one"), serde_json::json!("two")],
                    }),
                );
            }
            Ok(outputs)
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "batch_channel"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    #[tokio::test]
    async fn dataflow_batch_channel_output_fires_per_emitted_item() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("trigger")]))
            .process(Process::new(
                "producer",
                "batch",
                serde_json::json!({"producer":true}),
            ))
            .process(Process::new("consumer", "batch", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "producer", 0))
            .wire(Wire::direct("producer", 0, "consumer", 0))
            .build()
            .unwrap();
        let builder: NodeBuilder = Arc::new(|_, spec| {
            Ok(Box::new(BatchChannelNode {
                ports: dag_core::NodePorts::new(),
                producer: spec["producer"].as_bool().unwrap_or(false),
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["producer"].succeeded, 1);
        assert_eq!(report.processes["consumer"].succeeded, 2);
    }

    #[derive(Clone)]
    struct FileCollectionInspectNode {
        ports: dag_core::NodePorts,
    }

    #[async_trait::async_trait]
    impl DagNode for FileCollectionInspectNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _: &NodeCtx,
            inputs: &[NodeInput],
            _: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            match &inputs[0].data {
                NodeValue::FileSet(files)
                    if files
                        .iter()
                        .map(|file| file.path.as_str())
                        .collect::<Vec<_>>()
                        == ["a.fastq", "b.fastq"] =>
                {
                    Ok(PortOutputs::new())
                }
                other => Err(DagError::Schedule(format!(
                    "expected collected files, got {other:?}"
                ))),
            }
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "inspect_files"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn collected_file_channel_reaches_a_file_port() {
        for operator in ["collect", "to_list"] {
            let graph = FlowGraphBuilder::default()
                .source(Source::glob("reads", "*.fastq"))
                .process(Process::new(
                    "batch",
                    "channel",
                    serde_json::json!({"operator":operator}),
                ))
                .process(Process::new("sink", "inspect_files", serde_json::json!({})))
                .wire(Wire::direct("reads", 0, "batch", 0))
                .wire(Wire::direct("batch", 0, "sink", 0))
                .build()
                .unwrap();
            let mut resolution = SourceResolution::new();
            resolution.resolve(
                "reads",
                vec![serde_json::json!("a.fastq"), serde_json::json!("b.fastq")],
            );
            let builder: NodeBuilder = Arc::new(|_, _| {
                Ok(Box::new(FileCollectionInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::File),
                }))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
            let report = scheduler
                .run_dataflow(&graph, &resolution, &FlowRunConfig::default(), None)
                .await
                .unwrap();
            assert!(report.ok, "{operator}: {:?}", report.instances);
            assert_eq!(report.processes["sink"].succeeded, 1);
        }
    }

    #[derive(Clone)]
    struct ValueInspectNode {
        ports: dag_core::NodePorts,
    }
    #[async_trait::async_trait]
    impl DagNode for ValueInspectNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            match &inputs[0].data {
                NodeValue::Channel(value)
                    if value.items == vec![serde_json::json!("literal-value")] =>
                {
                    Ok(PortOutputs::new())
                }
                other => Err(DagError::Schedule(format!("unexpected input: {other:?}"))),
            }
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "inspect_value"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    #[tokio::test]
    async fn dataflow_list_string_remains_a_value_on_channel_port() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("literal-value")]))
            .process(Process::new(
                "check",
                "inspect_value",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "check", 0))
            .build()
            .unwrap();
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(ValueInspectNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel),
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
    }

    #[derive(Clone)]
    struct GroupStreamNode {
        ports: dag_core::NodePorts,
        producer: bool,
        consumed: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl DagNode for GroupStreamNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            inputs: &[NodeInput],
            reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            if self.producer {
                reporter
                    .emit_channel_item(0, 0, serde_json::json!(["g", "a"]))
                    .await;
                reporter
                    .emit_channel_item(0, 1, serde_json::json!(["g", "b"]))
                    .await;
                tokio::time::timeout(std::time::Duration::from_secs(2), self.consumed.notified())
                    .await
                    .map_err(|_| {
                        DagError::Schedule("group consumer did not run before source closed".into())
                    })?;
                reporter.close_channel(0, 2).await;
            } else {
                match &inputs[0].data {
                    NodeValue::Channel(v)
                        if v.items == vec![serde_json::json!(["g", ["a", "b"]])] =>
                    {
                        self.consumed.notify_one()
                    }
                    other => {
                        return Err(DagError::Schedule(format!("unexpected group: {other:?}")));
                    }
                }
            }
            Ok(PortOutputs::new())
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "group_stream"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    #[tokio::test]
    async fn dataflow_group_tuple_size_emits_before_source_closes() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!("trigger")]))
            .process(Process::new(
                "producer",
                "group_stream",
                serde_json::json!({"producer":true}),
            ))
            .process(Process::new(
                "group",
                "channel",
                serde_json::json!({"operator":"group_tuple", "size":2}),
            ))
            .process(Process::new(
                "consumer",
                "group_stream",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "producer", 0))
            .wire(Wire::direct("producer", 0, "group", 0))
            .wire(Wire::direct("group", 0, "consumer", 0))
            .build()
            .unwrap();
        let consumed = Arc::new(tokio::sync::Notify::new());
        let builder: NodeBuilder = Arc::new(move |_, spec| {
            Ok(Box::new(GroupStreamNode {
                ports: dag_core::NodePorts::new(),
                producer: spec["producer"].as_bool().unwrap_or(false),
                consumed: Arc::clone(&consumed),
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
        let cfg = FlowRunConfig {
            max_concurrency: 2,
            resume: false,
        };
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &cfg, None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["consumer"].succeeded, 1);
    }

    #[derive(Clone)]
    struct ListInspectNode {
        ports: dag_core::NodePorts,
        expected: serde_json::Value,
    }

    fn channel_value(value: serde_json::Value) -> NodeValue {
        NodeValue::Channel(dag_core::value::ChannelValue { items: vec![value] })
    }

    fn is_one(value: &NodeValue) -> bool {
        matches!(value, NodeValue::Channel(channel) if channel.items == vec![serde_json::json!(1)])
    }
    #[async_trait::async_trait]
    impl DagNode for ListInspectNode {
        fn ports(&self) -> &dag_core::NodePorts {
            &self.ports
        }
        async fn execute(
            &mut self,
            _ctx: &NodeCtx,
            inputs: &[NodeInput],
            _reporter: &NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            match &inputs[0].data {
                NodeValue::Channel(value) if value.items == vec![self.expected.clone()] => {
                    Ok(PortOutputs::new())
                }
                other => Err(DagError::Schedule(format!(
                    "unexpected list input: {other:?}"
                ))),
            }
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }
        fn kind(&self) -> &'static str {
            "inspect_list"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    #[tokio::test]
    async fn channel_functions_preserve_input_order_when_first_evaluation_is_slow() {
        let function = serde_json::json!({"__dag_shell_function__":{"source":"def f(x): return x", "captures":{}}});
        for spec in [
            serde_json::json!({"operator":"map", "function":function}),
            serde_json::json!({"operator":"filter", "function":function}),
            serde_json::json!({"operator":"branch", "branches":[{"name":"out", "function":function}]}),
        ] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items(
                    "s",
                    vec![serde_json::json!(1), serde_json::json!(2)],
                ))
                .process(Process::new("operator", "channel", spec.clone()))
                .process(Process::new(
                    "list",
                    "channel",
                    serde_json::json!({"operator":"to_list"}),
                ))
                .process(Process::new("sink", "inspect_list", serde_json::json!({})))
                .wire(Wire::direct("s", 0, "operator", 0))
                .wire(Wire::direct("operator", 0, "list", 0))
                .wire(Wire::direct("list", 0, "sink", 0))
                .build()
                .unwrap();
            let builder: NodeBuilder = Arc::new(|_, _| {
                Ok(Box::new(ListInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                    expected: serde_json::json!([1, 2]),
                }))
            });
            let map = spec["operator"] == "map";
            let evaluator: ChannelFunction = Arc::new(move |_, input| {
                let input = input.expect("operator input");
                if matches!(input, NodeValue::Channel(channel) if channel.items == vec![serde_json::json!(1)])
                {
                    std::thread::sleep(std::time::Duration::from_millis(75));
                }
                Ok(if map {
                    input.clone()
                } else {
                    channel_value(serde_json::json!(true))
                })
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
                .with_channel_function(evaluator);
            let cfg = FlowRunConfig {
                max_concurrency: 4,
                ..FlowRunConfig::default()
            };
            let report = scheduler
                .run_dataflow(&graph, &resolved(&graph), &cfg, None)
                .await
                .unwrap();
            assert!(report.ok, "{:?}: {:?}", spec, report.instances);
            assert_eq!(report.processes["sink"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn if_empty_function_runs_only_for_an_empty_channel() {
        for (items, expected, calls) in [(vec![], 42, 1), (vec![serde_json::json!(7)], 7, 0)] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items("s", items))
                .process(Process::new("default", "channel", serde_json::json!({"operator":"if_empty", "value":{"__dag_shell_function__":{"source":"def f(): return 42", "captures":{}}}})))
                .process(Process::new("sink", "inspect_list", serde_json::json!({})))
                .wire(Wire::direct("s", 0, "default", 0)).wire(Wire::direct("default", 0, "sink", 0)).build().unwrap();
            let builder: NodeBuilder = Arc::new(move |_, _| {
                Ok(Box::new(ListInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                    expected: serde_json::json!(expected),
                }))
            });
            let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed = Arc::clone(&called);
            let evaluator: ChannelFunction = Arc::new(move |_, input| {
                assert!(input.is_none());
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(channel_value(serde_json::json!(42)))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
                .with_channel_function(evaluator);
            let report = scheduler
                .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["sink"].succeeded, 1);
            assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), calls);
        }
    }

    #[tokio::test]
    async fn first_and_count_accept_function_predicates() {
        let predicate = serde_json::json!({"__dag_shell_function__":{"source":"def f(x): return x", "captures":{}}});
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "s",
                vec![
                    serde_json::json!(1),
                    serde_json::json!(2),
                    serde_json::json!(2),
                ],
            ))
            .process(Process::new(
                "first",
                "channel",
                serde_json::json!({"operator":"first", "criteria":predicate}),
            ))
            .process(Process::new(
                "count",
                "channel",
                serde_json::json!({"operator":"count", "criteria":predicate}),
            ))
            .process(Process::new(
                "first_sink",
                "inspect_list",
                serde_json::json!({}),
            ))
            .process(Process::new(
                "count_sink",
                "inspect_list",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("s", 0, "first", 0))
            .wire(Wire::direct("s", 0, "count", 0))
            .wire(Wire::direct("first", 0, "first_sink", 0))
            .wire(Wire::direct("count", 0, "count_sink", 0))
            .build()
            .unwrap();
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(ListInspectNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                expected: serde_json::json!(2),
            }))
        });
        let evaluator: ChannelFunction = Arc::new(|_, item| {
            Ok(channel_value(serde_json::json!(
                matches!(item, Some(NodeValue::Channel(channel)) if channel.items == vec![serde_json::json!(2)])
            )))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
            .with_channel_function(evaluator);
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["first_sink"].succeeded, 1);
        assert_eq!(report.processes["count_sink"].succeeded, 1);
    }

    #[tokio::test]
    async fn dataflow_flat_map_function_expands_with_backpressure() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!(1)]))
            .process(Process::new("flat", "channel", serde_json::json!({"operator":"flat_map", "function":{"__dag_shell_function__":{"source":"def f(x): return x", "captures":{}}}})))
            .process(Process::new("list", "channel", serde_json::json!({"operator":"to_list"})))
            .process(Process::new("sink", "inspect_list", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "flat", 0))
            .wire(Wire::direct("flat", 0, "list", 0))
            .wire(Wire::direct("list", 0, "sink", 0))
            .build().unwrap();
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(ListInspectNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                expected: serde_json::json!((0..500).collect::<Vec<_>>()),
            }))
        });
        let evaluator: ChannelFunction = Arc::new(|_, _| {
            Ok(channel_value(serde_json::json!(
                (0..500).collect::<Vec<_>>()
            )))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
            .with_channel_function(evaluator);
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            scheduler.run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].succeeded, 1);
    }

    #[tokio::test]
    async fn dataflow_filter_function_preserves_matching_input() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!(1), serde_json::json!(2)]))
            .process(Process::new("filter", "channel", serde_json::json!({"operator":"filter", "function":{"__dag_shell_function__":{"source":"def f(x): return x", "captures":{}}}})))
            .process(Process::new("sink", "inspect_list", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "filter", 0))
            .wire(Wire::direct("filter", 0, "sink", 0))
            .build().unwrap();
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(ListInspectNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                expected: serde_json::json!(2),
            }))
        });
        let evaluator: ChannelFunction = Arc::new(|_, item| {
            Ok(channel_value(if is_one(item.expect("input")) {
                serde_json::json!([])
            } else {
                serde_json::json!("nonempty")
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
            .with_channel_function(evaluator);
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].succeeded, 1);
    }

    #[tokio::test]
    async fn channel_functions_do_not_block_independent_processes() {
        let spec = serde_json::json!({"operator":"map", "function":{"__dag_shell_function__":{"source":"def f(x): return x", "captures":{}}}});
        let graph = FlowGraphBuilder::default()
            .source(Source::items("a", vec![serde_json::json!(1)]))
            .source(Source::items("b", vec![serde_json::json!(1)]))
            .process(Process::new("map_a", "channel", spec.clone()))
            .process(Process::new("map_b", "channel", spec))
            .process(Process::new(
                "sink_a",
                "inspect_list",
                serde_json::json!({}),
            ))
            .process(Process::new(
                "sink_b",
                "inspect_list",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("a", 0, "map_a", 0))
            .wire(Wire::direct("b", 0, "map_b", 0))
            .wire(Wire::direct("map_a", 0, "sink_a", 0))
            .wire(Wire::direct("map_b", 0, "sink_b", 0))
            .build()
            .unwrap();
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let evaluator: ChannelFunction = Arc::new(move |_, item| {
            started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while started.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                if std::time::Instant::now() >= deadline {
                    return Err("independent function was blocked".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Ok(item.expect("input").clone())
        });
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(ListInspectNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                expected: serde_json::json!(1),
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
            .with_channel_function(evaluator);
        let config = FlowRunConfig {
            max_concurrency: 2,
            resume: false,
        };
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &config, None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink_a"].succeeded, 1);
        assert_eq!(report.processes["sink_b"].succeeded, 1);
    }

    #[tokio::test]
    async fn dataflow_map_executes_persisted_function_and_skips_null() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![serde_json::json!(1), serde_json::json!(2)]))
            .process(Process::new("map", "channel", serde_json::json!({"operator":"map", "function":{"__dag_shell_function__":{"source":"def transform(x): return x", "captures":{}}}})))
            .process(Process::new("sink", "inspect_list", serde_json::json!({})))
            .wire(Wire::direct("s", 0, "map", 0))
            .wire(Wire::direct("map", 0, "sink", 0))
            .build().unwrap();
        let builder: NodeBuilder = Arc::new(|_, _| {
            Ok(Box::new(ListInspectNode {
                ports: dag_core::NodePorts::new()
                    .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                expected: serde_json::json!(20),
            }))
        });
        let evaluator: ChannelFunction = Arc::new(|_, item| {
            Ok(channel_value(if is_one(item.expect("input")) {
                serde_json::Value::Null
            } else {
                serde_json::json!(20)
            }))
        });
        let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx())
            .with_channel_function(evaluator);
        let report = scheduler
            .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["sink"].succeeded, 1);
    }

    #[tokio::test]
    async fn dataflow_count_matches_nextflow_literal_criteria() {
        for (criteria, expected) in [
            (serde_json::json!(2), 2),
            (serde_json::json!([1, 3]), 2),
            (serde_json::json!(null), 4),
            (serde_json::json!(9), 0),
        ] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items(
                    "s",
                    vec![
                        serde_json::json!(1),
                        serde_json::json!(2),
                        serde_json::json!(2),
                        serde_json::json!(3),
                    ],
                ))
                .process(Process::new(
                    "count",
                    "channel",
                    serde_json::json!({"operator":"count", "criteria":criteria}),
                ))
                .process(Process::new("sink", "inspect_list", serde_json::json!({})))
                .wire(Wire::direct("s", 0, "count", 0))
                .wire(Wire::direct("count", 0, "sink", 0))
                .build()
                .unwrap();
            let builder: NodeBuilder = Arc::new(move |_, _| {
                Ok(Box::new(ListInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                    expected: serde_json::json!(expected),
                }))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
            let report = scheduler
                .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["sink"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn dataflow_to_list_emits_empty_and_preserves_nested_items() {
        for values in [
            vec![],
            vec![serde_json::json!([1, 2]), serde_json::json!(3)],
            vec![serde_json::json!([]), serde_json::json!(3)],
        ] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items("s", values.clone()))
                .process(Process::new(
                    "list",
                    "channel",
                    serde_json::json!({"operator":"to_list"}),
                ))
                .process(Process::new("sink", "inspect_list", serde_json::json!({})))
                .wire(Wire::direct("s", 0, "list", 0))
                .wire(Wire::direct("list", 0, "sink", 0))
                .build()
                .unwrap();
            let expected = serde_json::Value::Array(values);
            let builder: NodeBuilder = Arc::new(move |_, _| {
                Ok(Box::new(ListInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                    expected: expected.clone(),
                }))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
            let report = scheduler
                .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["sink"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn dataflow_collect_flattens_empty_lists_before_deciding_whether_to_emit() {
        for (values, expected_count) in [
            (vec![serde_json::json!([]), serde_json::json!([])], 0),
            (vec![serde_json::json!([]), serde_json::json!([1])], 1),
        ] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items("s", values))
                .process(Process::new("sink", "inspect_list", serde_json::json!({})))
                .wire(Wire::collect("s", 0, "sink", 0))
                .build()
                .unwrap();
            let builder: NodeBuilder = Arc::new(|_, _| {
                Ok(Box::new(ListInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                    expected: serde_json::json!(1),
                }))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
            let report = scheduler
                .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
                .await
                .unwrap();
            assert!(report.ok, "{:?}", report.instances);
            assert_eq!(report.processes["sink"].succeeded, expected_count);
        }
    }

    #[tokio::test]
    async fn dataflow_unique_and_distinct_follow_stream_order() {
        for (operator, expected) in [
            ("unique", serde_json::json!(["a", "b", null])),
            ("distinct", serde_json::json!(["a", "b", "a", null])),
        ] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items(
                    "s",
                    vec![
                        serde_json::json!("a"),
                        serde_json::json!("a"),
                        serde_json::json!("b"),
                        serde_json::json!("a"),
                        serde_json::json!(null),
                        serde_json::json!(null),
                    ],
                ))
                .process(Process::new(
                    "dedup",
                    "channel",
                    serde_json::json!({"operator":operator}),
                ))
                .process(Process::new(
                    "list",
                    "channel",
                    serde_json::json!({"operator":"to_list"}),
                ))
                .process(Process::new("sink", "inspect_list", serde_json::json!({})))
                .wire(Wire::direct("s", 0, "dedup", 0))
                .wire(Wire::direct("dedup", 0, "list", 0))
                .wire(Wire::direct("list", 0, "sink", 0))
                .build()
                .unwrap();
            let builder: NodeBuilder = Arc::new(move |_, _| {
                Ok(Box::new(ListInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                    expected: expected.clone(),
                }))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
            let report = scheduler
                .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
                .await
                .unwrap();
            assert!(report.ok, "{operator}: {:?}", report.instances);
            assert_eq!(report.processes["sink"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn dataflow_first_and_take_close_without_affecting_other_subscribers() {
        for (spec, selected) in [
            (
                serde_json::json!({"operator":"first"}),
                serde_json::json!(["a"]),
            ),
            (
                serde_json::json!({"operator":"take", "count":0}),
                serde_json::json!([]),
            ),
            (
                serde_json::json!({"operator":"take", "count":2}),
                serde_json::json!(["a", "b"]),
            ),
            (
                serde_json::json!({"operator":"take", "count":-1}),
                serde_json::json!(["a", "b", "c"]),
            ),
        ] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items(
                    "s",
                    vec![
                        serde_json::json!("a"),
                        serde_json::json!("b"),
                        serde_json::json!("c"),
                    ],
                ))
                .process(Process::new("limit", "channel", spec.clone()))
                .process(Process::new(
                    "selected",
                    "channel",
                    serde_json::json!({"operator":"to_list"}),
                ))
                .process(Process::new(
                    "all",
                    "channel",
                    serde_json::json!({"operator":"to_list"}),
                ))
                .process(Process::new(
                    "selected_sink",
                    "inspect_list",
                    serde_json::json!({"expected":selected}),
                ))
                .process(Process::new(
                    "all_sink",
                    "inspect_list",
                    serde_json::json!({"expected":["a", "b", "c"]}),
                ))
                .wire(Wire::direct("s", 0, "limit", 0))
                .wire(Wire::direct("limit", 0, "selected", 0))
                .wire(Wire::direct("s", 0, "all", 0))
                .wire(Wire::direct("selected", 0, "selected_sink", 0))
                .wire(Wire::direct("all", 0, "all_sink", 0))
                .build()
                .unwrap();
            let builder: NodeBuilder = Arc::new(|_, spec| {
                Ok(Box::new(ListInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                    expected: spec["expected"].clone(),
                }))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
            let report = scheduler
                .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
                .await
                .unwrap();
            assert!(report.ok, "{spec}: {:?}", report.instances);
            assert_eq!(report.processes["selected_sink"].succeeded, 1);
            assert_eq!(report.processes["all_sink"].succeeded, 1);
        }
    }

    #[tokio::test]
    async fn dataflow_last_and_count_bind_after_closure_including_empty_input() {
        for (operator, values, expected) in [
            ("last", vec![], serde_json::json!(null)),
            (
                "last",
                vec![serde_json::json!(1), serde_json::json!(2)],
                serde_json::json!(2),
            ),
            ("count", vec![], serde_json::json!(0)),
            (
                "count",
                vec![serde_json::json!(1), serde_json::json!(2)],
                serde_json::json!(2),
            ),
        ] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items("s", values))
                .process(Process::new(
                    "reduce",
                    "channel",
                    serde_json::json!({"operator":operator}),
                ))
                .process(Process::new("sink", "inspect_list", serde_json::json!({})))
                .wire(Wire::direct("s", 0, "reduce", 0))
                .wire(Wire::direct("reduce", 0, "sink", 0))
                .build()
                .unwrap();
            let builder: NodeBuilder = Arc::new(move |_, _| {
                Ok(Box::new(ListInspectNode {
                    ports: dag_core::NodePorts::new()
                        .add_input_port_of_type(None, dag_core::value::PortType::Channel),
                    expected: expected.clone(),
                }))
            });
            let scheduler = FlowScheduler::new(Arc::new(InlineExecutor), builder, engine_ctx());
            let report = scheduler
                .run_dataflow(&graph, &resolved(&graph), &FlowRunConfig::default(), None)
                .await
                .unwrap();
            assert!(report.ok, "{operator}: {:?}", report.instances);
            assert_eq!(report.processes["sink"].succeeded, 1);
        }
    }
}
