use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache, PodmanConfig,
    PodmanConnection, PodmanRuntime,
};
use dag_core::registry::{NodeCtx, NodeFactory};
use nodes_io::deseq2_container::{DESEQ2_DE_CONTAINER_KIND, Deseq2DeContainerNodeFactory};
use nodes_io::file_reference::FileReferenceNode;
use sha2::{Digest, Sha256};
use vfs::OpendalFileStorage;

#[derive(Default)]
struct FakeDeseq2Runtime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
    input_contents: Mutex<Vec<(String, Vec<u8>)>>,
}

impl FakeDeseq2Runtime {
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
impl PodmanConnection for FakeDeseq2Runtime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        for index in 0..2 {
            let input = self.env(&request, &format!("AUTONOMICS_INPUT{index}"));
            let path = self.workspace_path(&request, &input);
            self.input_contents
                .lock()
                .unwrap()
                .push((input, std::fs::read(path)?));
        }
        let outputs = [
            ("AUTONOMICS_OUTPUT0", "gene_id\tbaseMean\nFBgn1\t1\n"),
            ("AUTONOMICS_OUTPUT1", "gene_id\ts1\nFBgn1\t1\n"),
            ("AUTONOMICS_OUTPUT2", "sample_id\tsize_factor\ns1\t1\n"),
            ("AUTONOMICS_OUTPUT3", "fake-deseq-dataset"),
            ("AUTONOMICS_OUTPUT4", "{\"schema_version\":\"1.0\"}\n"),
        ];
        for (name, contents) in outputs {
            let path = self.workspace_path(&request, &self.env(&request, name));
            std::fs::write(path, contents)?;
        }
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "fake DESeq2 run".into(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-deseq2"
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
        .expect("read published DESeq2 artifact")
        .to_vec()
}

#[tokio::test]
async fn stages_two_inputs_and_publishes_five_outputs() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/deseq2/fixtures");
    let counts = fixture_root.join("pasilla_gene_counts.tsv");
    let metadata = fixture_root.join("pasilla_sample_metadata.tsv");

