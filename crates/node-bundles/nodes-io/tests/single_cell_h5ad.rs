use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache, PodmanConnection,
};
use dag_core::registry::{NodeCtx, NodeFactory};
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::single_cell_h5ad::{H5AD_QC_FILTER_KIND, SingleCellH5adContainerNodeFactory};
use vfs::OpendalFileStorage;

#[derive(Default)]
struct FakeWorkflowRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
    params: Mutex<Vec<serde_json::Value>>,
}

impl FakeWorkflowRuntime {
    fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: Some(workspace_root.to_path_buf()),
            ..Default::default()
        }
    }

    fn workspace_path(&self, request: &ContainerRunRequest, path: &str) -> PathBuf {
        let relative = Path::new(path)
            .strip_prefix(container_runtime::DEFAULT_CONTAINER_WORKDIR)
            .expect("path is inside /work");
        request.workspace.host_path.join(relative)
    }
}

#[async_trait::async_trait]
impl PodmanConnection for FakeWorkflowRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        let env = |name: &str| {
            request
                .env
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ContainerRuntimeError::Invalid(format!("missing {name}")))
        };
        let output0 = self.workspace_path(&request, &env("AUTONOMICS_OUTPUT0")?);
        let output1 = self.workspace_path(&request, &env("AUTONOMICS_OUTPUT1")?);
        std::fs::write(output0, b"h5ad-fixture").unwrap();
        std::fs::write(output1, br#"{"operation":"qc_filter"}"#).unwrap();
        let params_path = request
            .workspace
            .host_path
            .join(".autonomics/files/params.json");
        let params: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(params_path).unwrap()).unwrap();
        self.params.lock().unwrap().push(params);
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-single-cell-workflow"
    }

    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

#[tokio::test]
async fn qc_node_stages_h5ad_and_publishes_contract_outputs() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let h5ad_path = root.path().join("input.h5ad");
    std::fs::write(&h5ad_path, b"h5ad-input").unwrap();

    let runtime = Arc::new(FakeWorkflowRuntime::new(root.path()));
    let factory = SingleCellH5adContainerNodeFactory::qc_filter(
        runtime.clone(),
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory
        .build(
            serde_json::json!({
                "min_genes": 10,
                "max_pct_mt": 15,
                "artifact_prefix": "/artifacts/single-cell-h5ad-test"
            }),
            ctx.clone(),
        )
        .unwrap();
    assert_eq!(node.kind(), H5AD_QC_FILTER_KIND);
    assert_eq!(node.ports().input_ports().len(), 1);
    assert_eq!(node.ports().output_ports().len(), 2);

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "source".into(),
        Box::new(FileReferenceNode::new(
            h5ad_path.to_string_lossy().into_owned(),
            Some("h5ad".into()),
        )),
    )
    .unwrap();
    dag.add_node("qc".into(), node).unwrap();
    dag.add_edge("source", "qc", 0, 0).unwrap();
    dag.run(
        &dag_core::dag::runtime::SchedulerConfig::default(),
        &ctx,
        None,
    )
    .await
    .unwrap();

    let request = runtime.requests.lock().unwrap()[0].clone();
    let params = runtime.params.lock().unwrap()[0].clone();
    assert_eq!(params["min_genes"], 10);
    assert_eq!(params["max_pct_mt"], 15);
    assert_eq!(
        request
            .env
            .iter()
            .find(|(key, _)| key == "AUTONOMICS_SINGLE_CELL_WORKFLOW")
            .map(|(_, value)| value.as_str()),
        Some("qc_filter")
    );

    let outputs = dag.output("qc").unwrap();
    assert!(
        outputs
            .get(&0)
            .unwrap()
            .as_file()
            .unwrap()
            .path
            .ends_with("/output.h5ad")
    );
    assert!(
        outputs
            .get(&1)
            .unwrap()
            .as_file()
            .unwrap()
            .path
            .ends_with("/report.json")
    );
}
