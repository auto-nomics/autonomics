//! Phase 5 integration test: skill export and import round-trips.
//!
//! ## Coverage
//!
//! - `client.export_skill` returns a JSON document that round-trips into
//!   the same `Skill` struct.
//! - `client.import_skill` creates a new version of the skill on disk.
//! - An imported skill is discoverable via `client.list_skills`.
//! - Round-tripping through JSON preserves `sop_text`, `tool_refs`,
//!   `surface_inputs`, `surface_outputs`, and the inner manifest.

use uuid::Uuid;
use workflow_editor::WorkflowManager;
use workflow_editor::model::{NodeEntry, PortSpec, Skill, WorkflowManifest};

fn make_skill(name: &str) -> Skill {
    let mut inner = WorkflowManifest::new("inner");
    inner.nodes.push(NodeEntry {
        id: Uuid::new_v4(),
        kind: "echo".into(),
        label: "n".into(),
        position: (0, 0),
        inputs: vec![PortSpec::new("in", "in")],
        outputs: vec![PortSpec::new("out", "out")],
        params: serde_json::json!({ "flag": true }),
    });
    Skill::new(name, inner)
        .with_description("phase5-skill")
        .with_sop("phase5 sop text")
        .with_tool_refs(vec!["a".into(), "b".into()])
        .with_surface_inputs(vec![PortSpec::new("in", "input")])
        .with_surface_outputs(vec![PortSpec::new("out", "output")])
}

#[tokio::test]
async fn export_then_import_round_trips_skill() {
    let mgr = WorkflowManager::open_memory().unwrap();
    let client = mgr.new_session();
    let saved = client.save_skill(make_skill("alpha")).await.unwrap();
    assert_eq!(saved.version, 1);

    // Export the saved skill (by stable id).
    let json = client.export_skill(saved.id).await.unwrap();
    assert!(json.contains("phase5 sop text"));
    assert!(json.contains("\"name\": \"alpha\""));

    // Parsing the JSON directly yields an equal Skill (modulo id/version).
    let parsed: Skill = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "alpha");
    assert_eq!(parsed.sop_text, "phase5 sop text");
    assert_eq!(parsed.tool_refs, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(parsed.surface_inputs.len(), 1);
    assert_eq!(parsed.surface_outputs.len(), 1);
    assert_eq!(parsed.manifest.nodes.len(), 1);

    // Import into a *fresh* manager — the imported skill will get a new id
    // (different from `saved.id`) but the same name.
    let mgr2 = WorkflowManager::open_memory().unwrap();
    let client2 = mgr2.new_session();
    let imported = client2.import_skill(json).await.unwrap();
    assert_eq!(imported.name, "alpha");
    assert_eq!(imported.sop_text, "phase5 sop text");
    assert_ne!(imported.id, saved.id, "fresh manager mints a new skill id");
    assert_eq!(imported.version, 1, "fresh manager sees alpha as version 1");

    // Listing skills on the new manager shows the imported one.
    let summaries = client2.list_skills().await.unwrap();
    assert!(summaries.iter().any(|s| s.name == "alpha"));

    // The skill repo can round-trip it too.
    let repo = mgr2.skill_repo();
    let fetched = repo.get(imported.id).unwrap();
    assert_eq!(fetched.sop_text, "phase5 sop text");
    assert_eq!(fetched.tool_refs, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(fetched.surface_inputs[0].id, "in");
    assert_eq!(fetched.surface_outputs[0].id, "out");
}

#[tokio::test]
async fn import_into_same_manager_bumps_version_and_keeps_id() {
    let mgr = WorkflowManager::open_memory().unwrap();
    let client = mgr.new_session();
    let saved_v1 = client.save_skill(make_skill("beta")).await.unwrap();
    let json = client.export_skill(saved_v1.id).await.unwrap();

    // Re-importing into the *same* manager keeps the same id and bumps the
    // version (the skill's NAME is the canonical key, not the id).
    let v2 = client.import_skill(json).await.unwrap();
    assert_eq!(v2.id, saved_v1.id);
    assert_eq!(v2.version, 2);
}

#[tokio::test]
async fn import_rejects_invalid_json() {
    let mgr = WorkflowManager::open_memory().unwrap();
    let client = mgr.new_session();
    let res = client.import_skill("not json".into()).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn export_unknown_skill_returns_error() {
    let mgr = WorkflowManager::open_memory().unwrap();
    let client = mgr.new_session();
    let res = client.export_skill(Uuid::new_v4()).await;
    assert!(res.is_err());
}
