//! Scheduler behaviour: task-executor integration (timeouts, resource
//! serialization, input staging, artifact publication), event forwarding,
//! the memory guard, and re-run hygiene.

use std::sync::Arc;

use super::common::*;
use crate::dag::graph::{DAG, Result};
use crate::dag::node_event::{NodeEvent, NodeEventKind};
use crate::dag::runtime::{RuntimeStatus, SchedulerConfig};
use crate::dag::{
    LocalTaskExecutor, NodePorts, TaskAttemptReceipt, TaskInputSource, TaskResources,
    TaskSubmission,
};
use crate::resource::MemoryGuardConfig;
use crate::value::PortType;

#[derive(Clone)]
struct WaveTrackingNode {
    id: String,
    events: Arc<std::sync::Mutex<Vec<String>>>,
    ports: NodePorts,
}

#[async_trait::async_trait]
impl crate::dag::DagNode for WaveTrackingNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[crate::dag::NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<crate::dag::graph::PortOutputs> {
        {
            let mut events = self.events.lock().expect("wave event mutex");
            events.push(format!("start:{}", self.id));
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        {
            let mut events = self.events.lock().expect("wave event mutex");
            events.push(format!("finish:{}", self.id));
        }
        Ok(crate::dag::graph::PortOutputs::new())
    }

    fn clone_box(&self) -> Box<dyn crate::dag::DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "wave_tracking_test_node"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[tokio::test]
async fn idle_cache_release_preserves_failure_and_worker_evidence() -> Result<()> {
    let workspace_root = tempfile::tempdir().unwrap();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(LocalTaskExecutor::with_workspace_root(
        workspace_root.path(),
    )));
    dag.add_node_with_spec(
        "sleep".into(),
        Box::new(SleepingNode::new(std::time::Duration::from_secs(5))),
        "sleeping_test_node".into(),
        serde_json::json!({"seconds":5}),
    )?;
    dag.set_task_resources(
        "sleep",
        TaskResources {
            cpus: Some(1),
            memory_bytes: Some(1024),
            max_duration_ms: Some(10),
        },
    )?;
    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await?;
    assert!(!report.ok);
    let failed = report.nodes.iter().find(|n| n.id == "sleep").unwrap();
    assert!(
        failed
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("exceeded max_duration_ms")
    );
    assert!(dag.node_run_details.contains_key("sleep"));
    let details = serde_json::to_value(dag.node_run_details.get("sleep")).unwrap();
    dag.release_output_cache();
    dag.release_output_cache();
    assert_eq!(dag.status("sleep"), Some(RuntimeStatus::Failed));
    assert_eq!(
        serde_json::to_value(dag.node_run_details.get("sleep")).unwrap(),
        details
    );
    Ok(())
}

#[tokio::test]
async fn local_executor_enforces_task_timeout() -> Result<()> {
    let workspace_root = tempfile::tempdir().unwrap();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(LocalTaskExecutor::with_workspace_root(
        workspace_root.path(),
    )));
    dag.add_node_with_spec(
        "sleep".into(),
        Box::new(SleepingNode::new(std::time::Duration::from_secs(5))),
        "sleeping_test_node".into(),
        serde_json::json!({"seconds": 5}),
    )?;
    let resources = TaskResources {
        cpus: Some(1),
        memory_bytes: Some(1024),
        max_duration_ms: Some(10),
    };
    dag.set_task_resources("sleep", resources.clone())?;

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await?;
    assert!(!report.ok);
    let node = report.nodes.iter().find(|node| node.id == "sleep").unwrap();
    assert_eq!(node.resources, resources);
    assert!(
        node.error
            .as_ref()
            .unwrap()
            .message
            .contains("exceeded max_duration_ms"),
        "{node:?}"
    );
    let execution = node.execution.as_ref().unwrap();
    let workspace = execution.workspace.as_deref().unwrap();
    assert!(std::path::Path::new(workspace).starts_with(workspace_root.path()));
    let manifest = execution.task_manifest.as_ref().unwrap();
    assert!(std::path::Path::new(&manifest.path).is_file());
    let status = std::path::Path::new(workspace).join("status.json");
    let status_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(status).unwrap()).unwrap();
    assert_eq!(status_json["status"], "timeout");
    assert_eq!(execution.exit_code, Some(124));

    let manifest = dag.to_manifest();
    manifest.validate_layers()?;
    assert_eq!(
        manifest.logical.physical_task_resources.get("sleep"),
        Some(&resources)
    );
    Ok(())
}

