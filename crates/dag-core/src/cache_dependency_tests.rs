use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use serde_json::json;

use crate::dag::graph::PortOutputs;
use crate::dag::runtime::{InputHashing, SchedulerConfig};
use crate::dag::{DAG, DagError, node_event::NodeReporter};
use crate::fingerprint::bind_cache_dependency_state;
use crate::node::{DagNode, NodeInput, NodePorts};
use crate::registry::NodeCtx;
use crate::value::{ChannelValue, PortType};

#[test]
fn absent_dependency_preserves_exact_existing_fingerprint() {
    assert_eq!(
        bind_cache_dependency_state("ordinary-fingerprint".into(), None),
        "ordinary-fingerprint"
    );
}

#[test]
fn changed_dependency_changes_fingerprint_without_changing_base() {
    assert_ne!(
        bind_cache_dependency_state("same-base".into(), Some(&json!({"revision":1}))),
        bind_cache_dependency_state("same-base".into(), Some(&json!({"revision":2})))
    );
}

#[test]
fn explicit_null_dependency_is_not_absent_dependency() {
    assert_ne!(
        bind_cache_dependency_state("same-base".into(), None),
        bind_cache_dependency_state("same-base".into(), Some(&serde_json::Value::Null))
    );
}

#[test]
fn equivalent_object_key_order_has_same_dependency_fingerprint() {
    assert_eq!(
        bind_cache_dependency_state("same-base".into(), Some(&json!({"alpha":1,"beta":2}))),
        bind_cache_dependency_state("same-base".into(), Some(&json!({"beta":2,"alpha":1})))
    );
}

struct DependentNode {
    state: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
    unavailable: Arc<AtomicBool>,
    ports: NodePorts,
}

impl DependentNode {
    fn new(state: Arc<AtomicUsize>, calls: Arc<AtomicUsize>, unavailable: Arc<AtomicBool>) -> Self {
        Self {
            state,
            calls,
            unavailable,
            ports: NodePorts::new().add_output_port_of_type(None, PortType::Channel),
        }
    }
}

#[async_trait]
impl DagNode for DependentNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self::new(
            self.state.clone(),
            self.calls.clone(),
            self.unavailable.clone(),
        ))
    }
    fn kind(&self) -> &'static str {
        "fixture_dependency_node"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn cache_dependency_state(
        &self,
        _: &NodeCtx,
        _: &[NodeInput],
    ) -> Result<Option<serde_json::Value>, DagError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(DagError::Schedule("external dependency unavailable".into()));
        }
        Ok(Some(json!({"revision":self.state.load(Ordering::SeqCst)})))
    }

    async fn execute(
        &mut self,
        _: &NodeCtx,
        _: &[NodeInput],
        _: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut outputs = PortOutputs::new();
        outputs.insert(
            0,
            ChannelValue {
                items: vec![json!(self.state.load(Ordering::SeqCst))],
            },
        );
        Ok(outputs)
    }
}

fn config() -> SchedulerConfig {
    SchedulerConfig {
        max_concurrency: 1,
        incremental: true,
        input_hashing: InputHashing::Content,
        compute_row_counts: false,
        memory_guard: None,
        ..SchedulerConfig::default()
    }
}

fn context() -> NodeCtx {
    NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        None,
    )
}

#[tokio::test]
async fn actual_scheduler_reuses_unchanged_but_executes_changed_dependency() {
    let state = Arc::new(AtomicUsize::new(1));
    let calls = Arc::new(AtomicUsize::new(0));
    let unavailable = Arc::new(AtomicBool::new(false));
    let mut dag = DAG::default();
    dag.add_node_with_spec(
        "dependency".into(),
        Box::new(DependentNode::new(
            state.clone(),
            calls.clone(),
            unavailable,
        )),
        "fixture_dependency_node".into(),
        json!({}),
    )
    .unwrap();
    let context = context();
    assert!(dag.run(&config(), &context, None).await.unwrap().ok);
    assert!(dag.run(&config(), &context, None).await.unwrap().ok);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    state.store(2, Ordering::SeqCst);
    assert!(dag.run(&config(), &context, None).await.unwrap().ok);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        dag.output("dependency")
            .unwrap()
            .get(&0)
            .unwrap()
            .as_channel()
            .unwrap()
            .items,
        vec![json!(2)]
    );
}

#[tokio::test]
async fn unavailable_dependency_blocks_old_cache_without_executing_node() {
    let calls = Arc::new(AtomicUsize::new(0));
    let unavailable = Arc::new(AtomicBool::new(false));
    let mut dag = DAG::default();
    dag.add_node_with_spec(
        "dependency".into(),
        Box::new(DependentNode::new(
            Arc::new(AtomicUsize::new(1)),
            calls.clone(),
            unavailable.clone(),
        )),
        "fixture_dependency_node".into(),
        json!({}),
    )
    .unwrap();
    let context = context();
    assert!(dag.run(&config(), &context, None).await.unwrap().ok);
    unavailable.store(true, Ordering::SeqCst);
    let report = dag.run(&config(), &context, None).await.unwrap();
    assert!(!report.ok);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(report.errors.contains_key("dependency"));
    assert!(dag.output("dependency").is_none());
    assert!(report.nodes[0].output_files.is_empty());
}

struct AlwaysAuditor {
    directory: std::path::PathBuf,
    calls: Arc<AtomicUsize>,
    ports: NodePorts,
}

#[async_trait]
impl DagNode for AlwaysAuditor {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            directory: self.directory.clone(),
            calls: self.calls.clone(),
            ports: self.ports.clone(),
        })
    }
    fn kind(&self) -> &'static str {
        "fixture_readonly_full_auditor"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn allows_cache_reuse(&self) -> bool {
        false
    }
    async fn execute(
        &mut self,
        _: &NodeCtx,
        _: &[NodeInput],
        _: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Reads a real non-port dependency. This is a scheduler fixture,
        // never a callback claiming a production verifier acceptance.
        if std::fs::read(self.directory.join("non-anchor.txt")).unwrap() != b"LOCKED" {
            return Err(DagError::Schedule("current directory bytes changed".into()));
        }
        let mut outputs = PortOutputs::new();
        outputs.insert(
            0,
            ChannelValue {
                items: vec![json!("current bytes verified")],
            },
        );
        Ok(outputs)
    }
}

#[tokio::test]
async fn never_reuse_rechecks_non_anchor_bytes_in_actual_incremental_scheduler() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("anchor.txt"), b"stable anchor").unwrap();
    std::fs::write(directory.path().join("non-anchor.txt"), b"LOCKED").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut dag = DAG::default();
    dag.add_node_with_spec(
        "audit".into(),
        Box::new(AlwaysAuditor {
            directory: directory.path().into(),
            calls: calls.clone(),
            ports: NodePorts::new().add_output_port_of_type(None, PortType::Channel),
        }),
        "fixture_readonly_full_auditor".into(),
        json!({"anchor":"stable anchor"}),
    )
    .unwrap();
    let context = context();
    for expected_calls in [1, 2] {
        let report = dag.run(&config(), &context, None).await.unwrap();
        assert!(report.ok);
        assert!(!report.nodes[0].reused);
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
    }
    std::fs::write(directory.path().join("non-anchor.txt"), b"BROKEN").unwrap();
    assert_eq!(
        std::fs::read(directory.path().join("anchor.txt")).unwrap(),
        b"stable anchor"
    );
    let report = dag.run(&config(), &context, None).await.unwrap();
    assert!(!report.ok);
    assert!(!report.nodes[0].reused);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(report.errors.contains_key("audit"));
    assert!(dag.output("audit").is_none());
}
