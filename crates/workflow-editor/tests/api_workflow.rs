//! Phase 1 integration test: end-to-end workflow creation / save / load / run
//! through the public `WorkflowClient` API.
//!
//! Asserts:
//! - CreateWorkflow returns a fresh id
//! - add_node + add_edge + save produces a workflow that round-trips via load
//! - validate returns ok for a well-formed graph
//! - run executes a 2-node pipeline in topological order and surfaces the
//!   `out` port as a `final_output`
//! - history grows by 1 per save

use async_trait::async_trait;
use schemars::Schema;
use std::sync::Arc;
use uuid::Uuid;
use workflow_editor::WorkflowManager;
use workflow_editor::error::Result;
use workflow_editor::executor::{NodeCtx, NodeExecutor, NodeReporter, PortInputs, PortOutputs};
use workflow_editor::model::{EdgeEntry, NodeEntry, PortSpec};
use workflow_editor::registry::{NodeFactory, NodeRegistry};

/// Echo node: copies inputs forward and emits `"out" -> "echoed"`.
struct EchoNode;

#[async_trait]
impl NodeExecutor for EchoNode {
    fn kind(&self) -> &'static str {
        "echo"
    }
    async fn execute(
        &self,
        _ctx: &mut NodeCtx,
        inputs: &PortInputs,
        _r: &NodeReporter,
    ) -> Result<PortOutputs> {
        let mut out = PortOutputs::new();
        for (k, v) in inputs {
            out.insert(k.clone(), v.clone());
        }
        out.insert("out".into(), serde_json::json!("echoed"));
        Ok(out)
    }
}

struct EchoFactory;

#[async_trait]
impl NodeFactory for EchoFactory {
    fn kind(&self) -> &'static str {
        "echo"
    }
    fn label(&self) -> &'static str {
        "Echo"
    }
    fn description(&self) -> &'static str {
        "Echoes its input."
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
        Some(Box::new(EchoNode))
    }
}

fn manager_with_echo() -> WorkflowManager {
    let mgr = WorkflowManager::open_memory().unwrap();
    mgr.node_registry().register(Arc::new(EchoFactory));
    mgr
}

fn echo_node(label: &str) -> NodeEntry {
    NodeEntry {
        id: Uuid::new_v4(),
        kind: "echo".into(),
        label: label.into(),
        position: (0, 0),
        inputs: vec![PortSpec::new("in", "in")],
        outputs: vec![PortSpec::new("out", "out")],
        params: serde_json::json!({}),
    }
}

#[tokio::test]
async fn create_save_load_roundtrip() {
    let mgr = manager_with_echo();
    let client = mgr.new_session();

    let id = client.create_workflow("hello").await.unwrap();

    let mut manifest = client.load_workflow(id).await.unwrap();
    let n1 = echo_node("src");
    let n2 = echo_node("sink");
    let n1_id = n1.id;
    let n2_id = n2.id;
    manifest.nodes.push(n1);
    manifest.nodes.push(n2);
    manifest.edges.push(EdgeEntry {
        id: Uuid::new_v4(),
        source: n1_id,
        source_handle: "out".into(),
        target: n2_id,
        target_handle: "in".into(),
    });

    client.save_workflow(manifest, "init").await.unwrap();

    let reloaded = client.load_workflow(id).await.unwrap();
    assert_eq!(reloaded.nodes.len(), 2);
    assert_eq!(reloaded.edges.len(), 1);
}

#[tokio::test]
async fn add_node_via_client_uses_actor() {
    let mgr = manager_with_echo();
    let client = mgr.new_session();
    let id = client.create_workflow("test").await.unwrap();
    client.add_node(id, echo_node("n1")).await.unwrap();
    client.add_node(id, echo_node("n2")).await.unwrap();
    let manifest = client.load_workflow(id).await.unwrap();
    assert_eq!(manifest.nodes.len(), 2);
}

#[tokio::test]
async fn validate_ok_for_well_formed_graph() {
    let mgr = manager_with_echo();
    let client = mgr.new_session();
    let id = client.create_workflow("ok").await.unwrap();
    client.add_node(id, echo_node("a")).await.unwrap();
    client.add_node(id, echo_node("b")).await.unwrap();
    let manifest = client.load_workflow(id).await.unwrap();
    let report = client.validate(manifest).await.unwrap();
    assert!(report.ok, "errors: {:?}", report.errors);
}

#[tokio::test]
async fn validate_rejects_cycle() {
    let mgr = manager_with_echo();
    let client = mgr.new_session();
    let id = client.create_workflow("cyc").await.unwrap();
    client.add_node(id, echo_node("a")).await.unwrap();
    client.add_node(id, echo_node("b")).await.unwrap();
    // Add a 2-node cycle by reading the manifest, adding edges, saving.
    let mut manifest = client.load_workflow(id).await.unwrap();
    let a = manifest.nodes[0].id;
    let b = manifest.nodes[1].id;
    manifest.edges.push(EdgeEntry {
        id: Uuid::new_v4(),
        source: a,
        source_handle: "out".into(),
        target: b,
        target_handle: "in".into(),
    });
    manifest.edges.push(EdgeEntry {
        id: Uuid::new_v4(),
        source: b,
        source_handle: "out".into(),
        target: a,
        target_handle: "in".into(),
    });
    let report = client.validate(manifest).await.unwrap();
    assert!(!report.ok);
    assert!(report.errors.iter().any(|e| e.contains("cycle")));
}

#[tokio::test]
async fn run_executes_topologically() {
    let mgr = manager_with_echo();
    let client = mgr.new_session();
    let id = client.create_workflow("run").await.unwrap();
    let n1 = echo_node("src");
    let n2 = echo_node("sink");
    let n1_id = n1.id;
    let n2_id = n2.id;
    client.add_node(id, n1).await.unwrap();
    client.add_node(id, n2).await.unwrap();
    let mut manifest = client.load_workflow(id).await.unwrap();
    manifest.edges.push(EdgeEntry {
        id: Uuid::new_v4(),
        source: n1_id,
        source_handle: "out".into(),
        target: n2_id,
        target_handle: "in".into(),
    });
    client
        .save_workflow(manifest.clone(), "wire")
        .await
        .unwrap();

    let result = client.run(id, Default::default()).await.unwrap();
    // Both nodes should have produced an "out" -> "echoed" output.
    assert!(
        result
            .outputs
            .values()
            .any(|v| v == &serde_json::json!("echoed"))
    );
    // The Phase 1 final_outputs surface "out" ports under the node label.
    assert!(
        result
            .final_outputs
            .values()
            .any(|v| v == &serde_json::json!("echoed"))
    );
}

#[tokio::test]
async fn history_grows_on_save() {
    let mgr = manager_with_echo();
    let client = mgr.new_session();
    let id = client.create_workflow("hist").await.unwrap();
    let initial = client.history(id).await.unwrap();
    assert_eq!(initial.len(), 1, "creation writes one snapshot");
    client
        .save_workflow(client.load_workflow(id).await.unwrap(), "second")
        .await
        .unwrap();
    let after = client.history(id).await.unwrap();
    assert_eq!(after.len(), 2);
}
