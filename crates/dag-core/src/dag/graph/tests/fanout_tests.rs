//! Dynamic fanout: empty channels, job expansion + collection, manifest
//! round-trip restoration, and incremental reuse of an unchanged channel.

use std::sync::Arc;

use super::common::*;
use crate::dag::graph::{DAG, Result};
use crate::dag::node_event::{NodeEvent, NodeEventKind};
use crate::dag::runtime::{RuntimeStatus, SchedulerConfig};
use crate::dag::{ChannelOperator, DagNode, NodePorts, TaskInputSource, TaskResources};
use crate::value::{NodeValue, PortType};

#[tokio::test]
async fn dynamic_fanout_materializes_file_items_for_file_input_ports() -> Result<()> {
    let file_refs = vec![
        serde_json::json!({"path": "/tmp/a.tsv", "format": "tsv", "fingerprint": null}),
        serde_json::json!({"path": "/tmp/b.tsv", "format": null, "fingerprint": null}),
    ];
    for (item, expected_path, expected_format) in [
        (serde_json::json!("/tmp/a.tsv"), "/tmp/a.tsv", None),
        (file_refs[0].clone(), "/tmp/a.tsv", Some("tsv")),
    ] {
        let logical = crate::dag::LogicalGraph::builder()
            .add_node(crate::dag::LogicalNode::channel(
                "source",
                ChannelOperator::OfItems { items: vec![item] },
            ))
            .add_node(crate::dag::LogicalNode::dynamic_for_each(
                "process",
                "echo",
                serde_json::json!({}),
                "assay",
            ))
            .add_node(crate::dag::LogicalNode::channel(
                "collect",
                ChannelOperator::Collect,
            ))
            .add_edge("source", "process", 0, 0)
            .add_edge("process", "collect", 0, 0)
            .build();
        let physical = logical
            .compile(|_, _| unreachable!("dynamic and channel nodes are built by the planner"))
            .unwrap();
        let mut dag = DAG::default();
        dag.set_dynamic_node_builder(Arc::new(|kind, _| {
            assert_eq!(kind, "echo");
            Ok(Box::new(EchoNode::from_ports(
                NodePorts::new()
                    .add_input_port_of_type(None, PortType::File)
                    .add_output_port_of_type(None, PortType::File),
            )) as Box<dyn DagNode>)
        }));
        dag.install_compiled_graph(logical, physical)?;

        let report = dag
            .run(&SchedulerConfig::default(), &test_ctx(), None)
            .await?;
        assert!(report.ok, "{report:?}");
        let NodeValue::Channel(channel) = &dag.output("collect#0").unwrap()[&0] else {
            panic!("collect should emit a channel");
        };
        assert_eq!(channel.items.len(), 1);
        assert_eq!(channel.items[0]["path"], expected_path);
        assert_eq!(
            channel.items[0]["format"],
            serde_json::json!(expected_format)
        );
        let binding = &report
            .nodes
            .iter()
            .find(|node| node.id.starts_with("process#assay="))
            .unwrap()
            .inputs[0];
        assert_eq!(binding.kind, "File");
        assert_eq!(binding.path, Some(expected_path.into()));
    }
    Ok(())
}

#[derive(Clone)]
struct ResumableDynamicItemNode {
    value: String,
    attempts: Arc<std::sync::atomic::AtomicUsize>,
    failures: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    ports: NodePorts,
}

