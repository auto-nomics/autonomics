//! Integration tests for the storage layer (Phase 0).
//!
//! Run with `cargo test -p workflow-editor --test phase0_store`.

use tempfile::tempdir;
use workflow_editor::model::{EdgeEntry, NodeEntry, PortSpec, Skill, WorkflowManifest};
use workflow_editor::store::{self, NodeKindRepo, SkillRepo, WorkflowRepo};

fn pool() -> store::DbPool {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let pool = store::open(&path).unwrap();
    // Keep the TempDir alive for the duration of the test by leaking it —
    // simpler than threading an Arc<TempDir> through every test.
    std::mem::forget(dir);
    store::run(&pool).unwrap();
    pool
}

#[test]
fn migrations_run_on_fresh_db() {
    let _p = pool();
}

#[test]
fn migrations_are_idempotent() {
    let p = pool();
    store::run(&p).unwrap();
}

#[test]
fn create_and_get_workflow() {
    let p = pool();
    let repo = WorkflowRepo::new(p);
    let m = WorkflowManifest::new("hello");
    let id = repo.create(&m).unwrap();
    let back = repo.get(id).unwrap();
    assert_eq!(back.name, "hello");
    assert_eq!(back.id, m.id);
}

#[test]
fn save_appends_snapshot() {
    let p = pool();
    let repo = WorkflowRepo::new(p);
    let mut m = WorkflowManifest::new("snap");
    let id = repo.create(&m).unwrap();

    m.name = "snap-renamed".into();
    repo.save(id, &m, "rename").unwrap();

    let hist = repo.history(id).unwrap();
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0].commit_message, "rename");
    assert_eq!(hist[1].commit_message, "initial");
}

#[test]
fn list_workflows_returns_summary() {
    let p = pool();
    let repo = WorkflowRepo::new(p);
    repo.create(&WorkflowManifest::new("a")).unwrap();
    repo.create(&WorkflowManifest::new("b")).unwrap();
    let list = repo.list().unwrap();
    assert_eq!(list.len(), 2);
}

#[test]
fn checkout_restores_manifest() {
    let p = pool();
    let repo = WorkflowRepo::new(p);
    let mut m = WorkflowManifest::new("ck");
    let id = repo.create(&m).unwrap();
    m.name = "ck-renamed".into();
    let new_snap = repo.save(id, &m, "rename").unwrap();

    m.name = "ck".into();
    repo.save(id, &m, "back-to-ck").unwrap();

    repo.checkout(id, new_snap).unwrap();
    let back = repo.get(id).unwrap();
    assert_eq!(back.name, "ck-renamed");
}

#[test]
fn save_skill_versions_correctly() {
    let p = pool();
    let repo = SkillRepo::new(p);
    let s = Skill::new("summarize", WorkflowManifest::new("inner"));
    let v1 = repo.save(&s).unwrap();
    assert_eq!(v1.version, 1);

    let v2 = repo.save(&s).unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(v1.id, v2.id);

    let list = repo.list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "summarize");
    assert_eq!(list[0].version, 2);
}

#[test]
fn node_kind_repo_upsert_and_list() {
    let p = pool();
    let repo = NodeKindRepo::new(p);
    repo.upsert(
        "echo",
        "Echo",
        "Echo the input back.",
        &serde_json::json!({"type": "object"}),
        &[PortSpec::new("in", "input"), PortSpec::new("out", "output")],
        "Echoes its input.",
        "Test",
    )
    .unwrap();
    let kinds = repo.list().unwrap();
    assert_eq!(kinds.len(), 1);
    assert_eq!(kinds[0].kind, "echo");

    let schema = repo.spec_schema("echo").unwrap();
    assert!(schema.is_some());
}

#[test]
fn save_rejects_invalid_manifest() {
    let p = pool();
    let repo = WorkflowRepo::new(p);
    let mut m = WorkflowManifest::new("bad");
    let n_id = uuid::Uuid::new_v4();
    m.nodes.push(NodeEntry {
        id: n_id,
        kind: "echo".into(),
        label: "n".into(),
        position: (0, 0),
        inputs: vec![PortSpec::new("in", "in")],
        outputs: vec![PortSpec::new("out", "out")],
        params: serde_json::json!({}),
    });
    m.edges.push(EdgeEntry {
        id: uuid::Uuid::new_v4(),
        source: n_id,
        source_handle: "out".into(),
        target: n_id,
        target_handle: "in".into(),
    });
    let id = repo.create(&m).unwrap();
    let err = repo.save(id, &m, "still bad");
    assert!(err.is_err());
}

#[test]
fn full_round_trip_workflow() {
    // Build a non-trivial workflow: 3 nodes, 2 edges, set viewport + SOP,
    // save it, reload it, and verify every field survived.
    let p = pool();
    let repo = WorkflowRepo::new(p);

    let mut m = WorkflowManifest::new("trip");
    m.viewport = workflow_editor::model::Viewport {
        pan: (40, 12),
        grid_step: 2,
    };
    m.sop = Some("Be terse.".into());

    let n1 = NodeEntry {
        id: uuid::Uuid::new_v4(),
        kind: "http_request".into(),
        label: "Fetch".into(),
        position: (10, 4),
        inputs: vec![PortSpec::new("trigger", "trigger")],
        outputs: vec![PortSpec::new("body", "body")],
        params: serde_json::json!({"url": "https://example.com"}),
    };
    let n2 = NodeEntry {
        id: uuid::Uuid::new_v4(),
        kind: "llm_prompt".into(),
        label: "Summarize".into(),
        position: (30, 4),
        inputs: vec![PortSpec::new("prompt", "prompt")],
        outputs: vec![PortSpec::new("text", "text")],
        params: serde_json::json!({"model": "claude-sonnet"}),
    };
    let n3 = NodeEntry {
        id: uuid::Uuid::new_v4(),
        kind: "echo".into(),
        label: "Print".into(),
        position: (50, 4),
        inputs: vec![PortSpec::new("in", "in")],
        outputs: vec![PortSpec::new("out", "out")],
        params: serde_json::json!({}),
    };
    m.nodes.extend([n1.clone(), n2.clone(), n3.clone()]);
    m.edges.push(EdgeEntry {
        id: uuid::Uuid::new_v4(),
        source: n1.id,
        source_handle: "body".into(),
        target: n2.id,
        target_handle: "prompt".into(),
    });
    m.edges.push(EdgeEntry {
        id: uuid::Uuid::new_v4(),
        source: n2.id,
        source_handle: "text".into(),
        target: n3.id,
        target_handle: "in".into(),
    });

    let id = repo.create(&m).unwrap();
    let back = repo.get(id).unwrap();
    assert_eq!(back.nodes.len(), 3);
    assert_eq!(back.edges.len(), 2);
    assert_eq!(back.viewport.pan, (40, 12));
    assert_eq!(back.viewport.grid_step, 2);
    assert_eq!(back.sop.as_deref(), Some("Be terse."));

    // Hashes round-trip.
    assert_eq!(m.content_hash(), back.content_hash());
}
