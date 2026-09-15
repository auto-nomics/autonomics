use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache, PodmanConfig,
    PodmanConnection, PodmanRuntime,
};
use dag_core::registry::{NodeCtx, NodeFactory};
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::mutation_analysis_container::{
    MUTATION_ANALYSIS_CONTAINER_KIND, MutationAnalysisContainerNodeFactory,
};
use vfs::OpendalFileStorage;

fn mutation_config() -> serde_json::Value {
    serde_json::json!({
        "maf_path": "/data/sample.maf",
        "clinical_path": "/data/clinical.tsv",
        "operation": "tmb",
        "panel_size_mb": 38.0,
        "tmb_group_col": "group",
        "tmb_groups": ["primary", "control"],
        "top_n": 20,
        "artifact_prefix": "/artifacts/mutation-analysis-test"
    })
}

fn write_fixture(root: &Path) -> (PathBuf, PathBuf) {
    let workspace = root.join("fixtures");
    std::fs::create_dir_all(&workspace).unwrap();
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/maf/sample_mutations.maf");
    let maf = workspace.join("sample.maf");
    std::fs::copy(source, &maf).unwrap();
    let clinical = workspace.join("clinical.tsv");
    std::fs::write(
        &clinical,
        "sample_id\tgroup\nsample_a\tprimary\nsample_b\tcontrol\nsample_c\tprimary\nsample_d\tcontrol\n",
    )
    .unwrap();
    (maf, clinical)
}

#[derive(Default)]
struct FakeMutationRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
}

impl FakeMutationRuntime {
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
impl PodmanConnection for FakeMutationRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        let report = self.env(&request, "AUTONOMICS_OUTPUT0");
        let details = self.env(&request, "AUTONOMICS_OUTPUT1");
        std::fs::write(self.workspace_path(&request, &report), "sample_id\tgroup\n").unwrap();
        std::fs::write(
            self.workspace_path(&request, &details),
            br#"{"operation":"tmb"}"#,
        )
        .unwrap();
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-mutation"
    }

    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

fn add_fixture_dag(
    dag: &mut dag_core::dag::DAG,
    node_name: &str,
    node: Box<dyn dag_core::node::DagNode>,
    maf: &Path,
    clinical: &Path,
) {
    dag.add_node(
        "maf".into(),
        Box::new(FileReferenceNode::new(
            maf.to_string_lossy().into_owned(),
            Some("maf".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "clinical".into(),
        Box::new(FileReferenceNode::new(
            clinical.to_string_lossy().into_owned(),
            Some("clinical".into()),
        )),
    )
    .unwrap();
    dag.add_node(node_name.into(), node).unwrap();
    dag.add_edge("maf", node_name, 0, 0).unwrap();
    dag.add_edge("clinical", node_name, 0, 1).unwrap();
}

#[tokio::test]
async fn stages_maf_and_clinical_and_publishes_two_outputs() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let (maf, clinical) = write_fixture(root.path());

    let runtime = Arc::new(FakeMutationRuntime::new(root.path()));
    let factory = MutationAnalysisContainerNodeFactory::new(
        runtime.clone(),
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let node = factory.build(mutation_config(), ctx.clone()).unwrap();
    assert_eq!(node.kind(), MUTATION_ANALYSIS_CONTAINER_KIND);
    assert_eq!(node.ports().input_ports().len(), 2);
    assert_eq!(node.ports().output_ports().len(), 2);

    let mut dag = dag_core::dag::DAG::default();
    add_fixture_dag(&mut dag, "mutation", node, &maf, &clinical);
    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("mutation"),
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
            "/opt/autonomics/mutation_analysis.R".to_string()
        ]
    );
    let request_env = |name: &str| {
        runtime
            .env(&request, name)
            .starts_with(container_runtime::DEFAULT_CONTAINER_WORKDIR)
    };
    for name in ["AUTONOMICS_INPUT0", "AUTONOMICS_INPUT1"] {
        assert!(request_env(name));
    }

    let outputs = dag.output("mutation").unwrap();
    outputs.get(&0).unwrap().as_dataframe().unwrap();
    let details_file = outputs.get(&1).unwrap().as_file().unwrap();
    assert!(
        details_file
            .path
            .starts_with("vfs:///artifacts/mutation-analysis-test/")
    );
    assert!(
        details_file
            .path
            .ends_with("/mutation_analysis_details.json")
    );
}

#[tokio::test]
async fn real_podman_runs_the_pinned_tmb_contract_when_gated() {
    if std::env::var("AUTONOMICS_MUTATION_E2E").as_deref() != Ok("1") {
        eprintln!("skipping: set AUTONOMICS_MUTATION_E2E=1 to run rootless Podman e2e");
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage.clone()),
    );
    let (maf, clinical) = write_fixture(root.path());
    let workspace_root = root.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let runtime = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
        workspace_root,
        panel_cache_root: root.path().join("panels"),
    }));
    let factory = MutationAnalysisContainerNodeFactory::new(
        runtime,
        Arc::new(PanelCache::new(root.path().join("panels"))),
    );
    let mut config = mutation_config();
    config["artifact_prefix"] = "/artifacts/mutation-analysis-real".into();
    let node = factory.build(config, ctx.clone()).unwrap();

    let mut dag = dag_core::dag::DAG::default();
    add_fixture_dag(&mut dag, "mutation", node, &maf, &clinical);
    let report = dag
        .run(
            &dag_core::dag::runtime::SchedulerConfig::default(),
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("mutation"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "real mutation node failed: {report:#?}"
    );

    let details_file = dag
        .output("mutation")
        .unwrap()
        .get(&1)
        .unwrap()
        .as_file()
        .unwrap()
        .clone();
    let virtual_path = details_file.path.strip_prefix("vfs://").unwrap();
    let bytes = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    let details: serde_json::Value = serde_json::from_slice(&bytes.to_vec()).unwrap();
    assert_eq!(details["operation"], "tmb");
    assert_eq!(details["variant_count"], 11);
    assert_eq!(details["group_comparison"], "wilcox:primary_vs_control");
}
