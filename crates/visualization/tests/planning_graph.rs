use serde_json::json;
use visualization::{Store, planning::Brief};

#[test]
fn declared_topology_exposes_disconnected_signal_without_rewriting_or_blocking_comparisons() {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Topology").unwrap();
    let figure = store
        .create("p", "Review", "excalidraw/v2", &json!({"elements":[]}))
        .unwrap();
    let brief: Brief = serde_json::from_value(json!({"message":"Compare two independent processes","audience":"Researchers","nodes":[{"id":"ligand","label":"Ligand"},{"id":"receptor","label":"Activated receptor"},{"id":"substrate","label":"Substrate"},{"id":"phospho","label":"Modified substrate"},{"id":"nuclear","label":"Nuclear complex"},{"id":"control","label":"Independent control"}],"edges":[{"from":"ligand","to":"receptor"},{"from":"substrate","to":"phospho"},{"from":"phospho","to":"nuclear"}]})).unwrap();
    let original = serde_json::to_value(&brief).unwrap();
    let saved = store.save_planning("p", &figure.id, 1, 0, &brief).unwrap();
    assert_eq!(saved.status, "ready", "parallel processes are permitted");
    let graph = &saved.graph_review;
    assert_eq!(
        graph.weak_components,
        vec![
            vec!["ligand", "receptor"],
            vec!["substrate", "phospho", "nuclear"],
            vec!["control"]
        ]
    );
    assert_eq!(graph.source_node_ids, ["ligand", "substrate"]);
    assert_eq!(graph.sink_node_ids, ["receptor", "nuclear"]);
    assert_eq!(graph.isolated_node_ids, ["control"]);
    assert_eq!(graph.verification, "not_verified");
    assert_eq!(graph.scope, "declared_graph_only");
    assert_eq!(serde_json::to_value(&saved.document).unwrap(), original);
    assert_eq!(
        store
            .planning("p", &figure.id)
            .unwrap()
            .unwrap()
            .graph_review,
        *graph
    );
    assert_eq!(
        serde_json::to_value(
            store
                .planning_revision("p", &figure.id, 1)
                .unwrap()
                .document
        )
        .unwrap(),
        original
    );
    store
        .planned_proposal("p", &figure.id, 1, 1, "Explicit parallel processes")
        .unwrap();
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
}

#[test]
fn cycles_bidirectional_undirected_and_invalid_edges_are_explicit() {
    let brief: Brief = serde_json::from_value(json!({"nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"},{"id":"c","label":"C"},{"id":"d","label":"D"}],"edges":[{"from":"a","to":"b","direction":"bidirectional"},{"from":"b","to":"c","direction":"undirected"},{"from":"c","to":"a"},{"from":"d","to":"missing"}]})).unwrap();
    let graph = brief.graph_review();
    assert_eq!(graph.weak_components, vec![vec!["a", "b", "c"], vec!["d"]]);
    assert!(graph.source_node_ids.is_empty());
    assert!(graph.sink_node_ids.is_empty());
    assert_eq!(graph.isolated_node_ids, ["d"]);
    assert_eq!(graph.invalid_edge_indices, [3]);
    assert!(Brief::default().graph_review().weak_components.is_empty());
}
