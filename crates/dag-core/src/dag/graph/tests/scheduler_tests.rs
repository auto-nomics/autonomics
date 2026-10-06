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