    let runtime = Arc::new(FakeDeseq2Runtime::new(root.path()));
    let factory = Deseq2DeContainerNodeFactory::new(
        runtime.clone(),
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory
        .build(
            serde_json::json!({
                "condition_reference": "untreated",
                "condition_test": "treated",
                "covariates": ["type"],
                "artifact_prefix": "/artifacts/deseq2-container-test"
            }),
            ctx.clone(),
        )
        .unwrap();
    assert_eq!(node.kind(), DESEQ2_DE_CONTAINER_KIND);
    assert_eq!(node.ports().input_ports().len(), 2);
    assert_eq!(node.ports().output_ports().len(), 5);

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "counts".into(),
        Box::new(FileReferenceNode::new(
            counts.to_string_lossy().into_owned(),
            Some("deseq2_counts".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "metadata".into(),
        Box::new(FileReferenceNode::new(
            metadata.to_string_lossy().into_owned(),
            Some("deseq2_metadata".into()),
        )),
    )
    .unwrap();
    dag.add_node("deseq2".into(), node).unwrap();
    dag.add_edge("counts", "deseq2", 0, 0).unwrap();
    dag.add_edge("metadata", "deseq2", 0, 1).unwrap();
    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("deseq2"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "DESeq2 fake-runtime DAG failed: {report:#?}"
    );

    let request = runtime.requests.lock().unwrap()[0].clone();
    assert_eq!(
        request.image,
        nodes_io::image_registry::registry_image(
            nodes_io::deseq2_container::DESEQ2_IMAGE_REPOSITORY,
            nodes_io::deseq2_container::DESEQ2_IMAGE_DIGEST
        )
        .unwrap()
    );
    assert_eq!(
        request.network,
        container_runtime::ContainerNetwork::Isolated
    );
    assert!(request.read_only_rootfs);
    assert_eq!(request.cpus, Some(2.0));
    assert_eq!(request.memory.as_deref(), Some("4Gi"));
    let env: BTreeMap<_, _> = request.env.iter().cloned().collect();
    assert_eq!(env["AUTONOMICS_DESEQ2_CONDITION_REFERENCE"], "untreated");
    assert_eq!(env["AUTONOMICS_DESEQ2_CONDITION_TEST"], "treated");
    assert_eq!(env["AUTONOMICS_DESEQ2_COVARIATES"], "type");

    let staged_inputs = runtime.input_contents.lock().unwrap().clone();
    assert_eq!(staged_inputs.len(), 2);
    for (index, (path, contents)) in staged_inputs.iter().enumerate() {
        assert!(path.starts_with(container_runtime::DEFAULT_CONTAINER_WORKDIR));
        if index == 0 {
            assert!(contents.starts_with(b"gene_id\t"));
        } else {
            assert!(contents.starts_with(b"sample_id\t"));
        }
    }
    assert!(request.panels.is_empty());

    let outputs = dag.output("deseq2").unwrap();
    let expected_names = [
        "results.tsv",
        "normalized_counts.tsv",
        "size_factors.tsv",
        "deseq2_dataset.rds",
        "run_report.json",
    ];
    for (index, name) in expected_names.into_iter().enumerate() {
        let output = outputs.get(&(index as u8)).unwrap().as_file().unwrap();
        assert!(
            output
                .path
                .starts_with("vfs:///artifacts/deseq2-container-test/")
        );
        assert!(output.path.ends_with(&format!("/{name}")));
    }
    let results = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let bytes = read_published(&ctx, &results).await;
    assert_eq!(bytes, b"gene_id\tbaseMean\nFBgn1\t1\n");
}

#[ignore = "requires rootless Podman and the local pinned DESeq2 image"]
#[tokio::test]
async fn real_official_deseq2_runs_in_podman_and_matches_baseline() {
    // The wrapper pins the manifest digest and resolves it against the fixed
    // GHCR namespace; this ignored E2E test pulls the published image.
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/deseq2/fixtures");
    let counts = fixture_root.join("pasilla_gene_counts.tsv");
    let metadata = fixture_root.join("pasilla_sample_metadata.tsv");

    let workspace_root = root.path().join("podman-workspace");
    let panel_root = root.path().join("podman-panels");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&panel_root).unwrap();
    let runtime: Arc<dyn PodmanConnection> = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
        workspace_root: workspace_root.clone(),
        panel_cache_root: panel_root.clone(),
    }));
    let factory = Deseq2DeContainerNodeFactory::new(runtime, Arc::new(PanelCache::new(panel_root)));
    let node = factory
        .build(
            serde_json::json!({
                "condition_reference": "untreated",
                "condition_test": "treated",
                "covariates": ["type"],
                "artifact_prefix": "/artifacts/deseq2-real-podman"
            }),
            ctx.clone(),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "counts".into(),
        Box::new(FileReferenceNode::new(
            counts.to_string_lossy().into_owned(),
            Some("deseq2_counts".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "metadata".into(),
        Box::new(FileReferenceNode::new(
            metadata.to_string_lossy().into_owned(),
            Some("deseq2_metadata".into()),
        )),
    )
    .unwrap();
    dag.add_node("deseq2".into(), node).unwrap();
    dag.add_edge("counts", "deseq2", 0, 0).unwrap();
    dag.add_edge("metadata", "deseq2", 0, 1).unwrap();
    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("deseq2"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "official DESeq2 Podman DAG failed: {report:#?}"
    );

    let outputs = dag.output("deseq2").unwrap();
    let expected_sha256 = [
        (
            0,
            "397048d278d94c8e00489503394dbcc73c9478028f59dc7d68ff746615821f86",
        ),
        (
            1,
            "31ce2f093de15e660a69b14e34fe01b3b8550a9e7eb806c526dea6742d1f4eae",
        ),
        (
            2,
            "ffa196522d61fce29263f0ace1269467f7d1ce0773b0b786cf30bd3b9a2e6fec",
        ),
        (
            3,
            "93f7c4a12f614df26eff713811d9a367e53602012b8e568af439703654ce1d59",
        ),
    ];
    for (index, expected) in expected_sha256 {
        let file = outputs
            .get(&(index as u8))
            .unwrap()
            .as_file()
            .unwrap()
            .clone();
        let bytes = read_published(&ctx, &file).await;
        let digest = Sha256::digest(&bytes);
        assert_eq!(format!("{digest:x}"), expected, "output {index} changed");
    }

    let report_file = outputs.get(&4).unwrap().as_file().unwrap().clone();
    let report_bytes = read_published(&ctx, &report_file).await;
    let report: serde_json::Value = serde_json::from_slice(&report_bytes).unwrap();
    assert_eq!(report["engine"]["package"], "DESeq2");
    assert_eq!(report["engine"]["version"], "1.50.2");
    assert_eq!(report["analysis"]["design"], "~ type + condition");
    assert_eq!(report["dimensions"]["genes"], 14_599);
    assert_eq!(report["dimensions"]["samples"], 7);
    assert_eq!(report["result"]["independent_filtering_retained"], 8_799);
    assert_eq!(report["result"]["significant_at_alpha"], 1_330);
    assert_eq!(
        report["inputs"]["count_matrix"]["md5"],
        "f46bb780e3bf314a7002229625454b7b"
    );
    assert_eq!(
        report["inputs"]["sample_metadata"]["md5"],
        "eb2539b290570c64c0c7356091706b59"
    );

    let results_file = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let results = String::from_utf8(read_published(&ctx, &results_file).await).unwrap();
    assert!(results.contains("FBgn0261552\t5128.82302485314\t-1.8688179971032"));
    assert!(results.contains("\t1.62385615005662e-34\n"));
}