#[tokio::test]
async fn local_executor_serializes_resource_requests() -> Result<()> {
    let workspace_root = tempfile::tempdir().unwrap();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(
        LocalTaskExecutor::with_workspace_root_and_resource_limits(
            workspace_root.path(),
            Some(1),
            Some(1024),
        )?,
    ));
    let trace = Arc::new(std::sync::Mutex::new(ResourceTrace::default()));
    for id in ["a", "b"] {
        dag.add_node_with_spec(
            id.into(),
            Box::new(ResourceTrackingNode::new(Arc::clone(&trace))),
            "resource_tracking_test_node".into(),
            serde_json::json!({"id": id}),
        )?;
        dag.set_task_resources(
            id,
            TaskResources {
                cpus: Some(1),
                memory_bytes: Some(1024),
                max_duration_ms: Some(5_000),
            },
        )?;
    }

    let report = dag
        .run(
            &SchedulerConfig {
                max_concurrency: 2,
                ..SchedulerConfig::default()
            },
            &test_ctx(),
            None,
        )
        .await?;
    assert!(report.ok, "{report:?}");
    assert_eq!(trace.lock().unwrap().max_active, 1);
    Ok(())
}

#[tokio::test]
async fn local_executor_stages_file_inputs_in_workspace() -> Result<()> {
    let source_dir = tempfile::tempdir().unwrap();
    let source_path = source_dir.path().join("source.txt");
    std::fs::write(&source_path, b"staged payload").unwrap();
    let workspace_root = tempfile::tempdir().unwrap();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(
        LocalTaskExecutor::with_workspace_root_resource_limits_and_input_staging(
            workspace_root.path(),
            Some(1),
            Some(1024),
        )?,
    ));
    dag.add_node_with_spec(
        "source".into(),
        Box::new(FileSourceNode::new(source_path.clone())),
        "file_source_test_node".into(),
        serde_json::json!({"path": source_path}),
    )?;
    let staged_paths = Arc::new(std::sync::Mutex::new(Vec::new()));
    dag.add_node_with_spec(
        "probe".into(),
        Box::new(InputPathProbeNode::new(Arc::clone(&staged_paths))),
        "input_path_probe_test_node".into(),
        serde_json::json!({}),
    )?;
    dag.add_edge("source", "probe", 0, 0)?;

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await?;
    assert!(report.ok, "{report:?}");
    let paths = staged_paths.lock().unwrap().clone();
    assert_eq!(paths.len(), 1);
    let staged_path = std::path::Path::new(&paths[0]);
    assert!(staged_path.starts_with(workspace_root.path()));
    assert_eq!(staged_path.parent().unwrap().file_name().unwrap(), "inputs");
    assert_eq!(std::fs::read(staged_path).unwrap(), b"staged payload");

    let probe = report.nodes.iter().find(|node| node.id == "probe").unwrap();
    let manifest_path = probe
        .execution
        .as_ref()
        .unwrap()
        .task_manifest
        .as_ref()
        .unwrap()
        .path
        .clone();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["spec"]["inputs"][0]["staged_paths"][0], paths[0]);
    Ok(())
}

