//! Node/edge mutation: add/delete/replace semantics, port wiring rules
//! enforced at insertion time, and resource-setter validation.

use std::assert_matches;

use super::common::*;
use crate::dag::error::DagError;
use crate::dag::graph::{DAG, PortOutputs};
use crate::dag::runtime::SchedulerConfig;
use crate::dag::{NodePorts, TaskResources};
use crate::value::PortType;

#[test]
fn cycle_rejected() {
    let mut dag = DAG::default();
    add(&mut dag, "x");
    add(&mut dag, "y");
    dag.add_edge("x", "y", 0, 0).unwrap();
    // Closing the cycle (y -> x) is rejected at add_edge time.
    let err = dag.add_edge("y", "x", 0, 0).unwrap_err();
    assert!(matches!(err, DagError::Cycle(_)), "{err:?}");
}

#[test]
fn self_loop_rejected() {
    // A self-edge (x -> x) is a trivial cycle — rejected at add_edge.
    let mut dag = DAG::default();
    add(&mut dag, "x");
    let err = dag.add_edge("x", "x", 0, 0).unwrap_err();
    assert!(matches!(err, DagError::Cycle(_)), "{err:?}");
}

#[test]
fn unknown_and_duplicate() {
    let mut dag = DAG::default();
    add(&mut dag, "a");
    // edge to missing node
    assert!(matches!(
        dag.add_edge("a", "ghost", 0, 0),
        Err(DagError::UnknownNode(_))
    ));
    // duplicate id
    assert!(matches!(
        dag.add_node("a".into(), Box::new(EchoNode::default())),
        Err(DagError::DuplicateNode(_))
    ));
}

#[test]
fn add_edge_rejects_unknown_output_port_immediately() {
    let mut dag = DAG::default();
    add(&mut dag, "x");
    add(&mut dag, "y");
    // EchoNode declares exactly one output port (0) — port 1 does not
    // exist and must be rejected at add_edge time, not silently stored.
    let err = dag.add_edge("x", "y", 1, 0).unwrap_err();
    assert_matches!(
        err,
        DagError::PortNotFound {
            node,
            port: 1,
            direction: "output",
        } if node == "x"
    );
}

#[test]
fn add_edge_rejects_unknown_input_port_on_fixed_nodes() {
    let mut dag = DAG::default();
    add(&mut dag, "x");
    dag.add_node(
        "fixed".into(),
        Box::new(EchoNode::from_ports(
            NodePorts::new().add_input_port(None).add_output_port(None),
        )),
    )
    .unwrap();
    let err = dag.add_edge("x", "fixed", 0, 1).unwrap_err();
    assert_matches!(
        err,
        DagError::PortNotFound {
            node,
            port: 1,
            direction: "input",
        } if node == "fixed"
    );
}

#[test]
fn add_edge_still_allows_undeclared_port_on_variadic_input() {
    // EchoNode::default has variadic input — wiring to an undeclared
    // port index remains legal (e.g. `sql` fan-in).
    let mut dag = DAG::default();
    add(&mut dag, "x");
    add(&mut dag, "y");
    dag.add_edge("x", "y", 0, 3).unwrap();
    let edges = dag.incoming_edges_with_ports("y");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].1.to_port, 3);
}

#[test]
fn one_to_multi_wiring_accept() {
    let mut dag = DAG::default();
    let node_a_ports = NodePorts::new().add_output_port(None);
    let node_b_ports = NodePorts::new().add_input_port(None);
    let node_c_ports = NodePorts::new().add_input_port(None);

    let node_a = PortedNode(node_a_ports);
    let node_b = PortedNode(node_b_ports);
    let node_c = PortedNode(node_c_ports);

    dag.add_node("node_a_id".into(), Box::new(node_a)).unwrap();
    dag.add_node("node_b_id".into(), Box::new(node_b)).unwrap();
    dag.add_node("node_c_id".into(), Box::new(node_c)).unwrap();

    // Fan-out: node_a's single output 0 feeds both node_b and node_c.
    // Each input port must end up with exactly one incoming edge.
    dag.add_edge("node_a_id", "node_b_id", 0, 0).unwrap();
    dag.add_edge("node_a_id", "node_c_id", 0, 0).unwrap();

    dag.validate_port_wiring().unwrap();
}

