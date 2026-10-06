//! Topology queries, topological order, cycle rejection, dot rendering, and
//! the TUI snapshot.

use super::common::*;
use crate::dag::NodePorts;
use crate::dag::graph::DAG;
use crate::dag::runtime::RuntimeStatus;
use crate::value::PortType;

#[test]
fn tui_snapshot_is_stable_and_ports_are_preserved() {
    let mut dag = DAG::default();
    dag.add_node_with_spec(
        "b".to_string(),
        Box::new(EchoNode::from_ports(
            NodePorts::new()
                .set_fixed_input(false)
                .add_input_port_of_type_with_label(None, PortType::DataFrame, "frame"),
        )),
        "echo".into(),
        serde_json::json!({}),
    )
    .unwrap();
    dag.add_node_with_spec(
        "a".to_string(),
        Box::new(EchoNode::from_ports(
            NodePorts::new().add_output_port_of_type(None, PortType::DataFrame),
        )),
        "echo".into(),
        serde_json::json!({}),
    )
    .unwrap();
    dag.add_edge("a", "b", 0, 0).unwrap();

    let snapshot = dag.tui_snapshot();
    assert_eq!(
        snapshot
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert!(snapshot.nodes.iter().all(|node| node.kind == "echo"));
    assert!(snapshot.nodes.iter().all(|node| node.dirty));
    assert_eq!(snapshot.nodes[1].inputs[0].label.as_deref(), Some("frame"));
    assert_eq!(
        snapshot.nodes[0].outputs[0].data_type,
        PortType::DataFrame.to_string()
    );
    assert_eq!(
        snapshot
            .edges
            .iter()
            .map(|edge| (
                edge.from.as_str(),
                edge.from_port,
                edge.to.as_str(),
                edge.to_port
            ))
            .collect::<Vec<_>>(),
        vec![("a", 0, "b", 0)]
    );
    assert_eq!(
        snapshot.status_count(RuntimeStatus::Pending),
        snapshot.nodes.len()
    );
}

#[test]
fn topo_order_diamond() {
    let dag = get_diamond_dag();
    let order = dag.topo_order().unwrap();
    dbg!(&order);
    let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
    assert!(pos("a") < pos("b"));
    assert!(pos("a") < pos("c"));
    assert!(pos("b") < pos("d"));
    assert!(pos("c") < pos("d"));
}

#[test]
fn predecessors_and_incoming() {
    let mut dag = DAG::default();
    for id in ["src", "a", "b"] {
        add(&mut dag, id);
    }
    dag.add_edge("src", "a", 0, 0).unwrap();
    dag.add_edge("src", "b", 0, 0).unwrap();

    assert_eq!(dag.predecessors("a").len(), 1);
    assert_eq!(dag.predecessors("a")[0], "src");
    let mut succ = dag.successors("src");
    succ.sort_unstable();
    assert_eq!(succ, vec!["a", "b"]);
    let inc = dag.incoming_edges("a");
    assert_eq!(inc.len(), 1);
}

#[test]
fn default_edge_uses_default_ports() {
    let mut dag = DAG::default();
    for id in ["src", "a"] {
        add(&mut dag, id);
    }
    dag.add_edge("src", "a", 0, 0).unwrap();

    let edges = dag.incoming_edges_with_ports("a");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].0, "src");
    assert_eq!(edges[0].1.from_port, 0);
    assert_eq!(edges[0].1.to_port, 0);
}

#[test]
fn explicit_edge_ports() {
    let mut dag = DAG::default();
    dag.add_node(
        "x".into(),
        Box::new(EchoNode::from_ports(
            NodePorts::new().add_output_port(None).add_output_port(None),
        )),
    )
    .unwrap();
    add(&mut dag, "y");
    dag.add_edge("x", "y", 1, 0).unwrap();

    let edges = dag.incoming_edges_with_ports("y");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].1.from_port, 1);
    assert_eq!(edges[0].1.to_port, 0);
}

#[test]
fn diamond_edge_ports() {
    let dag = get_diamond_dag();
    // Diamond: a→b, a→c, b→d, c→d — all default ports.
    let edges_d = dag.incoming_edges_with_ports("d");
    assert_eq!(edges_d.len(), 2);
    let from_nodes: Vec<&str> = edges_d.iter().map(|(n, _)| n.as_str()).collect();
    assert!(from_nodes.contains(&"b"));
    assert!(from_nodes.contains(&"c"));
    for (_, e) in &edges_d {
        assert_eq!(e.from_port, 0);
        assert_eq!(e.to_port, 0);
    }
}

#[test]
fn render_into_dot() {
    let dag = get_diamond_dag();
    let dot = dag.to_dot();

    // DOT output must be a digraph declaration
    assert!(
        dot.contains("digraph"),
        "to_dot output should be a digraph declaration"
    );

    // All 4 diamond nodes must appear as labels in the output
    for node_id in ["a", "b", "c", "d"] {
        assert!(
            dot.contains(node_id),
            "DOT output should contain node '{node_id}'"
        );
    }

    // 4 edges: a→b, a→c, b→d, c→d — petgraph uses "N -> M" notation
    assert!(
        dot.contains("->"),
        "DOT output should contain directed edges"
    );

    // Smoke-check: non-trivial output (a 4-node DAG should be > 20 chars)
    assert!(dot.len() > 20, "DOT output seems too short, got: {dot}");
}