#[tokio::test]
async fn local_executor_stages_and_publishes_dataframe_artifacts() -> Result<()> {
    let workspace_root = tempfile::tempdir().unwrap();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(
        LocalTaskExecutor::with_workspace_root_resource_limits_and_input_staging(
            workspace_root.path(),
            Some(1),
            Some(1024),
        )?,
    ));
    dag.add_node_with_spec(
        "source".into(),
        Box::new(EchoNode::default()),
        "echo".into(),
        serde_json::json!({}),
    )?;
    dag.add_node_with_spec(
        "target".into(),
        Box::new(EchoNode::from_ports(
            NodePorts::new()
                .add_input_port_of_type(None, PortType::DataFrame)
                .add_output_port_of_type(None, PortType::DataFrame),
        )),
        "echo".into(),
        serde_json::json!({}),
    )?;
    dag.add_edge("source", "target", 0, 0)?;

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await?;
    assert!(report.ok, "{report:?}");
    let source = report
        .nodes
        .iter()
        .find(|node| node.id == "source")
        .unwrap();
    let source_artifact = source
        .execution
        .as_ref()
        .unwrap()
        .output_artifacts
        .first()
        .unwrap();
    assert!(source_artifact.path.ends_with("outputs/port-0.arrow"));
    assert!(std::path::Path::new(&source_artifact.path).is_file());
    let receipt: TaskAttemptReceipt = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(
                source
                    .execution
                    .as_ref()
                    .unwrap()
                    .workspace
                    .as_deref()
                    .unwrap(),
            )
            .join("attempt.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt.task_id, "source");
    assert_eq!(receipt.executor, "local");
    assert_eq!(receipt.status, "success");
    assert_eq!(receipt.output_artifacts.len(), 1);
    assert_eq!(receipt.output_artifacts[0].path, source_artifact.path);

    let target = report
        .nodes
        .iter()
        .find(|node| node.id == "target")
        .unwrap();
    let target_manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            target
                .execution
                .as_ref()
                .unwrap()
                .task_manifest
                .as_ref()
                .unwrap()
                .path
                .clone(),
        )
        .unwrap(),
    )
    .unwrap();
    let staged_path = target_manifest["spec"]["inputs"][0]["staged_paths"][0]
        .as_str()
        .unwrap();
    assert!(staged_path.ends_with(".arrow"));
    assert!(std::path::Path::new(staged_path).is_file());
    Ok(())
}

#[test]
fn remote_dispatch_requires_staged_artifacts() {
    let input = |staged_paths: Vec<String>| crate::dag::TaskInputBinding {
        name: "frame".into(),
        port: 0,
        source: TaskInputSource::UpstreamPort {
            from: "source".into(),
            from_port: 0,
        },
        payload: "DataFrame".into(),
        path: None,
        fingerprint: None,
        staged_paths,
    };
    let submission = |input: crate::dag::TaskInputBinding| TaskSubmission {
        spec: crate::dag::TaskSpec {
            id: "remote".into(),
            logical_node: Some("process".into()),
            kind: "test".into(),
            spec: serde_json::json!({}),
            axis: None,
            item_key: None,
            item: None,
            inputs: vec![input],
            outputs: Vec::new(),
        },
        inputs: Vec::new(),
        resources: Default::default(),
    };

    let dispatch = submission(input(vec!["/work/input-0.arrow".into()]))
        .into_remote_dispatch()
        .unwrap();
    assert_eq!(dispatch.spec.id, "remote");
    let error = submission(input(Vec::new()))
        .into_remote_dispatch()
        .unwrap_err();
    assert!(error.to_string().contains("staged artifact"));
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

#[tokio::test]
async fn wave_size_creates_a_strict_barrier() -> Result<()> {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut dag = DAG::default();
    for id in ["one", "two", "three", "four"] {
        dag.add_node(
            id.into(),
            Box::new(WaveTrackingNode {
                id: id.to_string(),
                events: Arc::clone(&events),
                ports: NodePorts::new(),
            }),
        )?;
    }

    let report = dag
        .run(
            &SchedulerConfig {
                max_concurrency: 4,
                wave_size: Some(2),
                ..SchedulerConfig::default()
            },
            &test_ctx(),
            None,
        )
        .await?;
    assert!(report.ok, "{report:?}");
    let events = events.lock().expect("wave event mutex").clone();
    assert_eq!(report.waves.len(), 2, "{:?}", report.waves);
    assert_eq!(report.waves[0].node_ids.len(), 2);
    assert_eq!(report.waves[1].node_ids.len(), 2);
    let index = |needle: &str| events.iter().position(|event| event == needle).unwrap();
    let first_wave_finishes = report.waves[0]
        .node_ids
        .iter()
        .map(|id| index(&format!("finish:{id}")))
        .max()
        .expect("first wave has finishes");
    let second_wave_starts = report.waves[1]
        .node_ids
        .iter()
        .map(|id| index(&format!("start:{id}")))
        .min()
        .expect("second wave has starts");
    assert!(
        first_wave_finishes < second_wave_starts,
        "wave 1 must finish before wave 2 starts: {events:?}"
    );
    Ok(())
}

#[tokio::test]
async fn wave_scheduler_rejects_unpublished_evidence_before_downstream_dispatch() -> Result<()> {
    let root = tempfile::tempdir().unwrap();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(LocalTaskExecutor::with_workspace_root(
        root.path(),
    )));
    let source = FileOutputNode::new(root.path().join("source.tsv"));
    let source_runs = Arc::clone(&source.runs);
    dag.add_node_with_spec(
        "source".into(),
        Box::new(source),
        "file_output_test_node".into(),
        serde_json::json!({"role": "producer"}),
    )?;
    dag.add_node(
        "downstream".into(),
        Box::new(EchoNode::from_ports(
            NodePorts::new()
                .add_input_port_of_type(None, PortType::File)
                .add_output_port_of_type(None, PortType::File),
        )),
    )?;
    dag.add_edge("source", "downstream", 0, 0)?;
    let published = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&published);
    let report = dag
        .run(
            &SchedulerConfig {
                wave_size: Some(1),
                completion_publisher: Some(Arc::new(move |id, fingerprint, _, _| {
                    assert_eq!(fingerprint.len(), 64);
                    recorded.lock().unwrap().push(id.to_string());
                    Err(crate::dag::DagError::Schedule(
                        "durable evidence publication refused".into(),
                    ))
                })),
                ..SchedulerConfig::default()
            },
            &test_ctx(),
            None,
        )
        .await?;
    assert!(!report.ok);
    assert_eq!(source_runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(report.status("source"), Some(RuntimeStatus::Failed));
    assert_eq!(report.status("downstream"), Some(RuntimeStatus::Skipped));
    assert_eq!(*published.lock().unwrap(), ["source"]);
    assert!(dag.output("source").is_none());
    assert!(dag.output("downstream").is_none());
    assert!(
        report
            .nodes
            .iter()
            .find(|n| n.id == "source")
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("durable evidence publication refused")
    );
    Ok(())
}

