//! Run-report contents: per-port file assignments, input bindings, dispatch
//! ordering, and node-reported execution details.

use super::common::*;
use crate::dag::graph::DAG;
use crate::dag::runtime::{RuntimeStatus, SchedulerConfig};

#[tokio::test]
async fn run_report_assigns_files_by_declared_output_port() {
    let temp = tempfile::tempdir().unwrap();
    let mut dag = DAG::default();
    dag.add_node(
        "source".into(),
        Box::new(MultiFileOutputNode::new([
            temp.path().join("munged.sumstats.gz"),
            temp.path().join("munge_sumstats.log"),
        ])),
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

#[tokio::test]
async fn run_report_records_input_bindings() {
    let mut dag = DAG::default();
    add(&mut dag, "a");
    add(&mut dag, "b");
    dag.add_edge("a", "b", 0, 0).unwrap();

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await
        .unwrap();

    let node_a = report.nodes.iter().find(|node| node.id == "a").unwrap();
    let node_b = report.nodes.iter().find(|node| node.id == "b").unwrap();
    assert!(node_a.inputs.is_empty(), "source node has no bindings");
    assert_eq!(node_b.inputs.len(), 1);
    let binding = &node_b.inputs[0];
    assert_eq!(binding.from, "a");
    assert_eq!(binding.from_port, 0);
    assert_eq!(binding.to_port, 0);
    assert_eq!(binding.kind, "DataFrame");
    assert_eq!(
        binding.path, None,
        "DataFrame handles have no stable address"
    );
}

#[tokio::test]
async fn run_report_records_dispatch_sequence() {
    // A strict chain forces one dispatch order regardless of
    // concurrency: the recorded sequence must be a→b→c.
    let mut dag = DAG::default();
    add(&mut dag, "a");
    add(&mut dag, "b");
    add(&mut dag, "c");
    dag.add_edge("a", "b", 0, 0).unwrap();
    dag.add_edge("b", "c", 0, 0).unwrap();

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await
        .unwrap();

    let seq = |id: &str| {
        report
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap()
            .dispatch_seq
    };
    assert_eq!(seq("a"), Some(0));
    assert_eq!(seq("b"), Some(1));
    assert_eq!(seq("c"), Some(2));
}

#[tokio::test]
async fn node_report_carries_execution_details() {
    let mut dag = DAG::default();
    for (id, fail) in [("ok", false), ("bad", true)] {
        dag.add_node(id.into(), Box::new(DetailedNode::new(fail)))
            .unwrap();
    }

    let report = dag
        .run(&SchedulerConfig::default(), &test_ctx(), None)
        .await
        .unwrap();
    assert!(!report.ok, "the failing node marks the run as failed");

    let ok = report.nodes.iter().find(|node| node.id == "ok").unwrap();
    assert_eq!(ok.status, RuntimeStatus::Success);
    let details = ok
        .execution
        .as_ref()
        .expect("success node carries its execution details");
    assert_eq!(details.exit_code, Some(0));
    assert_eq!(details.image_digest.as_deref(), Some("sha256:abc"));
    assert_eq!(
        details.run_name.as_deref(),
        Some("autonomics-container-command-1-1")
    );
    assert!(
        details
            .stdout_log
            .as_ref()
            .unwrap()
            .path
            .ends_with("stdout.log")
    );
    assert!(
        details
            .stderr_log
            .as_ref()
            .unwrap()
            .path
            .ends_with("stderr.log")
    );
    assert!(std::path::Path::new(&details.stdout_log.as_ref().unwrap().path).is_file());
    assert!(std::path::Path::new(&details.stderr_log.as_ref().unwrap().path).is_file());

    let bad = report.nodes.iter().find(|node| node.id == "bad").unwrap();
    assert_eq!(bad.status, RuntimeStatus::Failed);
    let details = bad
        .execution
        .as_ref()
        .expect("failed node still carries its execution details");
    assert_eq!(details.exit_code, Some(42));
}
