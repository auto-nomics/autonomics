use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache, PodmanConnection,
};
use dag_core::registry::{NodeCtx, NodeFactory};
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::single_cell_container::{
    SINGLE_CELL_PREPROCESSOR_CONTAINER_KIND, SingleCellPreprocessorContainerNodeFactory,
};
use vfs::OpendalFileStorage;

#[derive(Default)]
struct FakePreprocessorRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
}

impl FakePreprocessorRuntime {
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
impl PodmanConnection for FakePreprocessorRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        let path = |name: &str| {
            request
                .env
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ContainerRuntimeError::Invalid(format!("missing {name}")))
        };
        let report = self.workspace_path(&request, &path("AUTONOMICS_OUTPUT0")?);
        let h5ad = self.workspace_path(&request, &path("AUTONOMICS_OUTPUT1")?);
        std::fs::write(report, br#"{"operation":"inspect"}"#).unwrap();
        std::fs::write(h5ad, b"ann-data-fixture").unwrap();
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-single-cell"
    }

    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

#[tokio::test]
async fn stages_four_files_and_publishes_two_outputs() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let names = ["matrix.mtx", "barcodes.tsv", "features.tsv", "metadata.tsv"];
    let inputs = names
        .iter()
        .map(|name| {
            let path = root.path().join(name);
            std::fs::write(&path, b"fixture").unwrap();
            path
        })
        .collect::<Vec<_>>();

    let runtime = Arc::new(FakePreprocessorRuntime::new(root.path()));
    let factory = SingleCellPreprocessorContainerNodeFactory::new(
        runtime.clone(),
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory
        .build(
            serde_json::json!({
                "operation": "inspect",
                "artifact_prefix": "/artifacts/single-cell-test"
            }),
            ctx.clone(),
        )
        .unwrap();
    assert_eq!(node.kind(), SINGLE_CELL_PREPROCESSOR_CONTAINER_KIND);
    assert_eq!(node.ports().input_ports().len(), 4);
    assert_eq!(node.ports().output_ports().len(), 2);

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node("preprocessor".into(), node).unwrap();
    for (index, path) in inputs.iter().enumerate() {
        let id = format!("input-{index}");
        dag.add_node(
            id.clone(),
            Box::new(FileReferenceNode::new(
                path.to_string_lossy().into_owned(),
                Some("file".into()),
            )),
        )
        .unwrap();
        dag.add_edge(id, "preprocessor", 0, index as u8).unwrap();
    }
    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("preprocessor"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "single-cell node failed: {report:#?}"
    );

    let request = runtime.requests.lock().unwrap()[0].clone();
    assert_eq!(
        request.network,
        container_runtime::ContainerNetwork::Isolated
    );
    assert!(request.read_only_rootfs);
    for index in 0..4 {
        let binding = format!("AUTONOMICS_INPUT{index}");
        let path = request
            .env
            .iter()
            .find(|(key, _)| key == &binding)
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(path.starts_with(container_runtime::DEFAULT_CONTAINER_WORKDIR));
    }

    let outputs = dag.output("preprocessor").unwrap();
    let report_file = outputs.get(&0).unwrap().as_file().unwrap();
    let h5ad_file = outputs.get(&1).unwrap().as_file().unwrap();
    assert!(
        report_file
            .path
            .starts_with("vfs:///artifacts/single-cell-test/")
    );
    assert!(report_file.path.ends_with("/preprocess_report.json"));
    assert!(h5ad_file.path.ends_with("/preprocessed.h5ad"));
}
