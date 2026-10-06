//! Dynamic fanout: empty channels, job expansion + collection, manifest
//! round-trip restoration, and incremental reuse of an unchanged channel.

use std::sync::Arc;

use super::common::*;
use crate::dag::graph::{DAG, Result};
use crate::dag::node_event::{NodeEvent, NodeEventKind};
use crate::dag::runtime::SchedulerConfig;
use crate::dag::{ChannelOperator, DagNode, TaskInputSource, TaskResources};
use crate::value::NodeValue;

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
    dag.set_task_executor(Arc::new(RecordingTaskExecutor {
        submissions: Arc::clone(&task_submissions),
    }));
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