#[test]
fn variadic_node_allows_undeclared_optional_input_port() {
    // Declared ports stay required even when the input is variadic; the
    // undeclared extra port may be unwired, wired once, and is still
    // subject to the strict 1:1 rule once connected.
    let mut dag = DAG::default();
    for name in ["s0", "s1", "s2", "s3", "s4"] {
        dag.add_node(
            name.into(),
            Box::new(PortedNode(NodePorts::new().add_output_port(None))),
        )
        .unwrap();
    }
    let target_ports = NodePorts::new()
        .add_input_port(None)
        .add_input_port(None)
        .add_input_port(None)
        .set_fixed_input(false);
    dag.add_node("t".into(), Box::new(PortedNode(target_ports)))
        .unwrap();

    for i in 0u8..3 {
        dag.add_edge(format!("s{i}"), "t", 0, i).unwrap();
    }
    dag.validate_port_wiring().unwrap();

    dag.add_edge("s3", "t", 0, 3).unwrap();
    dag.validate_port_wiring().unwrap();

    dag.add_edge("s4", "t", 0, 3).unwrap();
    let err = dag.validate_port_wiring().unwrap_err();
    assert_matches!(
        err,
        DagError::PortOverconnected { node, port } if node == "t" && port == 3
    );
}

#[test]
fn multi_to_one_wiring_reject() {
    let mut dag = DAG::default();
    let node_a_ports = NodePorts::new().add_output_port(None);
    let node_b_ports = NodePorts::new().add_input_port(None);
    let node_c_ports = NodePorts::new().add_output_port(None);

    dag.add_node("node_a_id".into(), Box::new(PortedNode(node_a_ports)))
        .unwrap();
    dag.add_node("node_b_id".into(), Box::new(PortedNode(node_b_ports)))
        .unwrap();
    dag.add_node("node_c_id".into(), Box::new(PortedNode(node_c_ports)))
        .unwrap();

    // Two edges to the same declared input port 0 — rejected at add_edge.
    dag.add_edge("node_a_id", "node_b_id", 0, 0).unwrap();
    let err = dag.add_edge("node_c_id", "node_b_id", 0, 0).unwrap_err();
    assert_matches!(
        err,
        DagError::PortOverconnected { node, port }
            if node == "node_b_id" && port == 0
    );
}

#[test]
fn add_edge_rejects_overconnected_port() {
    // `add_edge` enforces the 1:1 rule on declared input ports at insertion
    // time. Two nodes (a, c) both trying to connect to b's declared input
    // port 0 — the second `add_edge` must reject immediately.
    let mut dag = DAG::default();
    let node_a_ports = NodePorts::new().add_output_port(None);
    let node_b_ports = NodePorts::new().add_input_port(None);
    let node_c_ports = NodePorts::new().add_output_port(None);

    dag.add_node("node_a_id".into(), Box::new(PortedNode(node_a_ports)))
        .unwrap();
    dag.add_node("node_b_id".into(), Box::new(PortedNode(node_b_ports)))
        .unwrap();
    dag.add_node("node_c_id".into(), Box::new(PortedNode(node_c_ports)))
        .unwrap();

    // First edge to b's declared input port 0 — OK.
    dag.add_edge("node_a_id", "node_b_id", 0, 0).unwrap();

    // Second edge to the same declared input port 0 — must reject at add_edge.
    let err = dag.add_edge("node_c_id", "node_b_id", 0, 0).unwrap_err();
    assert_matches!(
        err,
        DagError::PortOverconnected { node, port }
            if node == "node_b_id" && port == 0
    );
}

