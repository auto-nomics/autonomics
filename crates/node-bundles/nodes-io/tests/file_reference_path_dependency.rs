//! End-to-end guard for the `file_reference` scheduling hazard: a DAG whose
//! `file_reference` node reads, by path, a file another node in the same DAG
//! writes must be rejected at validation time — the port graph cannot order
//! the pair, so the read would race the write (missing-file error, or a
//! silent read of a stale file left by an earlier run).

use async_trait::async_trait;
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DAG, DagError};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::NodeCtx;
use dag_core::value::{FileRef, PortType};
use datafusion::prelude::SessionContext;
use nodes_io::file_reference::FileReferenceNode;

/// Writes `path` on execute and declares it via `sink_path` — the shape any
/// statically-addressable file sink presents to validation.
struct DeclaredSink {
    ports: NodePorts,
    path: String,
}

impl DeclaredSink {
    fn new(path: &str) -> Self {
        Self {
            ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
            path: path.to_string(),
        }
    }
}

#[async_trait]
impl DagNode for DeclaredSink {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            path: self.path.clone(),
        })
    }
    fn kind(&self) -> &'static str {
        "declared_sink_probe"
    }
    fn sink_path(&self) -> Option<&str> {
        Some(&self.path)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        _ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        std::fs::write(&self.path, b"payload").unwrap();
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, FileRef::local(&self.path, None).unwrap());
        Ok(outputs)
    }
}

fn reference_node(path: &std::path::Path) -> FileReferenceNode {
    FileReferenceNode::new(path.to_string_lossy().into_owned(), None)
}

fn ctx() -> NodeCtx {
    NodeCtx::new(SessionContext::new().runtime_env(), None)
}

#[tokio::test]
async fn path_reference_aliased_to_an_in_dag_sink_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("artifact.bin");

    let mut dag = DAG::default();
    dag.add_node(
        "producer".into(),
        Box::new(DeclaredSink::new(&path.to_string_lossy())),
    )
    .unwrap();
    // Deliberately NO edge producer -> consumer: the hazard under test.
    dag.add_node("consumer".into(), Box::new(reference_node(&path)))
        .unwrap();

    let error = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx(),
            None,
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("`consumer` references file"), "{error}");
    assert!(
        error.contains("`producer` writes that same file"),
        "{error}"
    );
    assert!(error.contains("output port"), "{error}");
}

#[tokio::test]
async fn path_reference_to_an_external_file_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let external = dir.path().join("external.bin");
    std::fs::write(&external, b"external-payload").unwrap();

    let mut dag = DAG::default();
    dag.add_node(
        "producer".into(),
        Box::new(DeclaredSink::new(
            &dir.path().join("out.bin").to_string_lossy(),
        )),
    )
    .unwrap();
    dag.add_node("consumer".into(), Box::new(reference_node(&external)))
        .unwrap();

    dag.run(
        &dag_core::dag::runtime::SchedulerConfig::default(),
        &ctx(),
        None,
    )
    .await
    .unwrap();

    let output = dag.output("consumer").unwrap();
    let file = output.get(&0).unwrap().as_file().unwrap();
    assert_eq!(std::fs::read(&file.path).unwrap(), b"external-payload");
}

#[tokio::test]
async fn file_reference_declares_its_path_for_validation() {
    let path = std::path::Path::new("/workspace/in.bin");
    let node = FileReferenceNode::new("/workspace/in.bin", None);
    assert_eq!(node.referenced_file_paths(), vec![path.to_string_lossy()]);
}
