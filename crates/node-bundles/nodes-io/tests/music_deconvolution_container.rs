use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache, PodmanConfig,
    PodmanConnection, PodmanRuntime,
};
use dag_core::NodeFactory;
use dag_core::dag::DAG;
use dag_core::dag::runtime::SchedulerConfig;
use dag_core::registry::NodeCtx;
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::music_deconvolution_container::{
    MUSIC_DECONVOLUTION_CONTAINER_KIND, MUSIC_DECONVOLUTION_IMAGE_DIGEST,
    MUSIC_DECONVOLUTION_IMAGE_REPOSITORY, MusicDeconvolutionContainerNodeFactory,
};
use vfs::OpendalFileStorage;

#[derive(Default)]
struct FakeMusicRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
    input_contents: Mutex<Vec<(String, Vec<u8>)>>,
}

impl FakeMusicRuntime {
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
impl PodmanConnection for FakeMusicRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        for index in 0..5 {
            let input = self.env(&request, &format!("AUTONOMICS_INPUT{index}"));
            let path = self.workspace_path(&request, &input);
            self.input_contents
                .lock()
                .unwrap()
                .push((input, std::fs::read(path)?));
        }
        let outputs = [
            (
                "AUTONOMICS_OUTPUT0",
                "sample_id\tneuron\tastrocyte\nbulk_70n\t0.7\t0.3\n",
            ),
            (
                "AUTONOMICS_OUTPUT1",
                "sample_id\tneuron\tastrocyte\nbulk_70n\t0.7\t0.3\n",
            ),
            ("AUTONOMICS_OUTPUT2", "gene_id\tbulk_70n\nGENE000\t1.0\n"),
            (
                "AUTONOMICS_OUTPUT3",
                "sample_id\tr_squared\nbulk_70n\t0.99\n",
            ),
            ("AUTONOMICS_OUTPUT4", "{\"schema_version\":\"1.0\"}\n"),
        ];
        for (name, contents) in outputs {
            let path = self.workspace_path(&request, &self.env(&request, name));
            std::fs::write(path, contents)?;
        }
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "fake MuSiC run".into(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-music"
    }

    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

async fn read_published(ctx: &NodeCtx, file: &dag_core::value::FileRef) -> Vec<u8> {
    let storage = ctx.opendal.as_ref().expect("test VFS storage");
    let path = file
        .path
        .strip_prefix("vfs://")
        .expect("container output uses a vfs:// URI");
    storage
        .resolve(path)
        .read(&storage.resolve_path(path))
        .await
        .expect("read published MuSiC artifact")
        .to_vec()
}

#[tokio::test]
async fn stages_five_inputs_and_publishes_five_outputs() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/music-deconvolution/fixtures");
    let inputs = [
        fixture_root.join("bulk_expression.tsv"),
        fixture_root.join("single_cell_counts.tsv"),
        fixture_root.join("cell_metadata.tsv"),
        fixture_root.join("cell_sizes.tsv"),
        fixture_root.join("markers.tsv"),
    ];

