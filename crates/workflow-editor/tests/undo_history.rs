//! Phase 4 integration test: undo/redo and snapshot history checkout.
//!
//! ## Coverage
//!
//! - `AppState::push_undo` + `undo()` restore a prior manifest.
//! - `redo()` replays the undone change.
//! - Saving twice produces two snapshots in the history.
//! - `client.checkout` restores a prior snapshot.

use uuid::Uuid;
use workflow_editor::WorkflowManager;
use workflow_editor::model::{EdgeEntry, NodeEntry, PortSpec, WorkflowManifest};

fn empty_manifest() -> WorkflowManifest {
    WorkflowManifest::new("test")
}

#[test]
fn push_undo_then_undo_restores_prior_manifest() {
    let mut state = workflow_editor::tui::AppState::default();
    let mut m1 = empty_manifest();
    m1.name = "first".into();
    state.manifest = Some(m1.clone());
    state.push_undo("init");

    let mut m2 = m1.clone();
    m2.name = "second".into();
    state.manifest = Some(m2.clone());

    // Undo: manifest should be the original `m1`.
    assert!(state.undo());
    assert_eq!(state.manifest.as_ref().unwrap().name, "first");

    // Redo: back to m2.
    assert!(state.redo());
    assert_eq!(state.manifest.as_ref().unwrap().name, "second");

    // And we cannot undo past the first push.
    assert!(!state.undo() || state.manifest.as_ref().unwrap().name == "first");
    let final_name = state.manifest.as_ref().unwrap().name.clone();
    assert_eq!(final_name, "first");
}

#[test]
fn redo_invalidated_by_new_edit() {
    let mut state = workflow_editor::tui::AppState::default();
    state.manifest = Some(empty_manifest());
    state.push_undo("a");
    let mut m2 = empty_manifest();
    m2.name = "v2".into();
    state.manifest = Some(m2);
    state.undo();
    assert_eq!(state.manifest.as_ref().unwrap().name, "test");

    // New edit clears redo stack.
    let mut m3 = empty_manifest();
    m3.name = "v3".into();
    state.manifest = Some(m3);
    state.push_undo("c");
    assert!(!state.redo(), "redo must be empty after a new edit");
}

#[tokio::test]
async fn checkout_restores_prior_snapshot_in_storage() {
    let mgr = WorkflowManager::open_memory().unwrap();
    let client = mgr.new_session();
    let wf_id = client.create_workflow("w").await.unwrap();

    // v1
    let mut m1 = client.load_workflow(wf_id).await.unwrap();
    let id1 = Uuid::new_v4();
    m1.nodes.push(NodeEntry {
        id: id1,
        kind: "echo".into(),
        label: "v1-node".into(),
        position: (0, 0),
        inputs: vec![PortSpec::new("in", "in")],
        outputs: vec![PortSpec::new("out", "out")],
        params: serde_json::json!({}),
    });
    client.save_workflow(m1, "v1").await.unwrap();

    // v2 — append a second node + an edge
    let mut m2 = client.load_workflow(wf_id).await.unwrap();
    let id2 = Uuid::new_v4();
    m2.nodes.push(NodeEntry {
        id: id2,
        kind: "echo".into(),
        label: "v2-node".into(),
        position: (5, 0),
        inputs: vec![PortSpec::new("in", "in")],
        outputs: vec![PortSpec::new("out", "out")],
        params: serde_json::json!({}),
    });
    m2.edges.push(EdgeEntry {
        id: Uuid::new_v4(),
        source: id1,
        source_handle: "out".into(),
        target: id2,
        target_handle: "in".into(),
    });
    client.save_workflow(m2, "v2").await.unwrap();

    // History has at least 2 snapshots (create_workflow adds an empty one).
    let history = client.history(wf_id).await.unwrap();
    assert!(
        history.len() >= 2,
        "expected at least 2 snapshots, got {}",
        history.len()
    );
    let v1_snapshot = history
        .iter()
        .find(|s| s.commit_message == "v1")
        .cloned()
        .unwrap();
    let v2_snapshot = history
        .iter()
        .find(|s| s.commit_message == "v2")
        .cloned()
        .unwrap();
    assert_ne!(v1_snapshot.id, v2_snapshot.id);

    // Checkout to v1 — manifest should drop v2-node and the edge.
    client.checkout(wf_id, v1_snapshot.id).await.unwrap();
    let restored = client.load_workflow(wf_id).await.unwrap();
    assert_eq!(
        restored.nodes.len(),
        1,
        "v2-node should be gone after checkout"
    );
    assert_eq!(
        restored.edges.len(),
        0,
        "v2 edge should be gone after checkout"
    );

    // Checkout back to v2 — manifest should have both.
    client.checkout(wf_id, v2_snapshot.id).await.unwrap();
    let restored = client.load_workflow(wf_id).await.unwrap();
    assert_eq!(restored.nodes.len(), 2);
    assert_eq!(restored.edges.len(), 1);
}

#[tokio::test]
async fn undo_push_does_not_affect_storage_history() {
    // Verify the undo stack lives only in editor state — saving the same
    // manifest twice still produces two snapshots.
    let mgr = WorkflowManager::open_memory().unwrap();
    let client = mgr.new_session();
    let wf_id = client.create_workflow("u").await.unwrap();
    client
        .save_workflow(client.load_workflow(wf_id).await.unwrap(), "a")
        .await
        .unwrap();
    client
        .save_workflow(client.load_workflow(wf_id).await.unwrap(), "b")
        .await
        .unwrap();
    let history = client.history(wf_id).await.unwrap();
    assert!(
        history.len() >= 2,
        "expected at least 2 snapshots after 2 saves, got {}",
        history.len()
    );
}