#[async_trait::async_trait]
impl DagNode for ResumableDynamicItemNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[crate::dag::NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> std::result::Result<crate::dag::graph::PortOutputs, crate::dag::error::DagError> {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self
            .failures
            .lock()
            .expect("failure set mutex")
            .contains(&self.value)
        {
            return Err(crate::dag::error::DagError::Schedule(format!(
                "intentional failure for `{}`",
                self.value
            )));
        }

        let mut outputs = crate::dag::graph::PortOutputs::new();
        outputs.insert(
            0,
            crate::value::FileRef::new(format!("/{}.txt", self.value), Some("txt".into())),
        );
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "resumable_dynamic_item"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[tokio::test]
async fn dynamic_channel_fanout_supports_empty_channels() -> Result<()> {
    let logical = crate::dag::LogicalGraph::builder()
        .add_node(crate::dag::LogicalNode::channel(
            "source",
            ChannelOperator::OfItems { items: Vec::new() },
        ))
        .add_node(crate::dag::LogicalNode::dynamic_for_each(
            "process",
            "dynamic_item",
            serde_json::json!({"value": "{{item}}"}),
            "sample",
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "collect",
            ChannelOperator::Collect,
        ))
        .add_edge("source", "process", 0, 0)
        .add_edge("process", "collect", 0, 0)
        .build();
    let physical = logical
        .compile(|_, _| unreachable!("dynamic and channel nodes are built by the planner"))
        .unwrap();
    let mut dag = DAG::default();
    dag.set_dynamic_node_builder(Arc::new(|kind, _| {
        panic!("builder should not be called for kind `{kind}`")
    }));
    dag.install_compiled_graph(logical, physical)?;

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await?;
    assert!(report.ok, "{report:?}");
    assert!(!dag.nodes.contains_key("process#sample=item-000000"));
    let NodeValue::Channel(channel) = &dag.output("collect#0").unwrap()[&0] else {
        panic!("collect should emit a channel");
    };
    assert!(channel.items.is_empty());
    assert_eq!(
        report
            .logical_nodes
            .iter()
            .find(|summary| summary.logical_node == "process")
            .unwrap()
            .physical_job_count,
        0
    );
    dag.to_manifest().validate_layers()?;
    Ok(())
}

#[tokio::test]
async fn incremental_dynamic_fanout_reuses_successful_items() -> Result<()> {
    let logical = crate::dag::LogicalGraph::builder()
        .add_node(crate::dag::LogicalNode::channel(
            "source",
            ChannelOperator::OfItems {
                items: vec!["good-one".into(), "good-two".into(), "bad".into()],
            },
        ))
        .add_node(crate::dag::LogicalNode::dynamic_for_each(
            "process",
            "resumable_dynamic_item",
            serde_json::json!({"value": "{{item}}"}),
            "item",
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "collect",
            ChannelOperator::Collect,
        ))
        .add_edge("source", "process", 0, 0)
        .add_edge("process", "collect", 0, 0)
        .build();
    let physical = logical
        .compile(|_, _| unreachable!("dynamic fanout builder is deferred"))
        .unwrap();

    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failures = Arc::new(std::sync::Mutex::new(
        ["bad".to_string()]
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
    ));
    let mut dag = DAG::default();
    dag.set_dynamic_node_builder({
        let attempts = Arc::clone(&attempts);
        let failures = Arc::clone(&failures);
        Arc::new(move |kind, spec| {
            assert_eq!(kind, "resumable_dynamic_item");
            let value = spec["value"].as_str().expect("rendered item").to_string();
            Ok(Box::new(ResumableDynamicItemNode {
                value,
                attempts: Arc::clone(&attempts),
                failures: Arc::clone(&failures),
                ports: NodePorts::new().add_output_port_of_type(None, PortType::Any),
            }) as Box<dyn DagNode>)
        })
    });
    dag.install_compiled_graph(logical, physical)?;

    let cfg = SchedulerConfig {
        wave_size: Some(1),
        ..incremental_cfg()
    };
    let first = dag.run(&cfg, &test_ctx(), None).await.unwrap();
    assert!(!first.ok, "{first:?}");
    assert_eq!(
        attempts.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "first attempt should execute every item: {first:?}"
    );
    let process = first
        .logical_nodes
        .iter()
        .find(|summary| summary.logical_node == "process")
        .expect("process summary");
    assert_eq!(process.failed_item_keys, ["bad"], "{process:?}");
    assert!(process.reused_item_keys.is_empty(), "{process:?}");
    assert!(
        first
            .waves
            .iter()
            .any(|wave| wave.failed == 1 && wave.failed_item_keys == ["bad"]),
        "failed item should be grouped by wave: {:?}",
        first.waves
    );

    failures.lock().expect("failure set mutex").remove("bad");
    let second = dag.run(&cfg, &test_ctx(), None).await.unwrap();
    assert!(second.ok, "{second:?}");
    assert_eq!(
        attempts.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "resume should execute only the failed item: {second:?}"
    );
    let resumed = second
        .logical_nodes
        .iter()
        .find(|summary| summary.logical_node == "process")
        .expect("process summary");
    assert_eq!(
        resumed.failed_item_keys,
        Vec::<String>::new(),
        "{resumed:?}"
    );
    assert_eq!(
        resumed.reused_item_keys,
        ["good-one", "good-two"],
        "{resumed:?}"
    );
    assert!(
        second.nodes.iter().any(|node| node.id.ends_with("bad")
            && node.status == RuntimeStatus::Success
            && !node.reused),
        "failed item should execute successfully on resume: {:?}",
        second.nodes
    );
    Ok(())
}

#[tokio::test]
async fn dynamic_channel_fanout_expands_and_collects_jobs() -> Result<()> {
    let logical = crate::dag::LogicalGraph::builder()
        .add_node(crate::dag::LogicalNode::channel(
            "source",
            ChannelOperator::OfItems {
                items: vec!["a".into(), "b".into()],
            },
        ))
        .add_node(crate::dag::LogicalNode::dynamic_for_each(
            "process",
            "dynamic_item",
            serde_json::json!({"value": "{{item}}"}),
            "sample",
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "collect",
            ChannelOperator::Collect,
        ))
        .add_edge("source", "process", 0, 0)
        .add_edge("process", "collect", 0, 0)
        .build();
    let physical = logical
        .compile(|_, _| unreachable!("dynamic and channel nodes are built by the planner"))
        .unwrap();
    let mut dag = DAG::default();
    let executions = Arc::new(std::sync::Mutex::new(0usize));
    let builder_executions = Arc::clone(&executions);
    dag.set_dynamic_node_builder(Arc::new(move |kind, spec| {
        assert_eq!(kind, "dynamic_item");
        *builder_executions.lock().unwrap() += 1;
        let value = spec["value"].as_str().unwrap().to_string();
        Ok(Box::new(DynamicItemNode::new(value)) as Box<dyn DagNode>)
    }));
    let task_submissions = Arc::new(std::sync::Mutex::new(Vec::new()));
    dag.set_task_executor(Arc::new(RecordingTaskExecutor::with_submissions(
        Arc::clone(&task_submissions),
    )));
    let process_resources = TaskResources {
        cpus: Some(2),
        memory_bytes: Some(2048),
        max_duration_ms: Some(60_000),
    };
    dag.set_logical_task_resources("process", process_resources.clone())?;
    dag.install_compiled_graph(logical, physical).unwrap();

    let (_event_tx, mut event_rx) = tokio::sync::mpsc::channel::<NodeEvent>(128);
    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), Some(_event_tx))
        .await
        .unwrap();
    assert!(report.ok, "{report:?}");
    let mut finish_order = Vec::new();
    while let Some(event) = event_rx.recv().await {
        if matches!(event.kind, NodeEventKind::Finished { .. }) {
            finish_order.push(event.node_id.clone());
        }
    }
    let first_process = finish_order
        .iter()
        .position(|id| id.starts_with("process#sample="))
        .expect("dynamic process should finish");
    let source_position = finish_order
        .iter()
        .position(|id| id == "source#0")
        .expect("stream source should finish");
    assert!(
        first_process < source_position,
        "dynamic job should finish before its streaming source closes: {finish_order:?}"
    );
    assert_eq!(*executions.lock().unwrap(), 2);
    assert!(dag.nodes.contains_key("process#sample=a"));
    assert!(dag.nodes.contains_key("process#sample=b"));
    let NodeValue::Channel(channel) = &dag.output("collect#0").unwrap()[&0] else {
        panic!("collect should emit a channel");
    };
    assert_eq!(channel.items.len(), 2);
    assert!(
        channel
            .items
            .iter()
            .any(|item| { item.get("path").and_then(|path| path.as_str()) == Some("/a.txt") })
    );

    let manifest = dag.to_manifest();
    manifest.validate_layers().unwrap();
    assert_eq!(
        manifest
            .physical_jobs
            .values()
            .filter(|job| job.logical_node == "process" && job.item_key.is_some())
            .count(),
        2
    );
    let process_summary = report
        .logical_nodes
        .iter()
        .find(|summary| summary.logical_node == "process")
        .unwrap();
    assert_eq!(process_summary.physical_job_count, 2);
    let task_submissions = task_submissions.lock().unwrap();
    let process_submission = task_submissions
        .iter()
        .find(|submission| submission.spec.id == "process#sample=a")
        .unwrap();
    let process_spec = &process_submission.spec;
    assert_eq!(process_spec.logical_node.as_deref(), Some("process"));
    assert_eq!(process_spec.item, Some(serde_json::json!("a")));
    assert_eq!(
        process_spec.inputs[0].source,
        TaskInputSource::DynamicFanoutItem {
            axis: "sample".into()
        }
    );
    assert_eq!(process_spec.outputs.len(), 1);
    assert_eq!(process_spec.outputs[0].name, "port_0");
    assert_eq!(process_spec.outputs[0].payload, "any");
    assert_eq!(process_submission.resources, process_resources);
    assert_eq!(
        report
            .nodes
            .iter()
            .find(|node| node.id == "process#sample=a")
            .unwrap()
            .executor,
        Some("recording")
    );
    let collect_spec = task_submissions
        .iter()
        .find(|submission| submission.spec.id == "collect#0")
        .map(|submission| &submission.spec)
        .unwrap();
    assert!(collect_spec.inputs.iter().any(|input| matches!(
        &input.source,
        TaskInputSource::UpstreamPort {
            from,
            from_port: 0,
        } if from.starts_with("process#sample=")
    )));

    let incremental_report = dag
        .run(
            &SchedulerConfig {
                incremental: true,
                ..SchedulerConfig::default()
            },
            &test_ctx(),
            None,
        )
        .await?;
    assert!(incremental_report.ok, "{incremental_report:?}");
    assert_eq!(*executions.lock().unwrap(), 2);

    let mut restored = DAG::default();
    restored.set_dynamic_node_builder(Arc::new(|kind, spec| {
        assert_eq!(kind, "dynamic_item");
        let value = spec["value"].as_str().unwrap().to_string();
        Ok(Box::new(DynamicItemNode::new(value)) as Box<dyn DagNode>)
    }));
    for entry in &manifest.nodes {
        let node = if entry.kind == "channel" {
            Box::new(crate::dag::ChannelNode::from_spec(&entry.spec)?) as Box<dyn DagNode>
        } else if entry.kind == "dynamic_fanout" {
            Box::new(crate::dag::DynamicFanoutNode::default()) as Box<dyn DagNode>
        } else {
            restored.dynamic_node_builder.as_ref().unwrap()(&entry.kind, entry.spec.clone())?
        };
        restored.add_node_with_spec(
            entry.id.clone(),
            node,
            entry.kind.clone(),
            entry.spec.clone(),
        )?;
    }
    for edge in &manifest.edges {
        restored.add_edge(
            edge.from.clone(),
            edge.to.clone(),
            edge.from_port,
            edge.to_port,
        )?;
    }
    restored.restore_layers(
        manifest.logical.graphs.clone(),
        manifest.physical_jobs.clone(),
    )?;
    let restored_report = restored
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await
        .unwrap();
    assert!(restored_report.ok, "{restored_report:?}");
    assert_eq!(
        restored_report
            .logical_nodes
            .iter()
            .find(|summary| summary.logical_node == "process")
            .unwrap()
            .physical_job_count,
        2,
        "nodes={:?}",
        report.nodes,
    );
    Ok(())
}
