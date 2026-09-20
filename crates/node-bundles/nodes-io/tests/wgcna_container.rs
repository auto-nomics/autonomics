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
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::wgcna_container::{
    WGCNA_CONTAINER_KIND, WGCNA_IMAGE_DIGEST, WGCNA_IMAGE_REPOSITORY, WgcnaContainerNodeFactory,
};
use vfs::OpendalFileStorage;

#[derive(Default)]
struct FakeWgcnaRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
}

impl FakeWgcnaRuntime {
    fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: Some(workspace_root.to_path_buf()),
            ..Default::default()
        }
    }
    fn env(&self, request: &ContainerRunRequest, name: &str) -> String {
        request
            .env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("missing `{name}` environment binding"))
    }
    fn workspace_path(&self, request: &ContainerRunRequest, path: &str) -> PathBuf {
        let relative = Path::new(path)
            .strip_prefix(container_runtime::DEFAULT_CONTAINER_WORKDIR)
            .unwrap();
        request.workspace.host_path.join(relative)
    }
}

#[async_trait]
impl PodmanConnection for FakeWgcnaRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        for index in 0..1 {
            let input = self.env(&request, &format!("AUTONOMICS_INPUT{index}"));
            let path = self.workspace_path(&request, &input);
            std::fs::write(path, b"gene_id\ts1\nA\t1.0\n")?;
        }
        let outputs = [
            ("AUTONOMICS_OUTPUT0", "power\tchosen_power\n12\t12\n"),
            ("AUTONOMICS_OUTPUT1", "module_label\tmodule_size\n1\t1\n"),
            ("AUTONOMICS_OUTPUT2", "blocks\tpersisted_tom_bytes\n1\t0\n"),
            (
                "AUTONOMICS_OUTPUT3",
                "gene_id\tmodule_label\tmodule_color\tkME1\nA\t1\tblue\t1.0\n",
            ),
            ("AUTONOMICS_OUTPUT4", "sample_id\tME1\ns1\t0.5\n"),
            ("AUTONOMICS_OUTPUT5", "{\"schema_version\":\"1.0\"}\n"),
        ];
        for (name, contents) in outputs {
            let path = self.workspace_path(&request, &self.env(&request, name));
            std::fs::write(path, contents)?;
        }
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "fake wgcna run".into(),
            stderr: String::new(),
        })
    }
    fn name(&self) -> &'static str {
        "fake-wgcna"
    }
    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

async fn read_published(ctx: &NodeCtx, file: &FileRef) -> Vec<u8> {
    let storage = ctx.opendal.as_ref().expect("test VFS storage");
    let path = file.path.strip_prefix("vfs://").expect("vfs URI");
    storage
        .resolve(path)
        .read(&storage.resolve_path(path))
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn stages_one_input_and_publishes_six_outputs() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let counts = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/deseq2/fixtures/pasilla_gene_counts.tsv");
    let runtime = Arc::new(FakeWgcnaRuntime::new(root.path()));
    let factory = WgcnaContainerNodeFactory::new(
        runtime.clone(),
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory.build(
        serde_json::json!({"network_type":"signed","min_module_size":30,"deep_split":2,"merge_threshold":0.25,"artifact_prefix":"/artifacts/wgcna-test"}),
        ctx.clone(),
    ).unwrap();
    assert_eq!(node.kind(), WGCNA_CONTAINER_KIND);
    assert_eq!(node.ports().output_ports().len(), 6);

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "expr".into(),
        Box::new(FileReferenceNode::new(
            counts.to_string_lossy().into_owned(),
            Some("wgcna_expr".into()),
        )),
    )
    .unwrap();
    dag.add_node("wgcna".into(), node).unwrap();
    dag.add_edge("expr", "wgcna", 0, 0).unwrap();
    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("wgcna"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let request = runtime.requests.lock().unwrap()[0].clone();
    assert_eq!(
        request.image,
        nodes_io::image_registry::acr_image(WGCNA_IMAGE_REPOSITORY, WGCNA_IMAGE_DIGEST).unwrap()
    );
    assert_eq!(
        request.network,
        container_runtime::ContainerNetwork::Isolated
    );
    let env: BTreeMap<_, _> = request.env.iter().cloned().collect();
    assert_eq!(env["AUTONOMICS_WGCNA_NETWORK_TYPE"], "signed");
    assert_eq!(env["AUTONOMICS_WGCNA_MIN_MODULE_SIZE"], "30");
    assert_eq!(env["AUTONOMICS_WGCNA_DEEP_SPLIT"], "2");

    let outputs = dag.output("wgcna").unwrap();
    let expected = [
        "soft_threshold.tsv",
        "adjacency_stats.tsv",
        "tom_stats.tsv",
        "modules.tsv",
        "module_eigengenes.tsv",
        "run_report.json",
    ];
    for (i, name) in expected.into_iter().enumerate() {
        let out = outputs.get(&(i as u8)).unwrap().as_file().unwrap();
        assert!(out.path.starts_with("vfs:///artifacts/wgcna-test/"));
        assert!(out.path.ends_with(&format!("/{name}")));
    }
    let modules = outputs.get(&3).unwrap().as_file().unwrap().clone();
    let bytes = read_published(&ctx, &modules).await;
    assert!(bytes.starts_with(b"gene_id\tmodule_label"));
}