    let runtime = Arc::new(FakeMusicRuntime::new(root.path()));
    let factory = MusicDeconvolutionContainerNodeFactory::new(
        runtime.clone(),
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory
        .build(
            serde_json::json!({
                "select_cell_types": ["neuron", "astrocyte"],
                "artifact_prefix": "/artifacts/music-test"
            }),
            ctx.clone(),
        )
        .unwrap();
    assert_eq!(node.kind(), MUSIC_DECONVOLUTION_CONTAINER_KIND);
    assert_eq!(node.ports().input_ports().len(), 5);
    assert_eq!(node.ports().output_ports().len(), 5);

    let mut dag = DAG::default();
    dag.add_node("music".into(), node).unwrap();
    for (index, path) in inputs.iter().enumerate() {
        dag.add_node(
            format!("input{index}").into(),
            Box::new(FileReferenceNode::new(
                path.to_string_lossy().into_owned(),
                Some("music-input".into()),
            )),
        )
        .unwrap();
        dag.add_edge(&format!("input{index}"), "music", 0, index as u8)
            .unwrap();
    }
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("music"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "MuSiC fake-runtime DAG failed: {report:#?}"
    );

    let request = runtime.requests.lock().unwrap()[0].clone();
    assert_eq!(
        request.image,
        nodes_io::image_registry::acr_image(
            MUSIC_DECONVOLUTION_IMAGE_REPOSITORY,
            MUSIC_DECONVOLUTION_IMAGE_DIGEST
        )
        .unwrap()
    );
    assert_eq!(
        request.network,
        container_runtime::ContainerNetwork::Isolated
    );
    assert!(request.read_only_rootfs);
    assert!(request.panels.is_empty());
    let env: BTreeMap<_, _> = request.env.iter().cloned().collect();
    assert_eq!(env["AUTONOMICS_MUSIC_CELL_TYPE_COL"], "cell_type");
    assert_eq!(env["AUTONOMICS_MUSIC_SUBJECT_COL"], "subject_id");
    assert_eq!(
        env["AUTONOMICS_MUSIC_SELECT_CELL_TYPES"],
        "neuron,astrocyte"
    );
    assert_eq!(env["AUTONOMICS_MUSIC_ITER_MAX"], "1000");
    assert_eq!(runtime.input_contents.lock().unwrap().len(), 5);

    let outputs = dag.output("music").unwrap();
    let expected = [
        "cell_type_proportions.tsv",
        "nnls_proportions.tsv",
        "gene_weights.tsv",
        "diagnostics.tsv",
        "run_report.json",
    ];
    for (index, name) in expected.into_iter().enumerate() {
        let output = outputs.get(&(index as u8)).unwrap().as_file().unwrap();
        assert!(output.path.starts_with("vfs:///artifacts/music-test/"));
        assert!(output.path.ends_with(&format!("/{name}")));
    }
    let proportions = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let bytes = read_published(&ctx, &proportions).await;
    assert!(bytes.starts_with(b"sample_id\tneuron\tastrocyte\n"));
}

#[ignore = "requires rootless Podman and the published music-deconvolution image"]
#[tokio::test]
async fn real_podman_runs_music_deconvolution_with_known_mixtures() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/music-deconvolution/fixtures");
    let workspace = root.path().join("workspace");
    let panels = root.path().join("panels");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&panels).unwrap();
    let runtime = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
        workspace_root: workspace,
        panel_cache_root: panels.clone(),
    }));
    let factory =
        MusicDeconvolutionContainerNodeFactory::new(runtime, Arc::new(PanelCache::new(panels)));
    let node = factory
        .build(
            serde_json::json!({
                "select_cell_types": ["neuron", "astrocyte"],
                "artifact_prefix": "/artifacts/music-real-it"
            }),
            ctx.clone(),
        )
        .unwrap();

    let inputs = [
        fixture_root.join("bulk_expression.tsv"),
        fixture_root.join("single_cell_counts.tsv"),
        fixture_root.join("cell_metadata.tsv"),
        fixture_root.join("cell_sizes.tsv"),
        fixture_root.join("markers.tsv"),
    ];
    let mut dag = DAG::default();
    dag.add_node("music".into(), node).unwrap();
    for (index, path) in inputs.iter().enumerate() {
        dag.add_node(
            format!("input{index}").into(),
            Box::new(FileReferenceNode::new(
                path.to_string_lossy().into_owned(),
                Some("music-input".into()),
            )),
        )
        .unwrap();
        dag.add_edge(&format!("input{index}"), "music", 0, index as u8)
            .unwrap();
    }
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("music"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "MuSiC real Podman DAG failed: {report:#?}"
    );

    let outputs = dag.output("music").unwrap();
    let proportions = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let proportions_text = String::from_utf8(read_published(&ctx, &proportions).await).unwrap();
    assert_eq!(
        proportions_text.lines().next().unwrap(),
        "sample_id\tneuron\tastrocyte"
    );
    assert_eq!(proportions_text.lines().count(), 7);
    let expected = [
        ("bulk_70n", 0.70),
        ("bulk_60n", 0.60),
        ("bulk_50n", 0.50),
        ("bulk_40n", 0.40),
        ("bulk_30n", 0.30),
        ("bulk_20n", 0.20),
    ];
    for (sample, expected_fraction) in expected {
        let line = proportions_text
            .lines()
            .find(|line| line.starts_with(sample))
            .unwrap();
        let fields: Vec<_> = line.split('\t').collect();
        let neuron: f64 = fields[1].parse().unwrap();
        let astrocyte: f64 = fields[2].parse().unwrap();
        assert!(
            (neuron - expected_fraction).abs() < 0.025,
            "{sample}: neuron={neuron}, expected={expected_fraction}"
        );
        assert!((neuron + astrocyte - 1.0).abs() < 1e-10);
    }

    let report = outputs.get(&4).unwrap().as_file().unwrap().clone();
    let report: serde_json::Value =
        serde_json::from_slice(&read_published(&ctx, &report).await).unwrap();
    assert_eq!(report["engine"]["package"], "MuSiC");
    assert_eq!(report["engine"]["version"], "1.0.0");
    assert_eq!(
        report["engine"]["commit"],
        "f21fe67f5670d5e9fca0ad7550abaae3423eb59c"
    );
    assert_eq!(report["analysis"]["markers_provided"], true);
    assert_eq!(report["analysis"]["cell_sizes_provided"], true);
    assert_eq!(report["dimensions"]["reference_subjects"], 4);
    assert_eq!(report["dimensions"]["selected_cell_types"], 2);
}
