//! Phase 1 integration test: save a `Skill` and verify it round-trips
//! through the API.
//!
//! The full `Skill::execute` (as a subgraph) is part of Phase 3. This test
//! just exercises the metadata + persistence path: the `SkillRegistry` cache
//! stays in sync after `save_skill`, and the storage schema matches what we
//! wrote.

use async_trait::async_trait;
use schemars::Schema;
use std::sync::Arc;
use uuid::Uuid;
use workflow_editor::WorkflowManager;
use workflow_editor::error::Result;
use workflow_editor::executor::{NodeCtx, NodeExecutor, NodeReporter, PortInputs, PortOutputs};
use workflow_editor::model::{NodeEntry, PortSpec, Skill, WorkflowManifest};
use workflow_editor::registry::{NodeFactory, NodeRegistry};

struct StubNode;

#[async_trait]
impl NodeExecutor for StubNode {
    fn kind(&self) -> &'static str {
        "stub"
    }
    async fn execute(
        &self,
        _ctx: &mut NodeCtx,
        _inputs: &PortInputs,
        _r: &NodeReporter,
    ) -> Result<PortOutputs> {
        let mut out = PortOutputs::new();
        out.insert("out".into(), serde_json::json!("stub-out"));
        Ok(out)
    }
}

struct StubFactory;

#[async_trait]
impl NodeFactory for StubFactory {
    fn kind(&self) -> &'static str {
        "stub"
    }
    fn label(&self) -> &'static str {
        "Stub"
    }
    fn description(&self) -> &'static str {
        "Stub node."
    }
    fn category(&self) -> &'static str {
        "Test"
    }
    fn spec_schema(&self) -> Schema {
        schemars::schema_for!(serde_json::Value)
    }
    fn inputs(&self) -> Vec<PortSpec> {
        vec![PortSpec::new("in", "in")]
    }
    fn outputs(&self) -> Vec<PortSpec> {
        vec![PortSpec::new("out", "out")]
    }
    fn build_executor(&self, _params: serde_json::Value) -> Option<Box<dyn NodeExecutor>> {
        Some(Box::new(StubNode))
    }
}

fn manager_with_stub() -> WorkflowManager {
    let mgr = WorkflowManager::open_memory().unwrap();
    mgr.node_registry().register(Arc::new(StubFactory));
    mgr
}

#[tokio::test]
async fn save_skill_creates_v1_then_v2() {
    let mgr = manager_with_stub();
    let client = mgr.new_session();

    let mut inner = WorkflowManifest::new("inner");
    inner.nodes.push(NodeEntry {
        id: Uuid::new_v4(),
        kind: "stub".into(),
        label: "n".into(),
        position: (0, 0),
        inputs: vec![PortSpec::new("in", "in")],
        outputs: vec![PortSpec::new("out", "out")],
        params: serde_json::json!({}),
    });
    let skill = Skill::new("summarize", inner)
        .with_description("Summarize a document")
        .with_sop("You are a careful summarizer.")
        .with_tool_refs(vec!["stub".into()])
        .with_surface_inputs(vec![PortSpec::new("in", "input")])
        .with_surface_outputs(vec![PortSpec::new("out", "output")]);

    let saved_v1 = client.save_skill(skill.clone()).await.unwrap();
    assert_eq!(saved_v1.version, 1);

    let saved_v2 = client.save_skill(skill.clone()).await.unwrap();
    assert_eq!(saved_v2.version, 2);

    // Reload from DB: the latest version is 2.
    let repo = mgr.skill_repo();
    let latest = repo.get(saved_v2.id).unwrap();
    assert_eq!(latest.version, 2);
    assert_eq!(latest.sop_text, "You are a careful summarizer.");
    assert_eq!(latest.tool_refs, vec!["stub".to_string()]);
    assert_eq!(latest.surface_inputs.len(), 1);
    assert_eq!(latest.surface_outputs.len(), 1);

    // The inner manifest also survived.
    assert_eq!(latest.manifest.nodes.len(), 1);
    assert_eq!(latest.manifest.nodes[0].kind, "stub");
}

#[tokio::test]
async fn list_skills_reflects_saves() {
    let mgr = manager_with_stub();
    let client = mgr.new_session();

    let mut inner = WorkflowManifest::new("inner");
    inner.nodes.push(NodeEntry {
        id: Uuid::new_v4(),
        kind: "stub".into(),
        label: "n".into(),
        position: (0, 0),
        inputs: vec![PortSpec::new("in", "in")],
        outputs: vec![PortSpec::new("out", "out")],
        params: serde_json::json!({}),
    });
    client
        .save_skill(
            Skill::new("alpha", inner.clone())
                .with_description("Alpha skill")
                .with_sop("alpha sop"),
        )
        .await
        .unwrap();
    client
        .save_skill(
            Skill::new("beta", inner)
                .with_description("Beta skill")
                .with_sop("beta sop"),
        )
        .await
        .unwrap();

    let summaries = client.list_skills().await.unwrap();
    let names: Vec<&str> = summaries.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
}