#[tokio::test]
async fn wave_scheduler_passes_only_published_artifact_references_to_consumers() -> Result<()> {
    let root = tempfile::tempdir().unwrap();
    let published_path = root.path().join("private-published.tsv");
    let expected_path = published_path.to_string_lossy().into_owned();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(LocalTaskExecutor::with_workspace_root(
        root.path(),
    )));
    dag.add_node_with_spec(
        "source".into(),
        Box::new(FileOutputNode::new(root.path().join("scratch.tsv"))),
        "file_output_test_node".into(),
        serde_json::json!({"role": "producer"}),
    )?;
    dag.add_node(
        "downstream".into(),
        Box::new(EchoNode::from_ports(
            NodePorts::new()
                .add_input_port_of_type(None, PortType::File)
                .add_output_port_of_type(None, PortType::File),
        )),
    )?;
    dag.add_edge("source", "downstream", 0, 0)?;
    let published = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&published);
    let report = dag
        .run(
            &SchedulerConfig {
                wave_size: Some(1),
                completion_publisher: Some(Arc::new(move |id, _, outputs, _| {
                    recorded.lock().unwrap().push(id.to_string());
                    let file = outputs.get(&0).unwrap().as_file()?;
                    if id == "source" {
                        std::fs::copy(&file.path, &published_path)
                            .map_err(|e| crate::dag::DagError::Schedule(e.to_string()))?;
                        let mut result = crate::dag::graph::PortOutputs::new();
                        result.insert_file(0, crate::value::FileRef::local(&published_path, None)?);
                        Ok(result)
                    } else {
                        assert_eq!(file.path, expected_path);
                        assert_eq!(std::fs::read_to_string(&file.path).unwrap(), "run 1");
                        Ok(outputs)
                    }
                })),
                ..SchedulerConfig::default()
            },
            &test_ctx(),
            None,
        )
        .await?;
    assert!(report.ok, "{report:?}");
    assert_eq!(report.status("downstream"), Some(RuntimeStatus::Success));
    assert_eq!(*published.lock().unwrap(), ["source", "downstream"]);
    assert_eq!(report.waves.len(), 2);
    assert_eq!(
        dag.output("downstream")
            .unwrap()
            .get(&0)
            .unwrap()
            .as_file()?
            .path,
        root.path().join("private-published.tsv").to_string_lossy()
    );
    Ok(())
}

