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

use crate::instance::InstanceId;
use crate::error::Error;
use crate::model::ProcessName;
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
}

impl Default for FlowRunConfig {
    fn default() -> Self {
        Self {
            max_concurrency: std::thread::available_parallelism()
                .map(|cpus| cpus.get())
                .unwrap_or(1),
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
    pub outputs: BTreeMap<u8, NodeValue>,
    /// Failure text (`DagError` display) for `Failed` instances.
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

/// Per-process rollup — the default observation granularity (Nextflow's
/// process-level monitor view; per-instance detail is the drill-down).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ProcessRunSummary {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelled: usize,
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
        while in_flight > 0 || !ready.is_empty() {
            // Dispatch while capacity remains.
            while let Some(id) = ready.pop_front() {
                if limit != 0 && in_flight >= limit {
                    ready.push_front(id);
                    break;
                }
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
                        &successors, &mut pending, &mut ready,
                    );
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
            for status in statuses.values_mut() {
                if matches!(status, RuntimeStatus::Pending | RuntimeStatus::Running) {
                    *status = RuntimeStatus::Cancelled;
                }
            }
        }
        for (id, status) in &statuses {
            let record = records.entry(id.clone()).or_insert_with(|| InstanceRecord {
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

        let ok = statuses
            .values()
            .all(|status| *status == RuntimeStatus::Success);
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
    fn complete(
        &self,
        plan: &ExecutionPlan,
        result: JobResult,
        statuses: &mut BTreeMap<InstanceId, RuntimeStatus>,
        outputs: &mut BTreeMap<InstanceId, PortOutputs>,
        records: &mut BTreeMap<InstanceId, InstanceRecord>,
        successors: &BTreeMap<InstanceId, BTreeSet<InstanceId>>,
        pending: &mut BTreeMap<InstanceId, usize>,
        ready: &mut VecDeque<InstanceId>,
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
                    None, duration,
                ));
                release_successors(&id, successors, pending, statuses, ready);
            }
            JobResult::Failed {
                id,
                error,
                duration,
                ..
            } => {
                let id = InstanceId::adopt(id);
                statuses.insert(id.clone(), RuntimeStatus::Failed);
                records.insert(id.clone(), record_for(
                    plan, &id, RuntimeStatus::Failed, BTreeMap::new(),
                    Some(error.to_string()), duration,
                ));
                cascade_skip(&id, successors, pending, statuses);
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

fn record_for(
    plan: &ExecutionPlan,
    id: &InstanceId,
    status: RuntimeStatus,
    outputs: BTreeMap<u8, NodeValue>,
    error: Option<String>,
    duration: std::time::Duration,
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
        outputs,
        error,
        elapsed_ms: duration.as_millis() as u64,
    }
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

/// Mark all transitive dependents of a failed instance as skipped.
fn cascade_skip(
    id: &InstanceId,
    successors: &BTreeMap<InstanceId, BTreeSet<InstanceId>>,
    pending: &mut BTreeMap<InstanceId, usize>,
    statuses: &mut BTreeMap<InstanceId, RuntimeStatus>,
) {
    let mut queue = VecDeque::from([id.clone()]);
    while let Some(current) = queue.pop_front() {
        for successor in successors.get(&current).cloned().unwrap_or_default() {
            if statuses.get(&successor) != Some(&RuntimeStatus::Pending) {
                continue;
            }
            statuses.insert(successor.clone(), RuntimeStatus::Skipped);
            pending.remove(&successor);
            queue.push_back(successor);
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

    /// Echo node: forwards each input file as `"{tag}:{input.path}"`; fails
    /// when an input path contains `fail_on`. A node without inputs emits its
    /// tag as a file.
    #[derive(Clone)]
    struct EchoNode {
        meta: dag_core::NodePorts,
        tag: String,
        fail_on: Option<String>,
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
            if let Some(needle) = &self.fail_on {
                if *needle == self.tag {
                    return Err(DagError::Schedule("echo failed by request".into()));
                }
            }
            let mut outputs = PortOutputs::new();
            for input in inputs {
                let NodeValue::File(file) = &input.data else {
                    continue;
                };
                outputs.insert(
                    input.port,
                    file_value(format!("{}:{}", self.tag, file.path)),
                );
            }
            if inputs.is_empty() {
                outputs.insert(0, file_value(self.tag.clone()));
            }
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

    fn flow(fail_on: Option<&str>) -> FlowGraphBuilder {
        let items = vec![serde_json::json!("a"), serde_json::json!("b")];
        FlowGraphBuilder::default()
            .process(Process::fanout(
                "qc",
                "echo",
                serde_json::json!({ "tag": "qc:{{item}}", "fail_on": fail_on }),
                Source::items(items.clone()),
                "sample",
            ))
            .process(Process::fanout(
                "align",
                "echo",
                serde_json::json!({ "tag": "align" }),
                Source::items(items),
                "sample",
            ))
            .process(Process::singleton(
                "multiqc",
                "echo",
                serde_json::json!({ "tag": "multiqc" }),
            ))
            .wire(Wire::direct("qc", 0, "align", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
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

    fn path_of(record: &InstanceRecord) -> String {
        match record.outputs.get(&0) {
            Some(NodeValue::File(file)) => file.path.clone(),
            other => panic!("expected a file output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pipeline_executes_with_grouped_report() {
        let graph = flow(None).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let report = scheduler()
            .run(
                &plan,
                &FlowRunConfig { max_concurrency: 2 },
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
        assert_eq!(path_of(align_a), "align:qc:a");
    }

    #[tokio::test]
    async fn failure_skips_only_its_own_dependents() {
        let graph = flow(Some("qc:a")).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let report = scheduler()
            .run(&plan, &FlowRunConfig::default(), None)
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
        let graph = flow(None).build().unwrap();
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
            .run(&plan, &FlowRunConfig::default(), Some(token))
            .await
            .unwrap();
        assert!(!report.ok);
        assert_eq!(report.processes["qc"].cancelled, 2);
        assert_eq!(report.processes["align"].cancelled, 2);
        assert_eq!(report.processes["multiqc"].cancelled, 1);
    }

    #[tokio::test]
    async fn broken_kind_fails_before_dispatch() {
        let graph = flow(None).build().unwrap();
        let plan = build_plan(&graph, &resolved(&graph)).unwrap();
        let broken = FlowScheduler::new(
            Arc::new(InlineExecutor),
            Arc::new(|kind, _| Err(crate::model::Error::Invalid(format!("unknown kind `{kind}`")).into())),
            engine_ctx(),
        );
        let error = broken
            .run(&plan, &FlowRunConfig::default(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown kind"), "{error}");
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
            .run(&plan, &FlowRunConfig::default(), None)
            .await
            .unwrap();
        assert!(report.ok);
        assert_eq!(report.processes["qc"].total, 0);
    }
}
