//! Channel streaming: branch compilation, map-driven dynamic fanout, and
//! join finalization semantics.

use super::common::*;
use crate::dag::graph::{DAG, Result};
use crate::dag::runtime::{RuntimeStatus, SchedulerConfig};
use crate::dag::{ChannelBranch, ChannelOperator, DagNode};
use crate::value::NodeValue;

#[tokio::test]
async fn channel_branch_compiles_to_multiple_output_ports() -> Result<()> {
    let logical = crate::dag::LogicalGraph::builder()
        .add_node(crate::dag::LogicalNode::channel(
            "source",
            ChannelOperator::OfItems {
                items: vec![
                    serde_json::json!({"kind": "high"}),
                    serde_json::json!({"kind": "low"}),
                    serde_json::json!({"kind": "unknown"}),
                ],
            },
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "branch",
            ChannelOperator::Branch {
                branches: vec![
                    ChannelBranch {
                        name: "high".into(),
                        path: "kind".into(),
                        equals: Some(serde_json::json!("high")),
                        not_equals: None,
                        exists: None,
                        prefix: None,
                        suffix: None,
                        contains: None,
                    },
                    ChannelBranch {
                        name: "low".into(),
                        path: "kind".into(),
                        equals: Some(serde_json::json!("low")),
                        not_equals: None,
                        exists: None,
                        prefix: None,
                        suffix: None,
                        contains: None,
                    },
                ],
            },
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "collect_high",
            ChannelOperator::Collect,
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "collect_low",
            ChannelOperator::Collect,
        ))
        .add_edge("source", "branch", 0, 0)
        .add_edge("branch", "collect_high", 0, 0)
        .add_edge("branch", "collect_low", 1, 0)
        .build();
    let physical = logical
        .compile(|_, _| unreachable!("channel nodes are built by the planner"))
        .unwrap();
    let mut dag = DAG::default();
    dag.install_compiled_graph(logical, physical)?;
    assert_eq!(
        dag.get_node("branch#0")
            .unwrap()
            .ports()
            .output_port(1)
            .unwrap()
            .label,
        Some("low".into())
    );

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await?;
    assert!(report.ok, "{report:?}");
    let NodeValue::Channel(high) = &dag.output("collect_high#0").unwrap()[&0] else {
        panic!("high branch should emit a channel");
    };
    let NodeValue::Channel(low) = &dag.output("collect_low#0").unwrap()[&0] else {
        panic!("low branch should emit a channel");
    };
    assert_eq!(high.items, vec![serde_json::json!({"kind": "high"})]);
    assert_eq!(low.items, vec![serde_json::json!({"kind": "low"})]);
    dag.to_manifest().validate_layers()?;
    Ok(())
}

#[tokio::test]
async fn streaming_map_drives_dynamic_fanout() -> Result<()> {
    let logical = crate::dag::LogicalGraph::builder()
        .add_node(crate::dag::LogicalNode::channel(
            "source",
            ChannelOperator::OfItems {
                items: vec!["one".into(), "two".into()],
            },
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "mapped",
            ChannelOperator::Map {
                template: serde_json::json!("{{item}}"),
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
        .add_edge("source", "mapped", 0, 0)
        .add_edge("mapped", "process", 0, 0)
        .add_edge("process", "collect", 0, 0)
        .build();
    let physical = logical
        .compile(|_, _| unreachable!("dynamic and channel nodes are built by the planner"))
        .unwrap();
    let mut dag = DAG::default();
    dag.set_dynamic_node_builder(std::sync::Arc::new(move |kind, spec| {
        assert_eq!(kind, "dynamic_item");
        let value = spec["value"].as_str().unwrap().to_string();
        Ok(Box::new(DynamicItemNode::new(value)) as Box<dyn DagNode>)
    }));
    dag.install_compiled_graph(logical, physical).unwrap();

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await?;
    assert!(report.ok, "{report:?}");
    assert_eq!(
        report
            .logical_nodes
            .iter()
            .find(|summary| summary.logical_node == "process")
            .unwrap()
            .physical_job_count,
        2
    );
    Ok(())
}

#[tokio::test]
async fn streaming_join_waits_for_both_inputs_then_finalizes() -> Result<()> {
    let logical = crate::dag::LogicalGraph::builder()
        .add_node(crate::dag::LogicalNode::channel(
            "left",
            ChannelOperator::OfItems {
                items: vec![serde_json::json!({"id": "one", "value": 1})],
            },
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "right",
            ChannelOperator::OfItems {
                items: vec![serde_json::json!({"sample": "one", "score": 2})],
            },
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "joined",
            ChannelOperator::Join {
                left_key: "id".into(),
                right_key: "sample".into(),
            },
        ))
        .add_node(crate::dag::LogicalNode::channel(
            "output",
            ChannelOperator::Collect,
        ))
        .add_edge("left", "joined", 0, 0)
        .add_edge("right", "joined", 0, 1)
        .add_edge("joined", "output", 0, 0)
        .build();
    let physical = logical
        .compile(|_, _| unreachable!("channel nodes are built by the planner"))
        .unwrap();
    let mut dag = DAG::default();
    dag.install_compiled_graph(logical, physical)?;

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await?;
    assert!(report.ok, "{report:?}");
    assert_eq!(dag.status("left#0"), Some(RuntimeStatus::Success));
    assert_eq!(dag.status("right#0"), Some(RuntimeStatus::Success));
    assert_eq!(dag.status("joined#0"), Some(RuntimeStatus::Success));
    assert_eq!(dag.status("output#0"), Some(RuntimeStatus::Success));

    let NodeValue::Channel(channel) = &dag.output("output#0").unwrap()[&0] else {
        panic!("streaming join should produce a channel");
    };
    assert_eq!(channel.items.len(), 1);
    assert_eq!(channel.items[0]["left_id"], "one");
    assert_eq!(channel.items[0]["right_sample"], "one");
    assert_eq!(channel.items[0]["left_value"], 1);
    assert_eq!(channel.items[0]["right_score"], 2);
    Ok(())
}
