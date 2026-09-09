use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache, PodmanConfig,
    PodmanConnection, PodmanRuntime,
};
use dag_core::registry::{NodeCtx, NodeFactory};
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::visualization_container::{
    VISUALIZATION_CONTAINER_KIND, VisualizationContainerNodeFactory,
};
use vfs::OpendalFileStorage;

#[derive(Default)]
struct FakeRenderRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
}

impl FakeRenderRuntime {
    fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: Some(workspace_root.to_path_buf()),
            ..Default::default()
        }
    }

    fn workspace_path(&self, request: &ContainerRunRequest, path: &str) -> PathBuf {
        let relative = Path::new(path)
            .strip_prefix(container_runtime::DEFAULT_CONTAINER_WORKDIR)
            .expect("runtime path is inside /work");
        request.workspace.host_path.join(relative)
    }

    fn env(&self, request: &ContainerRunRequest, name: &str) -> String {
        request
            .env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("missing `{name}` environment binding"))
    }
}

#[async_trait]
impl PodmanConnection for FakeRenderRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        let data = self.env(&request, "AUTONOMICS_INPUT0");
        let script = self.env(&request, "AUTONOMICS_INPUT1");
        let output = self.env(&request, "AUTONOMICS_OUTPUT0");
        let data_path = self.workspace_path(&request, &data);
        let script_path = self.workspace_path(&request, &script);
        let output_path = self.workspace_path(&request, &output);

        assert!(
            data_path.is_file(),
            "data input is missing: {data_path:?}; workspace={:?}; bindings: data={data}, script={script}, output={output}",
            request.workspace.host_path
        );
        assert!(
            script_path.is_file(),
            "script input is missing: {script_path:?}; bindings: data={data}, script={script}, output={output}"
        );
        std::fs::write(output_path, [0x89, b'P', b'N', b'G']).unwrap();
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-visualization"
    }

    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

#[tokio::test]
async fn stages_data_and_script_files_and_publishes_png() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage.clone()),
    );
    let data = root.path().join("data.csv");
    let script = root.path().join("plot.R");
    std::fs::write(&data, "x,y\n1,2\n").unwrap();
    std::fs::write(&script, "p <- ggplot2::ggplot(df)\n").unwrap();

    let runtime = Arc::new(FakeRenderRuntime::new(root.path()));
    let factory = VisualizationContainerNodeFactory::new(
        runtime.clone(),
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory
        .build(
            serde_json::json!({
                "data_format": "csv",
                "artifact_prefix": "/artifacts/visualization-test"
            }),
            ctx.clone(),
        )
        .unwrap();
    assert_eq!(node.kind(), VISUALIZATION_CONTAINER_KIND);
    assert_eq!(node.ports().input_ports().len(), 2);
    assert_eq!(node.ports().output_ports().len(), 1);

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "data".into(),
        Box::new(FileReferenceNode::new(
            data.to_string_lossy().into_owned(),
            Some("csv".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "script".into(),
        Box::new(FileReferenceNode::new(
            script.to_string_lossy().into_owned(),
            Some("r".into()),
        )),
    )
    .unwrap();
    dag.add_node("plot".into(), node).unwrap();
    dag.add_edge("data", "plot", 0, 0).unwrap();
    dag.add_edge("script", "plot", 0, 1).unwrap();

    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("plot"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let request = runtime.requests.lock().unwrap()[0].clone();
    assert_eq!(
        request.network,
        container_runtime::ContainerNetwork::Isolated
    );
    assert!(request.read_only_rootfs);
    assert_eq!(
        request.command,
        vec![
            "Rscript".to_string(),
            "/opt/autonomics/render.R".to_string()
        ]
    );

    let output = dag
        .output("plot")
        .unwrap()
        .get(&0)
        .unwrap()
        .as_file()
        .unwrap()
        .clone();
    assert!(
        output
            .path
            .starts_with("vfs:///artifacts/visualization-test/")
    );
    assert!(output.path.ends_with("/plot.png"));
    let virtual_path = output.path.strip_prefix("vfs://").unwrap();
    let bytes = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    assert_eq!(bytes.to_vec(), [0x89, b'P', b'N', b'G']);
}

#[tokio::test]
#[ignore = "requires a working rootless Podman runtime and access to the visualization image"]
async fn real_podman_renders_staged_files_to_png() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage.clone()),
    );
    let data = root.path().join("data.csv");
    let script = root.path().join("plot.R");
    std::fs::write(&data, "x,y\n1,1\n2,4\n3,9\n").unwrap();
    std::fs::write(
        &script,
        "p <- ggplot2::ggplot(df, ggplot2::aes(x, y)) + ggplot2::geom_point()\n",
    )
    .unwrap();

    let workspace_root = root.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let runtime = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
        workspace_root,
        panel_cache_root: root.path().join("panels"),
    }));
    let factory = VisualizationContainerNodeFactory::new(
        runtime,
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory
        .build(
            serde_json::json!({
                "data_format": "csv",
                "artifact_prefix": "/artifacts/visualization-real",
                "timeout_secs": 120
            }),
            ctx.clone(),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "data".into(),
        Box::new(FileReferenceNode::new(
            data.to_string_lossy().into_owned(),
            Some("csv".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "script".into(),
        Box::new(FileReferenceNode::new(
            script.to_string_lossy().into_owned(),
            Some("r".into()),
        )),
    )
    .unwrap();
    dag.add_node("plot".into(), node).unwrap();
    dag.add_edge("data", "plot", 0, 0).unwrap();
    dag.add_edge("script", "plot", 0, 1).unwrap();

    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("plot"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let output = dag
        .output("plot")
        .unwrap()
        .get(&0)
        .unwrap()
        .as_file()
        .unwrap()
        .clone();
    assert!(output.path.ends_with("/plot.png"));
    let virtual_path = output.path.strip_prefix("vfs://").unwrap();
    let bytes = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    let png = bytes.to_vec();
    assert_eq!(png[..4], [0x89, b'P', b'N', b'G']);
}