#[test]
fn add_edge_allows_undeclared_port_fan_in() {
    // When the target node has NO declared input ports (empty Ports), `add_edge`
    // does NOT enforce 1:1 — multiple edges to the same port index are allowed.
    // This is what happens in diamond DAGs with EchoNode (default meta has
    // empty Ports).
    let mut dag = DAG::default();
    for id in ["a", "b", "c", "d"] {
        add(&mut dag, id);
    }

    // d has no declared ports, so both edges to port 0 are accepted.
    dag.add_edge("b", "d", 0, 0).unwrap();
    dag.add_edge("c", "d", 0, 0).unwrap();

    // The diamond edges are all present.
    assert_eq!(dag.incoming_edges_with_ports("d").len(), 2);
}

#[test]
fn replace_node_preserves_edges() {
    let mut dag = get_diamond_dag(); // a→b, a→c, b→d, c→d
    // Replace node "b" with a new EchoNode (same port topology).
    let new_b = Box::new(EchoNode::default());
    dag.replace_node("b", new_b).unwrap();

    // All four nodes still present.
    assert_eq!(dag.node_ids().len(), 4);
    // Edge a→b preserved.
    assert_eq!(dag.predecessors("b"), vec!["a"]);
    assert_eq!(dag.successors("b"), vec!["d"]);
    // Edge b→d preserved.
    assert_eq!(dag.predecessors("d").len(), 2);
}

#[test]
fn replace_node_clears_stale_outputs() {
    let mut dag = DAG::default();
    add(&mut dag, "x");
    // Insert a fake output.
    dag.outputs.insert("x".to_string(), PortOutputs::new());
    assert!(dag.output("x").is_some());

    dag.replace_node("x", Box::new(EchoNode::default()))
        .unwrap();
    // Output must be cleared after replacement.
    assert!(dag.output("x").is_none());
}

#[test]
fn replace_node_unknown_id_rejected() {
    let mut dag = DAG::default();
    let err = dag
        .replace_node("ghost", Box::new(EchoNode::default()))
        .unwrap_err();
    assert!(matches!(err, DagError::UnknownNode(_)));
}

#[test]
fn replace_node_rejects_incompatible_port() {
    let mut dag = DAG::default();
    let out_schema = make_schema(&[("a", arrow_schema::DataType::Int32)]);
    let in_schema = make_schema(&[("a", arrow_schema::DataType::Int32)]);

    dag.add_node(
        "src".into(),
        Box::new(PortedNode(
            NodePorts::new().add_output_port(Some(out_schema)),
        )),
    )
    .unwrap();
    dag.add_node(
        "dst".into(),
        Box::new(PortedNode(
            NodePorts::new().add_input_port(Some(in_schema.clone())),
        )),
    )
    .unwrap();
    dag.add_edge("src", "dst", 0, 0).unwrap();

    // Replace "dst" with a node that has NO declared input ports → the
    // existing edge references port 0 which no longer exists.
    let no_ports = Box::new(PortedNode(NodePorts::new()));
    let err = dag.replace_node("dst", no_ports).unwrap_err();
    assert!(
        matches!(err, DagError::PortNotFound { ref node, port, .. } if node == "dst" && port == 0),
        "expected PortNotFound for dst:0, got {err:?}"
    );
}

// ── delete_edge tests ──────────────────────────────────────────────────

#[test]
fn delete_edge_removes_matching_edge() {
    let mut dag = get_diamond_dag();
    // Diamond: a→b(0,0), a→c(0,0), b→d(0,0), c→d(0,0)
    assert_eq!(dag.incoming_edges_with_ports("d").len(), 2);

    dag.delete_edge("b", "d", 0, 0).unwrap();

    let edges_d = dag.incoming_edges_with_ports("d");
    assert_eq!(edges_d.len(), 1);
    assert_eq!(edges_d[0].0, "c");
    // a→b, a→c still intact
    assert_eq!(dag.incoming_edges_with_ports("b").len(), 1);
    assert_eq!(dag.incoming_edges_with_ports("c").len(), 1);
}