struct CoordinatorProbe {
    started: std::sync::atomic::AtomicUsize,
    active: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
    release: tokio::sync::Semaphore,
}

struct CoordinatorProbeGuard(Arc<CoordinatorProbe>);

impl Drop for CoordinatorProbeGuard {
    fn drop(&mut self) {
        self.0
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

struct CoordinatorProbeExecutor(Arc<CoordinatorProbe>);

#[async_trait::async_trait]
impl crate::dag::TaskExecutor for CoordinatorProbeExecutor {
    fn name(&self) -> &'static str {
        "coordinator-dispatch-probe"
    }

    async fn run(
        &self,
        mut execution: crate::dag::TaskExecution,
    ) -> crate::dag::node_event::JobResult {
        use crate::dag::node_event::JobResult;
        use std::sync::atomic::Ordering::SeqCst;
        let _active = if execution.node.kind() == "dynamic_item" {
            let active = self.0.active.fetch_add(1, SeqCst) + 1;
            let guard = CoordinatorProbeGuard(self.0.clone());
            self.0.peak.fetch_max(active, SeqCst);
            self.0.started.fetch_add(1, SeqCst);
            let permit = tokio::select! {
                permit = self.0.release.acquire() => permit.ok(),
                _ = execution.cancellation.cancelled() => None,
            };
            let Some(permit) = permit else {
                return JobResult::Failed {
                    id: execution.submission.spec.id,
                    error: crate::dag::DagError::Schedule("probe cancelled".into()),
                    duration: std::time::Duration::ZERO,
                    details: None,
                };
            };
            permit.forget();
            Some(guard)
        } else {
            None
        };
        let result = execution
            .node
            .execute(
                &execution.engine_ctx,
                &execution.submission.inputs,
                &execution.reporter,
            )
            .await;
        match result {
            Ok(outputs) => JobResult::Success {
                id: execution.submission.spec.id,
                outputs,
                duration: std::time::Duration::ZERO,
                details: None,
            },
            Err(error) => JobResult::Failed {
                id: execution.submission.spec.id,
                error,
                duration: std::time::Duration::ZERO,
                details: None,
            },
        }
    }
}

fn coordinator_probe() -> Arc<CoordinatorProbe> {
    Arc::new(CoordinatorProbe {
        started: 0.into(),
        active: 0.into(),
        peak: 0.into(),
        release: tokio::sync::Semaphore::new(0),
    })
}

async fn wait_for_coordinator_probe(probe: &CoordinatorProbe, count: usize) -> bool {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while probe.started.load(std::sync::atomic::Ordering::SeqCst) < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok()
}

fn independent_coordinator_probe_dag(probe: Arc<CoordinatorProbe>, count: usize) -> Result<DAG> {
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(CoordinatorProbeExecutor(probe)));
    for index in 0..count {
        dag.add_node_with_spec(
            format!("probe-{index}"),
            Box::new(DynamicItemNode::new(index.to_string())),
            "dynamic_item".into(),
            serde_json::json!({"value":index.to_string()}),
        )?;
    }
    Ok(dag)
}

