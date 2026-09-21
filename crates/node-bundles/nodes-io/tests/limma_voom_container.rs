use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache, PodmanConnection,
};
use dag_core::NodeFactory;
use dag_core::registry::NodeCtx;
use dag_core::value::FileRef;
use nodes_io::limma_voom_container::{
    LIMMA_VOOM_CONTAINER_KIND, LIMMA_VOOM_IMAGE_DIGEST, LIMMA_VOOM_IMAGE_REPOSITORY,
    LimmaVoomContainerNodeFactory,
};
use sha2::{Digest, Sha256};
use vfs::OpendalFileStorage;

#[derive(Default)]
struct FakeLimmaRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
    input_contents: Mutex<Vec<(String, Vec<u8>)>>,
}

impl FakeLimmaRuntime {
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
impl PodmanConnection for FakeLimmaRuntime {
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
            (
                "AUTONOMICS_OUTPUT0",
                "gene_id\tlogFC\tadj.P.Val\nA\t0.5\t0.01\n",
            ),
            ("AUTONOMICS_OUTPUT1", "gene_id\ts1\nA\t1.0\n"),
            ("AUTONOMICS_OUTPUT2", "gene_id\ts1\nA\t1.0\n"),
            ("AUTONOMICS_OUTPUT3", "outcome_mode\tcategorical\n"),
            ("AUTONOMICS_OUTPUT4", "{\"schema_version\":\"1.0\"}\n"),
        ];
        for (name, contents) in outputs {
            let path = self.workspace_path(&request, &self.env(&request, name));
            std::fs::write(path, contents)?;
        }
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "fake limma run".into(),
            stderr: String::new(),
        })
    }
    fn name(&self) -> &'static str {
        "fake-limma"
    }
    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

async fn read_published(ctx: &NodeCtx, file: &FileRef) -> Vec<u8> {
    let storage = ctx.opendal.as_ref().expect("test VFS storage");
    let path = file
        .path
        .strip_prefix("vfs://")
        .expect("container output uses vfs:// URI");
    storage
        .resolve(path)
        .read(&storage.resolve_path(path))
        .await
        .unwrap()
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
    let counts = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/deseq2/fixtures/pasilla_gene_counts.tsv");
    let metadata = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/deseq2/fixtures/pasilla_sample_metadata.tsv");

    let runtime = Arc::new(FakeLimmaRuntime::new(root.path()));
    let factory = LimmaVoomContainerNodeFactory::new(
        runtime.clone(),
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory
        .build(
            serde_json::json!({
                "outcome_var": "condition",
                "contrast_var": "condition",
                "contrast_levels": ["untreated", "treated"],
                "covariates": ["type"],
                "artifact_prefix": "/artifacts/limma-voom-test"
            }),
            ctx.clone(),
        )
        .unwrap();
    assert_eq!(node.kind(), LIMMA_VOOM_CONTAINER_KIND);
    assert_eq!(node.ports().input_ports().len(), 2);
    assert_eq!(node.ports().output_ports().len(), 5);

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "counts".into(),
        Box::new(nodes_io::file_reference::FileReferenceNode::new(
            counts.to_string_lossy().into_owned(),
            Some("limma_counts".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "metadata".into(),
        Box::new(nodes_io::file_reference::FileReferenceNode::new(
            metadata.to_string_lossy().into_owned(),
            Some("limma_metadata".into()),
        )),
    )
    .unwrap();
    dag.add_node("limma".into(), node).unwrap();
    dag.add_edge("counts", "limma", 0, 0).unwrap();
    dag.add_edge("metadata", "limma", 0, 1).unwrap();
    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("limma"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let request = runtime.requests.lock().unwrap()[0].clone();
    assert_eq!(
        request.image,
        nodes_io::image_registry::registry_image(
            LIMMA_VOOM_IMAGE_REPOSITORY,
            LIMMA_VOOM_IMAGE_DIGEST
        )
        .unwrap()
    );
    assert_eq!(
        request.network,
        container_runtime::ContainerNetwork::Isolated
    );
    assert!(request.read_only_rootfs);
    let env: BTreeMap<_, _> = request.env.iter().cloned().collect();
    assert_eq!(env["AUTONOMICS_LIMMA_OUTCOME"], "condition");
    assert_eq!(env["AUTONOMICS_LIMMA_CONTRAST_VAR"], "condition");
    assert_eq!(env["AUTONOMICS_LIMMA_NORMALIZATION"], "tmm");
    assert_eq!(env["AUTONOMICS_LIMMA_COVARIATES"], "type");

    let outputs = dag.output("limma").unwrap();
    let expected_names = [
        "results.tsv",
        "voom_weights.tsv",
        "normalized_expression.tsv",
        "contrast_summary.tsv",
        "run_report.json",
    ];
    for (index, name) in expected_names.into_iter().enumerate() {
        let output = outputs.get(&(index as u8)).unwrap().as_file().unwrap();
        assert!(output.path.starts_with("vfs:///artifacts/limma-voom-test/"));
        assert!(output.path.ends_with(&format!("/{name}")));
    }
    let results = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let bytes = read_published(&ctx, &results).await;
    assert_eq!(bytes, b"gene_id\tlogFC\tadj.P.Val\nA\t0.5\t0.01\n");
}

#[test]
fn rejects_contrast_var_that_differs_from_outcome() {
    let spec = serde_json::json!({
        "outcome_var": "condition",
        "contrast_var": "duration",
        "contrast_levels": ["ctrl", "case"],
        "covariates": [],
    });
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let runtime = Arc::new(FakeLimmaRuntime::new(root.path()));
    let factory = LimmaVoomContainerNodeFactory::new(
        runtime,
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let err = factory
        .build(spec, ctx)
        .err()
        .expect("build should reject a mismatched contrast_var");
    assert!(format!("{err:?}").contains("contrast_var must equal"));
}
