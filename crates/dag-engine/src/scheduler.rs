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

use dag_core::dag::graph::PortOutputs;
use dag_core::dag::execution::{TaskExecution, TaskExecutor, TaskSubmission};
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
    /// True when every materialized instance ended `Success` (vacuously true
    /// for a run whose fanouts resolved to zero items — matching the "no
    /// data, nothing to do" posture).
    pub ok: bool,
    /// Frozen executor binding for this invocation (from `bind_run`), when
    /// the executor provides one.
    pub execution_binding: Option<serde_json::Value>,
    pub processes: BTreeMap<ProcessName, ProcessRunSummary>,
    #[serde(skip)]
    pub instances: Vec<InstanceRecord>,
}

/// One flow scheduler: executor stack + node builder + engine context. Cheap
/// to construct; holds no mutable graph state.
pub struct FlowScheduler {
    executor: Arc<dyn TaskExecutor>,
    node_builder: NodeBuilder,
    engine_ctx: Arc<NodeCtx>,
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
        }
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
                let node =
                    (self.node_builder)(&process_plan.kind, instance.rendered_spec.clone())?;
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
                        .try_reuse(plan, &id, cache, &effective, &mut statuses, &mut outputs, &mut records)
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

        // Compose the terminal report from statuses so instances that never
        // dispatched (skipped by cascade, cancelled by abort) are present.
        // After an abort, everything not yet terminal reads as cancelled.
        if run_cancel.is_cancelled() {
            for (id, status) in statuses.iter_mut() {
                if matches!(status, RuntimeStatus::Pending | RuntimeStatus::Running) {
                    *status = RuntimeStatus::Cancelled;
                    fatal.insert(id.clone());
                }
            }
        }
        for (id, status) in &statuses {
            let record = records.entry(id.clone()).or_insert_with(|| InstanceRecord {
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
        }

        // A run is ok when nothing fatal happened: every instance either
        // succeeded or was an explicitly tolerated failure (Nextflow: an
        // `ignore`d error does not fail the pipeline).
        let ok = fatal.is_empty();
        let mut report = FlowRunReport {
            ok,
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
                        error: DagError::Schedule(message),
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
                outputs[id].iter().map(|(port, value)| (*port, value.clone())).collect(),
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
    /// from the node's declared ports, resources from the node's declaration.
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
                name: format!("port_{}", route.to_port),
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
                axis: None,
                item_key: instance.item_key.clone(),
                item: instance.item.clone(),
                inputs: bindings,
                outputs: outputs_contract,
            },
            inputs,
            resources: node.task_resources().unwrap_or_default(),
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
                records.insert(id.clone(), record_for(
                    plan, &id, RuntimeStatus::Success,
                    outputs[&id].iter().map(|(port, value)| (*port, value.clone())).collect(),
                    None, duration, false, false, *attempts.get(&id).unwrap_or(&1),
                ));
                // Identity settles after execution: reused and freshly
                // executed instances are indistinguishable downstream.
                if let Some(fingerprint) = plan_fingerprint(plan, &id, effective) {
                    if let Some(cache) = cache.as_deref_mut() {
                        let (process, item_key) = provenance(plan, &id);
                        cache.record(&process, item_key.as_deref(), fingerprint.clone(), &outputs[&id]);
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

                // Retry: re-dispatch until the budget is exhausted, then
                // degrade to terminal failure handling.
                if strategy.mode == ErrorMode::Retry && made_attempts <= strategy.max_retries {
                    statuses.insert(id.clone(), RuntimeStatus::Pending);
                    if strategy.retry_delay_ms == 0 {
                        ready.push_back(id);
                    } else {
                        *delayed_retries += 1;
                        let delay =
                            std::time::Duration::from_millis(strategy.retry_delay_ms);
                        let requeue_tx = requeue_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            let _ = requeue_tx.send(id);
                        });
                    }
                    return;
                }

                let ignored = strategy.mode == ErrorMode::Ignore;
                statuses.insert(id.clone(), RuntimeStatus::Failed);
                records.insert(id.clone(), record_for(
                    plan, &id, RuntimeStatus::Failed, BTreeMap::new(),
                    Some(error.to_string()), duration, false, ignored, made_attempts,
                ));
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
            records.insert(successor.clone(), record_for(
                plan, &successor, RuntimeStatus::Skipped, BTreeMap::new(), None,
                std::time::Duration::ZERO, false, false, 0,
            ));
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
    for route in plan.routes.iter().filter(|route| &route.from == id && route.op == WireOp::Collect) {
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
        for route in plan
            .routes
            .iter()
            .filter(|route| route.from == current)
        {
            let successor = &route.to;
            match route.op {
                WireOp::Direct => {
                    if statuses.get(successor) != Some(&RuntimeStatus::Pending) {
                        continue;
                    }
                    statuses.insert(successor.clone(), RuntimeStatus::Skipped);
                    pending.remove(successor);
                    records.insert(successor.clone(), record_for(
                        plan, successor, RuntimeStatus::Skipped, BTreeMap::new(), None,
                        std::time::Duration::ZERO, false, false, 0,
                    ));
                    direct_skips.push_back(successor.clone());
                }
                WireOp::Collect => {
                    if &current == id {
                        continue; // handled above
                    }
                    if let Some(count) = pending.get_mut(successor) {
                        *count = count.saturating_sub(1);
                        if *count == 0
                            && statuses.get(successor) == Some(&RuntimeStatus::Pending)
                        {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cardinality, FlowGraph, FlowGraphBuilder, Process, Source, Wire};
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
                if let Ok(text) = std::fs::read_to_string(
                    file.path.strip_prefix("vfs://").unwrap_or(&file.path),
                ) {
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

    /// Builds the canonical qc -> align -> multiqc flow with globally
    /// unique tags, so parallel tests never share temp output files.
    fn flow(fail_first_item: bool) -> FlowGraphBuilder {
        flow_with_strategy(fail_first_item, crate::model::ErrorStrategy::default())
    }

    fn flow_with_strategy(
        fail_first_item: bool,
        qc_strategy: crate::model::ErrorStrategy,
    ) -> FlowGraphBuilder {
        let qc = unique_tag("qc");
        let align = unique_tag("align");
        let multiqc = unique_tag("multiqc");
        let fail_on = fail_first_item.then(|| format!("{qc}:a"));
        let items = vec![serde_json::json!("a"), serde_json::json!("b")];
        let mut qc_process = Process::fanout(
            "qc",
            "echo",
            serde_json::json!({ "tag": format!("{}:{{{{item}}}}", qc), "fail_on": fail_on }),
            Source::items(items.clone()),
            "sample",
        );
        qc_process.error_strategy = qc_strategy;
        let builder = FlowGraphBuilder::default()
            .process(qc_process)
            .process(Process::fanout(
                "align",
                "echo",
                serde_json::json!({ "tag": align }),
                Source::items(items),
                "sample",
            ))
            .process(Process::singleton(
                "multiqc",
                "echo",
                serde_json::json!({ "tag": multiqc }),
            ))
            .wire(Wire::direct("qc", 0, "align", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0));
        builder
    }

    fn resolved(graph: &FlowGraph) -> SourceResolution {
        let mut resolution = SourceResolution::new();
        for process in graph.processes() {
            if let Cardinality::Fanout { source, .. } = &process.cardinality {
                if let Source::Items { values } = source {
                    resolution.resolve(process.id.clone(), values.clone());
                }
            }
        }
        resolution
    }

    fn content_of(record: &InstanceRecord) -> String {
        match record.outputs.get(&0) {
            Some(NodeValue::File(file)) => {
                std::fs::read_to_string(
                    file.path.strip_prefix("vfs://").unwrap_or(&file.path),
                )
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
            .run(&plan, &FlowRunConfig { max_concurrency: 2, resume: false }, None, None)
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
            Arc::new(|kind, _| Err(crate::model::Error::Invalid(format!("unknown kind `{kind}`")).into())),
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
        let cfg = FlowRunConfig { resume: true, ..FlowRunConfig::default() };

        let before = executions();
        let first = scheduler().run(&plan, &cfg, Some(&mut cache), None).await.unwrap();
        let first_executions = executions() - before;
        assert_eq!(first_executions, 5, "qc x2 + align x2 + multiqc");
        assert_eq!(first.processes["qc"].reused, 0);
        assert_eq!(cache.len(), 5);

        let before = executions();
        let second = scheduler().run(&plan, &cfg, Some(&mut cache), None).await.unwrap();
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
                first.instances.iter().find(|r| r.id.as_str() == id).unwrap(),
            );
            let second_content = content_of(
                second.instances.iter().find(|r| r.id.as_str() == id).unwrap(),
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
        let cfg = FlowRunConfig { resume: true, ..FlowRunConfig::default() };
        scheduler().run(&plan, &cfg, Some(&mut cache), None).await.unwrap();

        // Rebuild the flow with an edited qc spec template.
        let mut edited = flow(false);
        edited = {
            let rebuilt = edited.build().unwrap();
            // Rebuild via builder mutation is not exposed; construct a fresh
            // graph by cloning processes and editing the qc spec directly.
            let mut processes: Vec<Process> = rebuilt.processes().to_vec();
            for process in &mut processes {
                if process.id == "qc" {
                    process.spec["extra"] = serde_json::json!("edited");
                }
            }
            let wires: Vec<Wire> = rebuilt.wires().to_vec();
            let mut builder = FlowGraphBuilder::default();
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
        let cfg = FlowRunConfig { resume: true, ..FlowRunConfig::default() };
        let first = scheduler().run(&plan, &cfg, Some(&mut cache), None).await.unwrap();

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
        let second = scheduler().run(&plan, &cfg, Some(&mut cache), None).await.unwrap();
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
        let graph = flow_with_strategy(
            true,
            crate::model::ErrorStrategy::retry(2, 0),
        )
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
        let (qc_tag, align_tag, multiqc_tag) =
            (unique_tag("qc"), unique_tag("align"), unique_tag("multiqc"));
        let items = vec![serde_json::json!("a"), serde_json::json!("b")];
        let mut qc = Process::fanout(
            "qc",
            "echo",
            serde_json::json!({ "tag": format!("{}:{{{{item}}}}", qc_tag), "fail_times": 1 }),
            Source::items(items.clone()),
            "sample",
        );
        qc.error_strategy = crate::model::ErrorStrategy::retry(2, 0);
        let graph = FlowGraphBuilder::default()
            .process(qc)
            .process(Process::fanout(
                "align",
                "echo",
                serde_json::json!({ "tag": align_tag }),
                Source::items(items),
                "sample",
            ))
            .process(Process::singleton(
                "multiqc",
                "echo",
                serde_json::json!({ "tag": multiqc_tag }),
            ))
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
        assert_eq!(executions(), 7, "2 qc#a + 1 qc#b + 2 align + 1 multiqc + 1 (qc#a first fail)");
    }

    #[tokio::test]
    async fn delayed_retry_uses_the_backoff_channel() {
        let (qc_tag, multiqc_tag) = (unique_tag("qc"), unique_tag("multiqc"));
        let mut qc = Process::fanout(
            "qc",
            "echo",
            serde_json::json!({ "tag": qc_tag, "fail_times": 1 }),
            Source::items(vec![serde_json::json!("a")]),
            "sample",
        );
        qc.error_strategy = crate::model::ErrorStrategy::retry(1, 20);
        let graph = FlowGraphBuilder::default()
            .process(qc)
            .process(Process::singleton(
                "multiqc",
                "echo",
                serde_json::json!({ "tag": multiqc_tag }),
            ))
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
    async fn empty_fanout_run_is_ok() {
        let graph = FlowGraphBuilder::default()
            .process(Process::fanout(
                "qc",
                "echo",
                serde_json::json!({ "tag": "qc" }),
                Source::items(vec![]),
                "sample",
            ))
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
}