#[tokio::test]
async fn zero_coordinator_limit_dispatches_sixteen_ready_nodes_while_explicit_limit_remains()
-> Result<()> {
    use std::sync::atomic::Ordering::SeqCst;
    for limit in [0, 2] {
        let probe = coordinator_probe();
        let mut dag = independent_coordinator_probe_dag(probe.clone(), 16)?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let run_cancel = cancel.clone();
        let run = tokio::spawn(async move {
            dag.run_with_external_cancel(
                &SchedulerConfig {
                    max_concurrency: limit,
                    memory_guard: None,
                    ..SchedulerConfig::default()
                },
                &test_ctx(),
                None,
                run_cancel,
            )
            .await
        });
        let expected = if limit == 0 { 16 } else { limit };
        let ready = wait_for_coordinator_probe(&probe, expected).await;
        if !ready {
            cancel.cancel();
        }
        probe.release.add_permits(16);
        let report = run.await.unwrap()?;
        assert!(
            ready,
            "limit {limit} did not admit {expected} independent ready nodes"
        );
        assert!(report.ok, "{report:?}");
        assert_eq!(probe.started.load(SeqCst), 16);
        assert_eq!(probe.peak.load(SeqCst), expected);
        assert_eq!(probe.active.load(SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn zero_coordinator_limit_applies_to_dynamic_fanout_beyond_initial_graph_size() -> Result<()>
{
    use crate::dag::{ChannelOperator, DagNode, LogicalGraph, LogicalNode};
    use std::sync::atomic::Ordering::SeqCst;
    let logical = LogicalGraph::builder()
        .add_node(LogicalNode::channel(
            "source",
            ChannelOperator::OfItems {
                items: (0..16)
                    .map(|index| serde_json::json!(index.to_string()))
                    .collect(),
            },
        ))
        .add_node(LogicalNode::dynamic_for_each(
            "process",
            "dynamic_item",
            serde_json::json!({"value":"{{item}}"}),
            "item",
        ))
        .add_node(LogicalNode::channel("collect", ChannelOperator::Collect))
        .add_edge("source", "process", 0, 0)
        .add_edge("process", "collect", 0, 0)
        .build();
    let physical = logical
        .compile(|_, _| unreachable!("planner builds dynamic/channel nodes"))
        .unwrap();
    let probe = coordinator_probe();
    let mut dag = DAG::default();
    dag.set_task_executor(Arc::new(CoordinatorProbeExecutor(probe.clone())));
    dag.set_dynamic_node_builder(Arc::new(|kind, spec| {
        assert_eq!(kind, "dynamic_item");
        Ok(Box::new(DynamicItemNode::new(
            spec["value"].as_str().unwrap().to_owned(),
        )) as Box<dyn DagNode>)
    }));
    dag.install_compiled_graph(logical, physical)?;
    let cancel = tokio_util::sync::CancellationToken::new();
    let run_cancel = cancel.clone();
    let run = tokio::spawn(async move {
        let report = dag
            .run_with_external_cancel(
                &SchedulerConfig {
                    max_concurrency: 0,
                    memory_guard: None,
                    ..SchedulerConfig::default()
                },
                &test_ctx(),
                None,
                run_cancel,
            )
            .await;
        (dag, report)
    });
    let ready = wait_for_coordinator_probe(&probe, 16).await;
    if !ready {
        cancel.cancel();
    }
    probe.release.add_permits(16);
    let (dag, report) = run.await.unwrap();
    let report = report?;
    assert!(
        ready,
        "all sixteen dynamically created items must enter before any is released"
    );
    assert!(report.ok, "{report:?}");
    assert_eq!(probe.peak.load(SeqCst), 16);
    assert_eq!(probe.active.load(SeqCst), 0);
    let crate::value::NodeValue::Channel(values) = &dag.output("collect#0").unwrap()[&0] else {
        panic!("dynamic gather did not produce a channel")
    };
    assert_eq!(values.items.len(), 16);
    Ok(())
}

#[tokio::test]
async fn optional_coordinator_gate_cancellation_releases_all_running_and_waiting_tasks()
-> Result<()> {
    use std::sync::atomic::Ordering::SeqCst;
    for limit in [0, 2] {
        let probe = coordinator_probe();
        let mut dag = independent_coordinator_probe_dag(probe.clone(), 16)?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let run_cancel = cancel.clone();
        let run = tokio::spawn(async move {
            dag.run_with_external_cancel(
                &SchedulerConfig {
                    max_concurrency: limit,
                    memory_guard: None,
                    ..SchedulerConfig::default()
                },
                &test_ctx(),
                None,
                run_cancel,
            )
            .await
        });
        let expected = if limit == 0 { 16 } else { limit };
        let ready = wait_for_coordinator_probe(&probe, expected).await;
        cancel.cancel();
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .unwrap()
            .unwrap()?;
        assert!(
            ready,
            "probe did not reach expected concurrency before cancellation"
        );
        assert!(!report.ok);
        assert_eq!(
            probe.active.load(SeqCst),
            0,
            "cancelled execution futures must be destroyed before returning"
        );
    }
    Ok(())
}