#[test]
fn delete_edge_wrong_port_rejected() {
    let mut dag = DAG::default();
    add(&mut dag, "x");
    add(&mut dag, "y");
    dag.add_edge("x", "y", 0, 0).unwrap();

    let err = dag.delete_edge("x", "y", 0, 1).unwrap_err();
    assert!(
        matches!(err, DagError::EdgeNotFound { .. }),
        "expected EdgeNotFound, got {err:?}"
    );
    // Original edge still intact
    assert_eq!(dag.incoming_edges_with_ports("y").len(), 1);
}

#[test]
fn delete_edge_nonexistent_edge_rejected() {
    let mut dag = DAG::default();
    add(&mut dag, "x");
    add(&mut dag, "y");

    let err = dag.delete_edge("x", "y", 0, 0).unwrap_err();
    assert!(
        matches!(err, DagError::EdgeNotFound { .. }),
        "expected EdgeNotFound, got {err:?}"
    );
}

#[test]
fn delete_edge_unknown_node_rejected() {
    let mut dag = DAG::default();
    add(&mut dag, "x");

    let err = dag.delete_edge("x", "ghost", 0, 0).unwrap_err();
    assert!(matches!(err, DagError::UnknownNode(_)), "{err:?}");
}

#[test]
fn delete_edge_after_delete_node() {
    let mut dag = DAG::default();
    for id in ["a", "b", "c"] {
        add(&mut dag, id);
    }
    dag.add_edge("a", "b", 0, 0).unwrap();
    dag.add_edge("a", "c", 0, 0).unwrap();
    dag.delete_edge("a", "b", 0, 0).unwrap();
    assert_eq!(dag.incoming_edges_with_ports("c").len(), 1);

    // Now b has no incoming edges — safe to delete.
    dag.delete_node("b").unwrap();

    dbg!(dag.incoming_edges_with_ports("c"));
    // c is still connected to a.
    assert_eq!(dag.incoming_edges_with_ports("c").len(), 1);
}

#[tokio::test]
async fn multi_input_tmp_df_register() {
    let mut dag = DAG::default();
    dag.add_node(
        "a".into(),
        Box::new(EchoNode::from_ports(NodePorts::new().add_output_port(None))),
    )
    .unwrap();
    dag.add_node(
        "b".into(),
        Box::new(EchoNode::from_ports(NodePorts::new().add_output_port(None))),
    )
    .unwrap();
    dag.add_node(
        "c".into(),
        Box::new(EchoNode::from_ports(
            NodePorts::new()
                .add_input_port(None)
                .add_input_port(None)
                .set_fixed_input(true),
        )),
    )
    .unwrap();
    dag.add_edge("a", "c", 0, 0).unwrap();
    dag.add_edge("b", "c", 0, 1).unwrap();
    dag.validate().unwrap();
    assert_eq!(dag.node_ids().len(), 3);
    assert_eq!(dag.successors("a").len(), 1);
    assert_eq!(dag.predecessors("c").len(), 2);

    dag.run(&SchedulerConfig::default(), &test_ctx(), None)
        .await
        .unwrap();
    let output = dag.output("c").unwrap();
    dbg!(output);
}

#[test]
fn task_resources_reject_zero_requests() {
    let mut dag = DAG::default();
    dag.add_node_with_spec(
        "node".into(),
        Box::new(EchoNode::default()),
        "echo".into(),
        serde_json::json!({}),
    )
    .unwrap();
    let error = dag
        .set_task_resources(
            "node",
            TaskResources {
                cpus: Some(0),
                memory_bytes: None,
                max_duration_ms: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("cpus"));
}
