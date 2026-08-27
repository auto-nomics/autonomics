//! End-to-end file-flow tests for `dataframe_to_file -> container_command -> file_to_dataframe`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntime, ContainerRuntimeError,
    DEFAULT_CONTAINER_WORKDIR, K3sConfig, K3sRuntime, PanelCache, PullPolicy,
    unique_container_name,
};
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::runtime::SchedulerConfig;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeRegistry};
use data_catalog::{CatalogConfig, CatalogRuntime};
use datafusion::common::HashMap;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::SessionContext;
use flate2::read::GzDecoder;

use nodes_io::coloc_abf_container::{COLOC_ABF_CONTAINER_KIND, ColocAbfContainerNodeFactory};
use nodes_io::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use nodes_io::dataframe_to_file::{DataFrameToFileNode, WriteFormat};
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::file_to_dataframe::FileToDataFrameNode;
use nodes_io::gcta_container::{
    GCTA_ACAT_CONTAINER_KIND, GCTA_COJO_SELECT_CONTAINER_KIND, GCTA_FASTBAT_CONTAINER_KIND,
    GCTA_GENE_LIST_PANEL, GCTA_REF_BINARY_PANEL, GCTA_SBLUP_CONTAINER_KIND,
    GctaContainerNodeFactory,
};
use nodes_io::hyprcoloc_container::{HYPRCOLOC_CONTAINER_KIND, HyPrColocContainerNodeFactory};
use nodes_io::lava_container::{
    LAVA_CONTAINER_KIND, LAVA_TUTORIAL_REF_PANEL, LAVA_UKB_EUR_PANEL, LavaContainerNodeFactory,
};
use nodes_io::ldsc_h2_container::{LDSC_H2_CONTAINER_KIND, LdscH2ContainerNodeFactory};
use nodes_io::ldsc_munge_container::{LDSC_MUNGE_CONTAINER_KIND, LdscMungeContainerNodeFactory};
use nodes_io::ldsc_rg_container::{LDSC_RG_CONTAINER_KIND, LdscRgContainerNodeFactory};
use nodes_io::magma_annotate_container::{
    MAGMA_ANNOTATE_CONTAINER_KIND, MAGMA_GENE_LOC_PANEL, MagmaAnnotateContainerNodeFactory,
};
use nodes_io::mixer_container::{
    MIXER_FIT1_CONTAINER_KIND, MIXER_FIT2_CONTAINER_KIND, MIXER_G1000_EUR_PANEL,
    MixerFit1ContainerNodeFactory, MixerFit2ContainerNodeFactory,
};
use nodes_io::mrpresso_container::{MRPRESSO_CONTAINER_KIND, MrpressoContainerNodeFactory};
use nodes_io::mvmr_container::{MVMR_CONTAINER_KIND, MvmrContainerNodeFactory};
use nodes_io::plink2_clump_container::{
    PLINK2_CLUMP_CONTAINER_KIND, PLINK2_REF_BINARY_PANEL, Plink2ClumpContainerNodeFactory,
};
use nodes_io::smr_heidi_container::{
    SMR_HEIDI_CONTAINER_KIND, SMR_REF_BINARY_PANEL, SMR_WESTRA_EQTL_PANEL,
    SmrHeidiContainerNodeFactory,
};
use nodes_io::susie_rss_container::{
    SUSIE_REF_PANEL, SUSIE_RSS_CONTAINER_KIND, SusieRssContainerNodeFactory,
};
use nodes_io::twas_fusion_container::{
    FUSION_GTEX_V8_PANEL, TWAS_FUSION_CONTAINER_KIND, TwasFusionContainerNodeFactory,
};
use sha2::{Digest, Sha256};
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

async fn run_ldsc_h2_dag(
    ctx: &NodeCtx,
    registry: &NodeRegistry,
    input_path: &Path,
    format: &str,
) -> String {
    let ldsc = registry
        .build_node(LDSC_H2_CONTAINER_KIND, serde_json::json!({}))
        .unwrap();
    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "sumstats".into(),
        Box::new(FileReferenceNode::new(
            input_path.to_string_lossy().into_owned(),
            Some(format.into()),
        )),
    )
    .unwrap();
    dag.add_node("ldsc_h2".into(), ldsc).unwrap();
    dag.add_edge("sumstats", "ldsc_h2", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("sumstats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("ldsc_h2"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let output = dag
        .output("ldsc_h2")
        .unwrap()
        .get(&0)
        .unwrap()
        .as_file()
        .unwrap()
        .clone();
    assert!(
        output
            .path
            .starts_with("vfs:///artifacts/ldsc_h2_container/")
    );
    assert!(output.path.ends_with("/ldsc_h2.log"));
    let virtual_path = output
        .path
        .strip_prefix("vfs://")
        .expect("LDSC h2 artifact is a VFS URI");
    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    let published = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    String::from_utf8_lossy(&published.to_vec()).into_owned()
}

#[derive(Clone)]
struct DataFrameSourceNode {
    ports: NodePorts,
    batch: RecordBatch,
}

#[async_trait]
impl DagNode for DataFrameSourceNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "container_file_flow_test_source"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, dag_core::dag::DagError> {
        let dataframe = ctx.session().read_batch(self.batch.clone())?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

#[derive(Default)]
struct FakeContainerRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
}

impl FakeContainerRuntime {
    fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: Some(workspace_root.to_path_buf()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn workspace_path(&self, request: &ContainerRunRequest, path: &str) -> PathBuf {
        let relative = Path::new(path)
            .strip_prefix(DEFAULT_CONTAINER_WORKDIR)
            .expect("runtime path is inside /work");
        request.workspace.host_path.join(relative)
    }
}

#[async_trait]
impl ContainerRuntime for FakeContainerRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        let input = request
            .env
            .iter()
            .find(|(name, _)| name == "AUTONOMICS_INPUT0")
            .map(|(_, value)| value.clone())
            .ok_or_else(|| ContainerRuntimeError::Invalid("missing input binding".into()))?;
        let output = request
            .env
            .iter()
            .find(|(name, _)| name == "AUTONOMICS_OUTPUT0")
            .map(|(_, value)| value.clone())
            .ok_or_else(|| ContainerRuntimeError::Invalid("missing output binding".into()))?;
        std::fs::copy(
            self.workspace_path(&request, &input),
            self.workspace_path(&request, &output),
        )?;
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "copied by fake runtime".into(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-container-file-flow"
    }

    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

fn sample_batch() -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![1, 2])),
            Arc::new(StringArray::from(vec!["alpha", "beta"])),
        ],
    )
    .unwrap()
}

fn workspace_vfs(workspace_root: &Path) -> (Arc<MountedObjectStore>, NodeCtx) {
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "workspace".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "workspace".into(),
            source: workspace_root.to_string_lossy().into_owned(),
            read_only: false,
        }],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(OpendalFileStorage::with_mounts(
        workspace_root,
        mounted.clone(),
    ));
    let session = SessionContext::new();
    session.runtime_env().register_object_store(
        ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
        mounted.clone(),
    );
    (mounted, NodeCtx::new(session.runtime_env(), Some(storage)))
}

fn container_spec(image: &str, workspace: &Path, artifact_prefix: String) -> ContainerCommandSpec {
    ContainerCommandSpec {
        image: image.into(),
        command: vec![
            "cp".into(),
            "--".into(),
            "$input0".into(),
            "$output0".into(),
        ],
        script: None,
        files: Default::default(),
        env: Default::default(),
        outputs: vec![ContainerCommandOutputSpec {
            path: "copied.csv".into(),
            format: Some("csv".into()),
        }],
        workdir: Some(workspace.to_string_lossy().into_owned()),
        artifact_prefix,
        timeout_secs: 120,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Never,
        cpus: None,
        memory: None,
        pids_limit: None,
        shm_size: None,
        user: None,
    }
}

#[track_caller]
fn assert_successful_dag(statuses: &HashMap<String, dag_core::dag::RuntimeStatus>) {
    for id in ["source", "write_input", "container", "read_result"] {
        assert_eq!(
            statuses.get(id),
            Some(&dag_core::dag::RuntimeStatus::Success),
            "node `{id}` did not succeed"
        );
    }
}

async fn assert_container_artifact(
    ctx: &NodeCtx,
    output: &dag_core::value::FileRef,
    workspace_output: &Path,
) {
    assert!(
        output
            .path
            .starts_with("vfs:///artifacts/container-file-flow/"),
        "container output is not a VFS artifact: {}",
        output.path
    );
    assert!(output.path.ends_with("/copied.csv"));
    assert_eq!(output.format.as_deref(), Some("csv"));

    let expected_bytes = tokio::fs::read(workspace_output)
        .await
        .expect("read workspace output copied by the container");
    let fingerprint = output
        .fingerprint
        .as_ref()
        .expect("container output has a fingerprint");
    assert_eq!(fingerprint.size, expected_bytes.len() as u64);

    let digest = Sha256::digest(&expected_bytes);
    assert_eq!(
        fingerprint.content_hash.as_deref(),
        Some(format!("sha256:{:x}", digest).as_str()),
        "container output fingerprint must match the workspace file"
    );

    let storage = ctx.opendal.as_ref().expect("test VFS storage");
    let virtual_path = output
        .path
        .strip_prefix("vfs://")
        .expect("container output path is a vfs:// URI");
    let actual_bytes = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .expect("read published container output through VFS");
    assert_eq!(
        actual_bytes.to_vec(),
        expected_bytes,
        "downstream nodes must be able to read the exact container output artifact"
    );
}

#[tokio::test]
async fn dataframe_to_file_output_flows_through_container_command_in_dag() {
    let workspace = tempfile::tempdir().unwrap();
    let (_mounted, ctx) = workspace_vfs(workspace.path());
    let input_path = format!("vfs:///input-{}.csv", unique_container_name());
    let run_root = workspace.path().join("fake-run");
    std::fs::create_dir_all(&run_root).unwrap();
    let runtime = Arc::new(FakeContainerRuntime::new(workspace.path()));
    let mut spec = container_spec(
        "localhost/container-file-flow-test:not-present",
        &run_root,
        "/artifacts/container-file-flow/fake".into(),
    );
    spec.pull_policy = PullPolicy::Never;

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "source".into(),
        Box::new(DataFrameSourceNode {
            ports: NodePorts::new().add_output_port(None),
            batch: sample_batch(),
        }),
    )
    .unwrap();
    dag.add_node(
        "write_input".into(),
        Box::new(DataFrameToFileNode::new(
            input_path,
            WriteFormat::Csv,
            dag_core::SinkMode::Overwrite,
        )),
    )
    .unwrap();
    dag.add_node(
        "container".into(),
        Box::new(
            ContainerCommandNode::new(
                spec,
                runtime.clone(),
                Arc::new(PanelCache::new(workspace.path().join("panels"), "")),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    dag.add_node(
        "read_result".into(),
        Box::new(FileToDataFrameNode::new(None, None)),
    )
    .unwrap();
    dag.add_edge("source", "write_input", 0, 0).unwrap();
    dag.add_edge("write_input", "container", 0, 0).unwrap();
    dag.add_edge("container", "read_result", 0, 0).unwrap();

    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_successful_dag(&report.statuses);

    let request = runtime.requests.lock().unwrap()[0].clone();
    let input = request
        .env
        .iter()
        .find(|(name, _)| name == "AUTONOMICS_INPUT0")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert_eq!(
        input,
        format!("{DEFAULT_CONTAINER_WORKDIR}/.autonomics/inputs/input-0.csv")
    );
    assert!(
        request
            .command
            .contains(&format!("{DEFAULT_CONTAINER_WORKDIR}/copied.csv"))
    );

    let container_output = dag
        .output("container")
        .unwrap()
        .get(&0)
        .unwrap()
        .as_file()
        .unwrap()
        .clone();
    assert_container_artifact(&ctx, &container_output, &run_root.join("copied.csv")).await;

    let output = dag.output("read_result").unwrap();
    let dataframe = output.get(&0).unwrap().as_dataframe().unwrap().clone();
    let collected = dataframe.collect().await.unwrap();
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].num_rows(), 2);
    let ids = collected[0]
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    assert_eq!(ids.values(), &[1, 2]);
}

#[tokio::test]
#[ignore = "requires a configured k3s cluster, shared workspace PVC, kubeconfig, and local Debian image"]
async fn real_k3s_container_receives_upstream_dataframe_to_file_output() {
    let workspace_root = std::env::var_os("AUTONOMICS_K3S_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .expect("AUTONOMICS_K3S_WORKSPACE_ROOT must point to the shared workspace PVC path");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let (_mounted, ctx) = workspace_vfs(&workspace_root);
    let suffix = unique_container_name();
    let input_path = format!("vfs:///container-file-flow-{suffix}/input.csv");
    let run_root = workspace_root.join(format!("container-file-flow-{suffix}"));
    std::fs::create_dir_all(&run_root).unwrap();

    let mut k3s_config = K3sConfig::from_env();
    k3s_config.workspace_root = workspace_root;
    let image = std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
        .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into());
    let spec = container_spec(
        &image,
        &run_root,
        format!("/artifacts/container-file-flow/real/{suffix}"),
    );
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root,
        k3s_config.panel_pvc_prefix,
    ));

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "source".into(),
        Box::new(DataFrameSourceNode {
            ports: NodePorts::new().add_output_port(None),
            batch: sample_batch(),
        }),
    )
    .unwrap();
    dag.add_node(
        "write_input".into(),
        Box::new(DataFrameToFileNode::new(
            input_path,
            WriteFormat::Csv,
            dag_core::SinkMode::Overwrite,
        )),
    )
    .unwrap();
    dag.add_node(
        "container".into(),
        Box::new(ContainerCommandNode::new(spec, runtime, panel_cache).unwrap()),
    )
    .unwrap();
    dag.add_node(
        "read_result".into(),
        Box::new(FileToDataFrameNode::new(None, None)),
    )
    .unwrap();
    dag.add_edge("source", "write_input", 0, 0).unwrap();
    dag.add_edge("write_input", "container", 0, 0).unwrap();
    dag.add_edge("container", "read_result", 0, 0).unwrap();

    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_successful_dag(&report.statuses);

    let container_output = dag
        .output("container")
        .unwrap()
        .get(&0)
        .unwrap()
        .as_file()
        .unwrap()
        .clone();
    assert_container_artifact(&ctx, &container_output, &run_root.join("copied.csv")).await;

    let output = dag.output("read_result").unwrap();
    let dataframe = output.get(&0).unwrap().as_dataframe().unwrap().clone();
    let collected = dataframe.collect().await.unwrap();
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].num_rows(), 2);
    let ids = collected[0]
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    assert_eq!(ids.values(), &[1, 2]);
}

#[tokio::test]
#[ignore = "requires Garage catalog panels, k3s PVCs, kubeconfig, the local LDSC image, and test sumstats"]
async fn real_catalog_backed_original_ldsc_h2_accepts_tsv_and_gz_in_k3s() {
    let config_path = std::env::var_os("AUTONOMICS_TEST_VFS_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| Path::new(&home).join(".autonomics/vfs.toml"))
        })
        .expect("HOME or AUTONOMICS_TEST_VFS_CONFIG is required");
    let source = std::fs::read_to_string(&config_path).unwrap();
    let catalog_manifest = VfsManifest::from_toml(&source).unwrap();
    let catalog_config = CatalogConfig::from_vfs_toml(&source).unwrap();
    let catalog_runtime = CatalogRuntime::load(&catalog_manifest, &catalog_config)
        .await
        .unwrap();
    let bundles = catalog_runtime.data_bundles();
    let panel_ids = [
        "ldsc.ref_ld.1000g_eur.basic",
        "ldsc.w_ld.1000g_eur_hm3_no_mhc",
    ];
    for id in panel_ids {
        assert!(bundles.get(id).is_some(), "catalog is missing {id}");
    }

    let scratch = tempfile::tempdir().unwrap();
    let catalog_backend = catalog_manifest
        .backend
        .iter()
        .find(|backend| backend.id == catalog_config.backend)
        .expect("catalog backend is defined");
    let mut manifest = VfsManifest {
        backend: vec![
            catalog_backend.clone(),
            BackendDefinition {
                id: "ldsc-test-local".into(),
                config: BackendConfig::local("/"),
            },
        ],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "ldsc-test-local".into(),
            source: scratch.path().to_string_lossy().into_owned(),
            read_only: false,
        }],
    };
    manifest.mount.extend(
        data_catalog::catalog_mount_definitions(&manifest, &catalog_runtime.index, &catalog_config)
            .unwrap(),
    );
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(OpendalFileStorage::with_mounts(
        scratch.path(),
        mounted.clone(),
    ));
    let session = SessionContext::new();
    session
        .runtime_env()
        .register_object_store(ObjectStoreUrl::parse("vfs://").unwrap().as_ref(), mounted);
    let ctx = NodeCtx::new(session.runtime_env(), Some(storage));

    let k3s_config = K3sConfig::from_env();
    let sumstats_path = std::env::var_os("AUTONOMICS_LDSC_IT_SUMSTATS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new("/mnt/data/ldsc_data/sumstats_107/GBMI.Asthma.sumstats.gz").to_path_buf()
        });
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = ctx.clone().with_data_bundle_catalog(Arc::new(bundles));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(LdscH2ContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));

    let tsv_path = scratch.path().join("GBMI.Asthma.sumstats.tsv");
    {
        let mut decoder = GzDecoder::new(std::fs::File::open(&sumstats_path).unwrap());
        let mut output = std::fs::File::create(&tsv_path).unwrap();
        std::io::copy(&mut decoder, &mut output).unwrap();
        output.flush().unwrap();
    }

    let gz_log = run_ldsc_h2_dag(&ctx, &registry, &sumstats_path, "sumstats_gz").await;
    let tsv_log = run_ldsc_h2_dag(&ctx, &registry, &tsv_path, "sumstats_tsv").await;
    assert!(!tsv_log.contains("RuntimeWarning: compression has no effect"));
    for log in [&gz_log, &tsv_log] {
        assert!(
            log.contains("Total Observed scale h2: 0.0196 (0.0014)"),
            "unexpected LDSC h2 result:\n{log}"
        );
        assert!(
            log.contains("Intercept: 1.1516 (0.0132)"),
            "unexpected LDSC intercept:\n{log}"
        );
    }

    let cached_panels = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            panel_ids
                .iter()
                .any(|id| name.starts_with(&format!("{id}@")))
        })
        .count();
    assert_eq!(
        cached_panels, 2,
        "both native LDSC panels should remain in PanelCache"
    );
}

struct CatalogTextFixture {
    ctx: NodeCtx,
    bundles: dag_core::DataBundleCatalog,
    _scratch: tempfile::TempDir,
}

#[tokio::test]
#[ignore = "requires k3s PVCs, kubeconfig, and the local official LDSC image"]
async fn real_official_ldsc_munge_runs_in_k3s() {
    let scratch = tempfile::tempdir().unwrap();
    let input_path = scratch.path().join("raw-daner.sumstats");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../containers/ldsc/ldsc-python3/test/munge_test/sumstats"),
        &input_path,
    )
    .unwrap();
    let (_mounted, ctx) = workspace_vfs(scratch.path());
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let mut registry = NodeRegistry::new(ctx.clone());
    registry.register(Box::new(LdscMungeContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let munge = registry
        .build_node(
            LDSC_MUNGE_CONTAINER_KIND,
            serde_json::json!({"daner": true, "chunksize": 100}),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "raw_sumstats".into(),
        Box::new(FileReferenceNode::new(
            input_path.to_string_lossy().into_owned(),
            Some("sumstats".into()),
        )),
    )
    .unwrap();
    dag.add_node("ldsc_munge".into(), munge).unwrap();
    dag.add_edge("raw_sumstats", "ldsc_munge", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("raw_sumstats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("ldsc_munge"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("ldsc_munge").unwrap();
    let sumstats = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(sumstats.path.ends_with("/munged.sumstats.gz"));
    assert!(log.path.ends_with("/munge_sumstats.log"));
    assert!(
        sumstats
            .path
            .starts_with("vfs:///artifacts/ldsc_munge_container/")
    );

    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    let sumstats_vpath = sumstats
        .path
        .strip_prefix("vfs://")
        .expect("munged sumstats is a VFS URI");
    let bytes = storage
        .resolve(sumstats_vpath)
        .read(&storage.resolve_path(sumstats_vpath))
        .await
        .unwrap();
    let mut text = String::new();
    let bytes = bytes.to_vec();
    GzDecoder::new(&bytes[..])
        .read_to_string(&mut text)
        .unwrap();
    assert!(text.starts_with("SNP\tA1\tA2\tZ\tN\n"));
    assert_eq!(text.lines().count(), 5, "munged output:\n{text}");

    let log_vpath = log
        .path
        .strip_prefix("vfs://")
        .expect("munge log is a VFS URI");
    let bytes = storage
        .resolve(log_vpath)
        .read(&storage.resolve_path(log_vpath))
        .await
        .unwrap();
    let log_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    for expected in [
        "Version 3.0.1",
        "4 SNPs remain",
        "Writing summary statistics",
    ] {
        assert!(
            log_text.contains(expected),
            "official LDSC munge baseline is missing `{expected}`:\n{log_text}"
        );
    }
}

async fn catalog_test_fixture() -> CatalogTextFixture {
    let config_path = std::env::var_os("AUTONOMICS_TEST_VFS_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| Path::new(&home).join(".autonomics/vfs.toml"))
        })
        .expect("HOME or AUTONOMICS_TEST_VFS_CONFIG is required");
    let source = std::fs::read_to_string(&config_path).unwrap();
    let catalog_manifest = VfsManifest::from_toml(&source).unwrap();
    let catalog_config = CatalogConfig::from_vfs_toml(&source).unwrap();
    let catalog_runtime = CatalogRuntime::load(&catalog_manifest, &catalog_config)
        .await
        .unwrap();

    let scratch = tempfile::tempdir().unwrap();
    let catalog_backend = catalog_manifest
        .backend
        .iter()
        .find(|backend| backend.id == catalog_config.backend)
        .expect("catalog backend is defined");
    let mut manifest = VfsManifest {
        backend: vec![
            catalog_backend.clone(),
            BackendDefinition {
                id: "catalog-test-local".into(),
                config: BackendConfig::local("/"),
            },
        ],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "catalog-test-local".into(),
            source: scratch.path().to_string_lossy().into_owned(),
            read_only: false,
        }],
    };
    manifest.mount.extend(
        data_catalog::catalog_mount_definitions(&manifest, &catalog_runtime.index, &catalog_config)
            .unwrap(),
    );
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(OpendalFileStorage::with_mounts(
        scratch.path(),
        mounted.clone(),
    ));
    let session = SessionContext::new();
    session
        .runtime_env()
        .register_object_store(ObjectStoreUrl::parse("vfs://").unwrap().as_ref(), mounted);
    CatalogTextFixture {
        ctx: NodeCtx::new(session.runtime_env(), Some(storage)),
        bundles: catalog_runtime.data_bundles(),
        _scratch: scratch,
    }
}

#[tokio::test]
#[ignore = "requires the Garage gene-location panel, k3s PVCs, kubeconfig, and the local MAGMA image"]
async fn real_catalog_backed_official_magma_annotate_runs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    assert!(fixture.bundles.get(MAGMA_GENE_LOC_PANEL).is_some());

    let snp_loc_path = std::env::var_os("AUTONOMICS_MAGMA_IT_SNP_LOC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new("/mnt/data/magma/results/smoke_test.annotation.snp.loc").to_path_buf()
        });
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(MagmaAnnotateContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let magma = registry
        .build_node(MAGMA_ANNOTATE_CONTAINER_KIND, serde_json::json!({}))
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "snp_locations".into(),
        Box::new(FileReferenceNode::new(
            snp_loc_path.to_string_lossy().into_owned(),
            Some("magma_snp_loc".into()),
        )),
    )
    .unwrap();
    dag.add_node("magma_annotate".into(), magma).unwrap();
    dag.add_edge("snp_locations", "magma_annotate", 0, 0)
        .unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("snp_locations"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("magma_annotate"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("magma_annotate").unwrap();
    let log = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let annotation = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(log.path.ends_with("/magma_annotate.log"));
    assert!(annotation.path.ends_with("/magma_annotate.genes.annot"));
    assert!(
        annotation
            .path
            .starts_with("vfs:///artifacts/magma_annotate_container/")
    );

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let annotation_path = annotation
        .path
        .strip_prefix("vfs://")
        .expect("MAGMA annotation is a VFS URI");
    let published = storage
        .resolve(annotation_path)
        .read(&storage.resolve_path(annotation_path))
        .await
        .unwrap();
    let annotation = String::from_utf8_lossy(&published.to_vec()).into_owned();
    assert!(annotation.contains("79501\t1:69091:70008"));
    assert!(annotation.contains("rs140739101"));

    let cached_panel = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{MAGMA_GENE_LOC_PANEL}@"))
        });
    assert!(cached_panel, "MAGMA gene-location panel should be cached");
}

#[tokio::test]
#[ignore = "requires the Garage LAVA tutorial panel, k3s PVCs, kubeconfig, the local official LAVA image, and zip"]
async fn real_catalog_backed_official_lava_univ_runs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    assert!(fixture.bundles.get(LAVA_TUTORIAL_REF_PANEL).is_some());

    let scratch = tempfile::tempdir().unwrap();
    let source = std::env::var_os("AUTONOMICS_LAVA_IT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/lava/LAVA")
        });
    let fixture_data = source.join("vignettes/data");
    let bundle_root = scratch.path().join("bundle");
    let bundle_data = bundle_root.join("vignettes/data");
    std::fs::create_dir_all(&bundle_data).unwrap();
    let bundle_files = [
        "input.info.txt",
        "sample.overlap.txt",
        "test.loci",
        "depression.sumstats.txt",
        "neuro.sumstats.txt",
        "bmi.sumstats.txt",
    ];
    for name in bundle_files {
        std::fs::copy(fixture_data.join(name), bundle_data.join(name)).unwrap();
    }
    let bundle_zip = scratch.path().join("lava-univ-bundle.zip");
    let zip_status = Command::new("zip")
        .args(["-q", "-r"])
        .arg(&bundle_zip)
        .arg("vignettes")
        .current_dir(&bundle_root)
        .status()
        .unwrap();
    assert!(zip_status.success(), "could not create the LAVA run bundle");

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(LavaContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let lava = registry
        .build_node(
            LAVA_CONTAINER_KIND,
            serde_json::json!({
                "analysis": "univ",
                "panel_id": nodes_io::lava_container::LAVA_TUTORIAL_REF_PANEL,
                "input_info_file": "vignettes/data/input.info.txt",
                "loci_file": "vignettes/data/test.loci",
                "sample_overlap_file": "vignettes/data/sample.overlap.txt",
                "locus_index": 1,
                "phenotypes": ["depression", "neuro", "bmi"]
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "bundle".into(),
        Box::new(FileReferenceNode::new(
            bundle_zip.to_string_lossy().into_owned(),
            Some("lava_run_bundle_zip".into()),
        )),
    )
    .unwrap();
    dag.add_node("lava_univ".into(), lava).unwrap();
    dag.add_edge("bundle", "lava_univ", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("bundle"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("lava_univ"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "LAVA node failed: {report:#?}"
    );

    let outputs = dag.output("lava_univ").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let rds = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(
        rds.fingerprint
            .as_ref()
            .expect("LAVA RDS artifact has a fingerprint")
            .size
            > 0
    );
    assert!(tsv.path.ends_with("/lava.tsv"));
    assert!(rds.path.ends_with("/lava.RDS"));
    assert!(log.path.ends_with("/lava.log"));
    assert!(tsv.path.starts_with("vfs:///artifacts/lava_container/"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    async fn published_text(
        storage: &OpendalFileStorage,
        output: &dag_core::value::FileRef,
    ) -> String {
        let path = output
            .path
            .strip_prefix("vfs://")
            .expect("LAVA artifact is a VFS URI");
        let bytes = storage
            .resolve(path)
            .read(&storage.resolve_path(path))
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes.to_vec()).into_owned()
    }

    let tsv = published_text(storage, &tsv).await;
    let log = published_text(storage, &log).await;
    assert!(tsv.contains("phen\th2.obs\th2.latent\tascertained\tp"));
    for expected in [
        "depression\t8.45733e-05\t0.000141198\tFALSE\t0.036558",
        "neuro\t0.000116406\tNA\tFALSE\t0.0315434",
        "bmi\t0.000193535\tNA\tFALSE\t0.00146622",
    ] {
        assert!(
            tsv.contains(expected),
            "official LAVA univ baseline is missing `{expected}`:\n{tsv}"
        );
    }
    assert!(log.contains("98667 SNPs shared across data sets"));

    let cached_panel = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{LAVA_TUTORIAL_REF_PANEL}@"))
        });
    assert!(cached_panel, "LAVA tutorial panel should be cached");
}

#[tokio::test]
#[ignore = "requires the official gsa-mixer fixtures, k3s PVCs, kubeconfig, and the local MiXeR image"]
async fn real_official_mixer_fit1_and_fit2_run_in_k3s_and_match_baselines() {
    let source = std::env::var_os("AUTONOMICS_MIXER_IT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../containers/mixer/fixtures/mixer-test-data")
        });
    let scratch = tempfile::tempdir().unwrap();
    let panel_staging = scratch.path().join("panel-staging");
    let panel_package = scratch.path().join("panel-package");
    for directory in ["stage_flat", "ld_mixer", "snps"] {
        std::fs::create_dir_all(panel_staging.join(directory)).unwrap();
    }
    for chromosome in [21, 22] {
        std::fs::copy(
            source.join(format!("g1000_eur_hm3_chr{chromosome}.bim")),
            panel_staging.join(format!("stage_flat/chr{chromosome}.bim")),
        )
        .unwrap();
        std::fs::copy(
            source.join(format!("g1000_eur_hm3_chr{chromosome}.ld")),
            panel_staging.join(format!("ld_mixer/1000G.EUR.chr{chromosome}")),
        )
        .unwrap();
    }
    std::fs::copy(
        source.join("g1000_eur_hm3_chr@.snps"),
        panel_staging.join("snps/g1000_eur_chr@.snps"),
    )
    .unwrap();
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("population".to_string(), "EUR".to_string());
    metadata.insert("genome_build".to_string(), "GRCh37".to_string());
    metadata.insert(
        "description".to_string(),
        "official gsa-mixer chr21-22 migration fixture".to_string(),
    );
    let built = data_catalog::package::build_package(
        &panel_staging,
        &panel_package,
        data_catalog::package::BuildOptions {
            id: Some(MIXER_G1000_EUR_PANEL.to_string()),
            version: Some("v2.2.1-fixture".to_string()),
            kind: Some("mixer_reference".to_string()),
            metadata,
            payload: [
                ("bim_template".to_string(), "stage_flat/chr@.bim".into()),
                ("ld_template".to_string(), "ld_mixer/1000G.EUR.chr@".into()),
                (
                    "extract_template".to_string(),
                    "snps/g1000_eur_chr@.snps".into(),
                ),
            ]
            .into_iter()
            .collect(),
            force: false,
        },
    )
    .unwrap();

    let panel_runtime = scratch.path().join("panel-runtime");
    for directory in ["stage_flat", "ld_mixer", "snps"] {
        std::fs::create_dir_all(panel_runtime.join(directory)).unwrap();
    }
    std::fs::copy(
        panel_package.join("manifest.json"),
        panel_runtime.join("manifest.json"),
    )
    .unwrap();
    std::fs::copy(
        panel_staging.join("stage_flat/chr21.bim"),
        panel_runtime.join("stage_flat/chr21.bim"),
    )
    .unwrap();
    std::fs::copy(
        panel_staging.join("stage_flat/chr22.bim"),
        panel_runtime.join("stage_flat/chr22.bim"),
    )
    .unwrap();
    std::fs::copy(
        panel_staging.join("ld_mixer/1000G.EUR.chr21"),
        panel_runtime.join("ld_mixer/1000G.EUR.chr21"),
    )
    .unwrap();
    std::fs::copy(
        panel_staging.join("ld_mixer/1000G.EUR.chr22"),
        panel_runtime.join("ld_mixer/1000G.EUR.chr22"),
    )
    .unwrap();
    std::fs::copy(
        panel_staging.join("snps/g1000_eur_chr@.snps"),
        panel_runtime.join("snps/g1000_eur_chr@.snps"),
    )
    .unwrap();

    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "mixer-test-local".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![
            MountDefinition {
                path: "/".into(),
                backend: "mixer-test-local".into(),
                source: scratch.path().to_string_lossy().into_owned(),
                read_only: false,
            },
            MountDefinition {
                path: "/catalog/mixer_g1000_eur".into(),
                backend: "mixer-test-local".into(),
                source: panel_runtime.to_string_lossy().into_owned(),
                read_only: true,
            },
        ],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(OpendalFileStorage::with_mounts(
        scratch.path(),
        mounted.clone(),
    ));
    let session = SessionContext::new();
    session.runtime_env().register_object_store(
        ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
        mounted.clone(),
    );
    let ctx = NodeCtx::new(session.runtime_env(), Some(storage));

    let mut panel = dag_core::DataBundle::new(
        MIXER_G1000_EUR_PANEL,
        "MiXeR GRCh37 EUR migration fixture",
        "/catalog/mixer_g1000_eur",
    );
    panel.source = Some("/catalog/mixer_g1000_eur".into());
    panel.digest = built.manifest.digest.clone();
    let registry_ctx = ctx.clone().with_data_bundle_catalog(Arc::new(
        dag_core::DataBundleCatalog::from_bundles([panel]).unwrap(),
    ));

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(MixerFit1ContainerNodeFactory::new(
        Arc::clone(&runtime),
        Arc::clone(&panel_cache),
    )));
    registry.register(Box::new(MixerFit2ContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let mixer = registry
        .build_node(
            MIXER_FIT1_CONTAINER_KIND,
            serde_json::json!({
                "chr2use": "21-22",
                "seed": 123,
                "diffevo_fast_repeats": 2,
                "kmax_pdf": 10,
                "downsample_factor": 50,
                "threads": 1,
                "fast_run": true
            }),
        )
        .unwrap();

    let sumstats = source.join("trait1.sumstats.gz");
    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "trait1".into(),
        Box::new(FileReferenceNode::new(
            sumstats.to_string_lossy().into_owned(),
            Some("sumstats_gz".into()),
        )),
    )
    .unwrap();
    dag.add_node("mixer_fit1".into(), mixer).unwrap();
    dag.add_edge("trait1", "mixer_fit1", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    for id in ["trait1", "mixer_fit1"] {
        assert_eq!(
            report.statuses.get(id),
            Some(&dag_core::dag::RuntimeStatus::Success),
            "node `{id}` did not succeed"
        );
    }

    let outputs = dag.output("mixer_fit1").unwrap();
    let result_file = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log_file = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(result_file.path.ends_with("/mixer_fit1.json"));
    assert!(log_file.path.ends_with("/mixer_fit1.log"));
    assert!(
        result_file
            .path
            .starts_with("vfs:///artifacts/mixer_container/")
    );

    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    async fn published_text(
        storage: &OpendalFileStorage,
        output: &dag_core::value::FileRef,
    ) -> String {
        let path = output
            .path
            .strip_prefix("vfs://")
            .expect("MiXeR artifact is a VFS URI");
        let bytes = storage
            .resolve(path)
            .read(&storage.resolve_path(path))
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes.to_vec()).into_owned()
    }
    let result = published_text(storage, &result_file).await;
    let log = published_text(storage, &log_file).await;
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();
    let baseline_text = std::fs::read_to_string(source.join("trait1.fit1.json")).unwrap();
    let baseline: serde_json::Value = serde_json::from_str(&baseline_text).unwrap();
    for field in ["params", "inft_params"] {
        assert_eq!(
            result[field], baseline[field],
            "MiXeR {field} baseline changed"
        );
    }
    assert_eq!(result["options"]["num_snp"], 34_958.0);
    assert_eq!(result["options"]["num_tag"], 11_200.0);
    assert!(log.contains("MiXeR v2.2.1"));

    let mixer_fit2 = registry
        .build_node(
            MIXER_FIT2_CONTAINER_KIND,
            serde_json::json!({
                "chr2use": "21-22",
                "seed": 123,
                "diffevo_fast_repeats": 2,
                "kmax_pdf": 10,
                "downsample_factor": 50,
                "threads": 1,
                "fast_run": false
            }),
        )
        .unwrap();
    let fit2_inputs = [
        "trait1.sumstats.gz",
        "trait2.sumstats.gz",
        "trait1_sanity.json",
        "trait2_fg.json",
    ];
    let mut fit2_dag = dag_core::dag::DAG::default();
    fit2_dag.add_node("mixer_fit2".into(), mixer_fit2).unwrap();
    for (port, name) in fit2_inputs.iter().enumerate() {
        let port = port as u8;
        fit2_dag
            .add_node(
                format!("fit2-input-{port}"),
                Box::new(FileReferenceNode::new(
                    source.join(name).to_string_lossy().into_owned(),
                    Some("mixer_fit2_input".into()),
                )),
            )
            .unwrap();
        fit2_dag
            .add_edge(format!("fit2-input-{port}"), "mixer_fit2", 0, port)
            .unwrap();
    }
    let fit2_report = fit2_dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    for id in 0..4 {
        assert_eq!(
            fit2_report.statuses.get(&format!("fit2-input-{id}")),
            Some(&dag_core::dag::RuntimeStatus::Success)
        );
    }
    assert_eq!(
        fit2_report.statuses.get("mixer_fit2"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let fit2_outputs = fit2_dag.output("mixer_fit2").unwrap();
    let fit2_result_file = fit2_outputs.get(&0).unwrap().as_file().unwrap().clone();
    let fit2_log_file = fit2_outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(fit2_result_file.path.ends_with("/mixer_fit2.json"));
    assert!(fit2_log_file.path.ends_with("/mixer_fit2.log"));
    let fit2_result_text = published_text(storage, &fit2_result_file).await;
    let fit2_log = published_text(storage, &fit2_log_file).await;
    let fit2_result: serde_json::Value = serde_json::from_str(&fit2_result_text).unwrap();
    let fit2_baseline_text = std::fs::read_to_string(source.join("fit2.json")).unwrap();
    let fit2_baseline: serde_json::Value = serde_json::from_str(&fit2_baseline_text).unwrap();
    assert_eq!(
        fit2_result["params"], fit2_baseline["params"],
        "MiXeR fit2 params baseline changed"
    );
    assert_eq!(fit2_result["options"]["num_snp"], 34_958.0);
    assert_eq!(fit2_result["options"]["num_tag"], 11_200.0);
    assert!(fit2_log.contains("MiXeR v2.2.1"));

    let cache_key = format!(
        "{MIXER_G1000_EUR_PANEL}@{}",
        built.manifest.digest.as_deref().unwrap()
    );
    assert!(
        k3s_config.panel_cache_root.join(cache_key).is_dir(),
        "MiXeR fixture panel should remain in PanelCache"
    );
}

#[tokio::test]
#[ignore = "requires the published MiXeR GRCh37 EUR catalog panel, k3s PVCs, kubeconfig, and the local official image"]
async fn real_published_mixer_g1000_eur_panel_runs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    assert!(fixture.bundles.get(MIXER_G1000_EUR_PANEL).is_some());
    let sumstats_path = std::env::var_os("AUTONOMICS_MIXER_IT_SUMSTATS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../containers/mixer/fixtures/mixer-test-data/trait1.sumstats.gz")
        });

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(MixerFit1ContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let mixer = registry
        .build_node(
            MIXER_FIT1_CONTAINER_KIND,
            serde_json::json!({
                "chr2use": "21-22",
                "diffevo_fast_repeats": 1,
                "threads": 1
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "trait1".into(),
        Box::new(FileReferenceNode::new(
            sumstats_path.to_string_lossy().into_owned(),
            Some("sumstats_gz".into()),
        )),
    )
    .unwrap();
    dag.add_node("mixer_fit1".into(), mixer).unwrap();
    dag.add_edge("trait1", "mixer_fit1", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("trait1"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("mixer_fit1"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("mixer_fit1").unwrap();
    let result_file = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log_file = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(result_file.path.ends_with("/mixer_fit1.json"));
    assert!(log_file.path.ends_with("/mixer_fit1.log"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    async fn mixer_published_text(
        storage: &OpendalFileStorage,
        output: &dag_core::value::FileRef,
    ) -> String {
        let path = output
            .path
            .strip_prefix("vfs://")
            .expect("MiXeR artifact is a VFS URI");
        let bytes = storage
            .resolve(path)
            .read(&storage.resolve_path(path))
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes.to_vec()).into_owned()
    }
    let result_text = mixer_published_text(storage, &result_file).await;
    let log = mixer_published_text(storage, &log_file).await;
    let result: serde_json::Value = serde_json::from_str(&result_text).unwrap();
    assert_eq!(result["options"]["num_snp"], 271_783.0);
    assert!(log.contains("MiXeR v2.2.1"));

    let cached_panel = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{MIXER_G1000_EUR_PANEL}@"))
        });
    assert!(
        cached_panel,
        "published MiXeR GRCh37 EUR panel should remain in PanelCache"
    );
}

#[tokio::test]
#[ignore = "requires the cached published MiXeR GRCh37 EUR panel, k3s PVCs, kubeconfig, and the local official image"]
async fn real_published_mixer_fit2_stages_four_inputs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    let fixture_data = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/mixer/fixtures/mixer-test-data");
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root,
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(MixerFit2ContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let mixer = registry
        .build_node(
            MIXER_FIT2_CONTAINER_KIND,
            serde_json::json!({
                "chr2use": "21-22",
                "diffevo_fast_repeats": 2,
                "threads": 1,
                "fast_run": false
            }),
        )
        .unwrap();

    let input_files = [
        "trait1.sumstats.gz",
        "trait2.sumstats.gz",
        "trait1_sanity.json",
        "trait2_fg.json",
    ];
    let mut dag = dag_core::dag::DAG::default();
    dag.add_node("mixer_fit2".into(), mixer).unwrap();
    for (port, name) in input_files.iter().enumerate() {
        dag.add_node(
            format!("input-{port}"),
            Box::new(FileReferenceNode::new(
                fixture_data.join(name).to_string_lossy().into_owned(),
                Some("mixer_fit2_input".into()),
            )),
        )
        .unwrap();
        let port = port as u8;
        dag.add_edge(format!("input-{port}"), "mixer_fit2", 0, port)
            .unwrap();
    }
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    for id in 0..4 {
        assert_eq!(
            report.statuses.get(&format!("input-{id}")),
            Some(&dag_core::dag::RuntimeStatus::Success)
        );
    }
    assert_eq!(
        report.statuses.get("mixer_fit2"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("mixer_fit2").unwrap();
    let result_file = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log_file = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(result_file.path.ends_with("/mixer_fit2.json"));
    assert!(log_file.path.ends_with("/mixer_fit2.log"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    async fn mixer_fit2_text(
        storage: &OpendalFileStorage,
        output: &dag_core::value::FileRef,
    ) -> String {
        let path = output
            .path
            .strip_prefix("vfs://")
            .expect("MiXeR artifact is a VFS URI");
        let bytes = storage
            .resolve(path)
            .read(&storage.resolve_path(path))
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes.to_vec()).into_owned()
    }
    let result: serde_json::Value =
        serde_json::from_str(&mixer_fit2_text(storage, &result_file).await).unwrap();
    let log = mixer_fit2_text(storage, &log_file).await;
    assert_eq!(result["analysis"], "bivariate");
    assert_eq!(result["options"]["num_snp"], 271_783.0);
    assert!(log.contains("MiXeR v2.2.1"));
    assert!(log.contains("--trait2-params-file"));
}

#[tokio::test]
#[ignore = "requires Garage catalog panels, k3s PVCs, kubeconfig, the local LDSC image, and two test sumstats"]
async fn real_catalog_backed_original_ldsc_rg_runs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    let asthma_path = std::env::var_os("AUTONOMICS_LDSC_IT_SUMSTATS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new("/mnt/data/ldsc_data/sumstats_107/GBMI.Asthma.sumstats.gz").to_path_buf()
        });
    let bmi_path = std::env::var_os("AUTONOMICS_LDSC_RG_IT_SUMSTATS2")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new("/mnt/data/ldsc_data/sumstats_107/PASS.BMI.Yengo2018.sumstats.gz")
                .to_path_buf()
        });

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(LdscRgContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let ldsc = registry
        .build_node(LDSC_RG_CONTAINER_KIND, serde_json::json!({}))
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "trait1".into(),
        Box::new(FileReferenceNode::new(
            asthma_path.to_string_lossy().into_owned(),
            Some("sumstats_gz".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "trait2".into(),
        Box::new(FileReferenceNode::new(
            bmi_path.to_string_lossy().into_owned(),
            Some("sumstats_gz".into()),
        )),
    )
    .unwrap();
    dag.add_node("ldsc_rg".into(), ldsc).unwrap();
    dag.add_edge("trait1", "ldsc_rg", 0, 0).unwrap();
    dag.add_edge("trait2", "ldsc_rg", 0, 1).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("trait1"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("trait2"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("ldsc_rg"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let output = dag
        .output("ldsc_rg")
        .unwrap()
        .get(&0)
        .unwrap()
        .as_file()
        .unwrap()
        .clone();
    assert!(
        output
            .path
            .starts_with("vfs:///artifacts/ldsc_rg_container/")
    );
    assert!(output.path.ends_with("/ldsc_rg.log"));
    let virtual_path = output
        .path
        .strip_prefix("vfs://")
        .expect("LDSC rg artifact is a VFS URI");
    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let published = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    let log = String::from_utf8_lossy(&published.to_vec()).into_owned();
    for expected in [
        "Total Observed scale h2: 0.0196 (0.0017)",
        "Total Observed scale h2: 0.1921 (0.008)",
        "Total Observed scale gencov: 0.0173 (0.0013)",
        "Genetic Correlation: 0.2826 (0.022)",
        "Z-score: 12.8269",
    ] {
        assert!(
            log.contains(expected),
            "LDSC rg baseline is missing `{expected}`:\n{log}"
        );
    }
}

#[tokio::test]
#[ignore = "requires k3s PVCs, kubeconfig, and the local official MRPRESSO image"]
async fn real_official_mrpresso_runs_in_k3s() {
    let scratch = tempfile::tempdir().unwrap();
    let csv_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../bio_crates/mrpresso/tests/summary_stats_headers.csv");
    let csv = std::fs::read_to_string(csv_path).unwrap();
    let tsv = csv.replace(',', "\t");
    let input_path = scratch.path().join("mrpresso-summary.tsv");
    std::fs::write(&input_path, tsv).unwrap();

    let (_mounted, ctx) = workspace_vfs(scratch.path());
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let mut registry = NodeRegistry::new(ctx.clone());
    registry.register(Box::new(MrpressoContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let mrpresso = registry
        .build_node(
            MRPRESSO_CONTAINER_KIND,
            serde_json::json!({
                "beta_outcome": "Y_effect",
                "sd_outcome": "Y_se",
                "beta_exposure": ["E1_effect"],
                "sd_exposure": ["E1_se"],
                "outlier_test": true,
                "distortion_test": true
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "summary_stats".into(),
        Box::new(FileReferenceNode::new(
            input_path.to_string_lossy().into_owned(),
            Some("tsv".into()),
        )),
    )
    .unwrap();
    dag.add_node("mrpresso".into(), mrpresso).unwrap();
    dag.add_edge("summary_stats", "mrpresso", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("summary_stats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("mrpresso"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("mrpresso").unwrap();
    let rds = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(rds.path.ends_with("/mrpresso.RDS"));
    assert!(log.path.ends_with("/mrpresso.log"));
    assert!(log.path.starts_with("vfs:///artifacts/mrpresso_container/"));

    let virtual_path = log
        .path
        .strip_prefix("vfs://")
        .expect("MR-PRESSO log is a VFS URI");
    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    let published = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    let log = String::from_utf8_lossy(&published.to_vec()).into_owned();
    for expected in [
        "RSSobs",
        "133.0666",
        "<0.001",
        "0.5390120",
        "0.5014829",
        "7.483624",
    ] {
        assert!(
            log.contains(expected),
            "MR-PRESSO official baseline is missing `{expected}`:\n{log}"
        );
    }
}

#[tokio::test]
#[ignore = "requires k3s PVCs, kubeconfig, and the local official MVMR image"]
async fn real_official_mvmr_runs_in_k3s() {
    let scratch = tempfile::tempdir().unwrap();
    let csv_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../bio_crates/mvmr/tests/rawdat_mvmr.csv");
    let csv = std::fs::read_to_string(csv_path).unwrap();
    let input_path = scratch.path().join("rawdat_mvmr.tsv");
    std::fs::write(&input_path, csv.replace(',', "\t")).unwrap();

    let (_mounted, ctx) = workspace_vfs(scratch.path());
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let mut registry = NodeRegistry::new(ctx.clone());
    registry.register(Box::new(MvmrContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let mvmr = registry
        .build_node(
            MVMR_CONTAINER_KIND,
            serde_json::json!({
                "beta_yg": "SBP_beta",
                "sebeta_yg": "SBP_se",
                "beta_xg": ["LDL_beta", "HDL_beta"],
                "sebeta_xg": ["LDL_se", "HDL_se"],
                "label_column": "SNP",
                "strength": true,
                "strhet": true,
                "pleiotropy": true,
                "qhet": false
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "instruments".into(),
        Box::new(FileReferenceNode::new(
            input_path.to_string_lossy().into_owned(),
            Some("tsv".into()),
        )),
    )
    .unwrap();
    dag.add_node("mvmr".into(), mvmr).unwrap();
    dag.add_edge("instruments", "mvmr", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("instruments"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("mvmr"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("mvmr").unwrap();
    let rds = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(rds.path.ends_with("/mvmr.RDS"));
    assert!(log.path.ends_with("/mvmr.log"));
    assert!(log.path.starts_with("vfs:///artifacts/mvmr_container/"));

    let virtual_path = log
        .path
        .strip_prefix("vfs://")
        .expect("MVMR log is a VFS URI");
    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    let published = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    let log = String::from_utf8_lossy(&published.to_vec()).into_owned();
    for expected in [
        "-0.031003996",
        "0.006039167",
        "67.17187",
        "79.50517",
        "695.5924",
        "7.338e-74",
    ] {
        assert!(
            log.contains(expected),
            "MVMR official baseline is missing `{expected}`:\n{log}"
        );
    }
}

#[tokio::test]
#[ignore = "requires the official LAVA tutorial panel, k3s PVCs, kubeconfig, and the local official LAVA image"]
async fn real_catalog_backed_official_lava_bivar_runs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    assert!(fixture.bundles.get(LAVA_TUTORIAL_REF_PANEL).is_some());
    let scratch = tempfile::tempdir().unwrap();
    let source = std::env::var_os("AUTONOMICS_LAVA_IT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/lava/LAVA")
        });
    let fixture_data = source.join("vignettes/data");
    let bundle_root = scratch.path().join("bundle");
    let bundle_data = bundle_root.join("vignettes/data");
    std::fs::create_dir_all(&bundle_data).unwrap();
    for name in [
        "input.info.txt",
        "sample.overlap.txt",
        "test.loci",
        "depression.sumstats.txt",
        "neuro.sumstats.txt",
        "bmi.sumstats.txt",
    ] {
        std::fs::copy(fixture_data.join(name), bundle_data.join(name)).unwrap();
    }
    let bundle_zip = scratch.path().join("lava-bivar-bundle.zip");
    let zip_status = Command::new("zip")
        .args(["-q", "-r"])
        .arg(&bundle_zip)
        .arg("vignettes")
        .current_dir(&bundle_root)
        .status()
        .unwrap();
    assert!(
        zip_status.success(),
        "could not create the LAVA bivar run bundle"
    );

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(LavaContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let lava = registry
        .build_node(
            LAVA_CONTAINER_KIND,
            serde_json::json!({
                "analysis": "bivar",
                "panel_id": LAVA_TUTORIAL_REF_PANEL,
                "input_info_file": "vignettes/data/input.info.txt",
                "loci_file": "vignettes/data/test.loci",
                "sample_overlap_file": "vignettes/data/sample.overlap.txt",
                "locus_index": 1,
                "phenotypes": ["depression", "neuro", "bmi"]
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "bundle".into(),
        Box::new(FileReferenceNode::new(
            bundle_zip.to_string_lossy().into_owned(),
            Some("lava_run_bundle_zip".into()),
        )),
    )
    .unwrap();
    dag.add_node("lava_bivar".into(), lava).unwrap();
    dag.add_edge("bundle", "lava_bivar", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("bundle"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("lava_bivar"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "LAVA bivar node failed: {report:#?}"
    );

    let outputs = dag.output("lava_bivar").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let tsv_text = read_published_text(storage, &tsv).await;
    let log_text = read_published_text(storage, &log).await;
    assert!(tsv_text.contains("phen1\tphen2\trho\t"));
    // bivariate correlations are deterministic; CI bounds and p-values come
    // from LAVA's adaptive Monte-Carlo integration, so we assert the leading
    // numeric columns match the official baseline.
    for anchor in [
        "depression\tneuro\t-0.256507",
        "depression\tbmi\t-0.419247",
        "neuro\tbmi\t-0.463308",
    ] {
        assert!(
            tsv_text.contains(anchor),
            "official LAVA bivar baseline is missing `{anchor}`:\n{tsv_text}"
        );
    }
    // r2 columns are written by R's write.table and may drop a trailing zero
    // (e.g. 0.1757680 becomes 0.175768), so we check the canonical short form.
    for anchor in ["0.0657958", "0.175768", "0.214654"] {
        assert!(
            tsv_text.contains(anchor),
            "official LAVA bivar baseline is missing r2 `{anchor}`:\n{tsv_text}"
        );
    }
    assert!(log_text.contains("98667 SNPs shared across data sets"));
}

#[tokio::test]
#[ignore = "requires the official LAVA tutorial panel, k3s PVCs, kubeconfig, and the local official LAVA image"]
async fn real_catalog_backed_official_lava_pcor_runs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    assert!(fixture.bundles.get(LAVA_TUTORIAL_REF_PANEL).is_some());
    let scratch = tempfile::tempdir().unwrap();
    let source = std::env::var_os("AUTONOMICS_LAVA_IT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/lava/LAVA")
        });
    let fixture_data = source.join("vignettes/data");
    let bundle_root = scratch.path().join("bundle");
    let bundle_data = bundle_root.join("vignettes/data");
    std::fs::create_dir_all(&bundle_data).unwrap();
    for name in [
        "input.info.txt",
        "sample.overlap.txt",
        "test.loci",
        "depression.sumstats.txt",
        "neuro.sumstats.txt",
        "bmi.sumstats.txt",
    ] {
        std::fs::copy(fixture_data.join(name), bundle_data.join(name)).unwrap();
    }
    let bundle_zip = scratch.path().join("lava-pcor-bundle.zip");
    let zip_status = Command::new("zip")
        .args(["-q", "-r"])
        .arg(&bundle_zip)
        .arg("vignettes")
        .current_dir(&bundle_root)
        .status()
        .unwrap();
    assert!(
        zip_status.success(),
        "could not create the LAVA pcor run bundle"
    );

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(LavaContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let lava = registry
        .build_node(
            LAVA_CONTAINER_KIND,
            serde_json::json!({
                "analysis": "pcor",
                "panel_id": LAVA_TUTORIAL_REF_PANEL,
                "input_info_file": "vignettes/data/input.info.txt",
                "loci_file": "vignettes/data/test.loci",
                "sample_overlap_file": "vignettes/data/sample.overlap.txt",
                "locus_index": 1,
                "phenotypes": ["depression", "neuro", "bmi"],
                "target": ["depression", "neuro"]
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "bundle".into(),
        Box::new(FileReferenceNode::new(
            bundle_zip.to_string_lossy().into_owned(),
            Some("lava_run_bundle_zip".into()),
        )),
    )
    .unwrap();
    dag.add_node("lava_pcor".into(), lava).unwrap();
    dag.add_edge("bundle", "lava_pcor", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("bundle"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("lava_pcor"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "LAVA pcor node failed: {report:#?}"
    );

    let outputs = dag.output("lava_pcor").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let tsv_text = read_published_text(storage, &tsv).await;
    assert!(tsv_text.contains("phen1\tphen2\tz\tr2.phen1_z\tr2.phen2_z\tpcor"));
    // pcor, r2.phen_z, and the conditioning z are deterministic; CI bounds and
    // p-value come from LAVA's adaptive Monte-Carlo integration.
    assert!(tsv_text.contains("depression\tneuro\tbmi"));
    for anchor in ["0.175768", "0.214654", "-0.560245"] {
        assert!(
            tsv_text.contains(anchor),
            "official LAVA pcor baseline is missing `{anchor}`:\n{tsv_text}"
        );
    }
}

#[tokio::test]
#[ignore = "requires the official LAVA tutorial panel, k3s PVCs, kubeconfig, and the local official LAVA image"]
async fn real_catalog_backed_official_lava_multireg_runs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    assert!(fixture.bundles.get(LAVA_TUTORIAL_REF_PANEL).is_some());
    let scratch = tempfile::tempdir().unwrap();
    let source = std::env::var_os("AUTONOMICS_LAVA_IT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/lava/LAVA")
        });
    let fixture_data = source.join("vignettes/data");
    let bundle_root = scratch.path().join("bundle");
    let bundle_data = bundle_root.join("vignettes/data");
    std::fs::create_dir_all(&bundle_data).unwrap();
    for name in [
        "input.info.txt",
        "sample.overlap.txt",
        "test.loci",
        "depression.sumstats.txt",
        "neuro.sumstats.txt",
        "bmi.sumstats.txt",
    ] {
        std::fs::copy(fixture_data.join(name), bundle_data.join(name)).unwrap();
    }
    let bundle_zip = scratch.path().join("lava-multireg-bundle.zip");
    let zip_status = Command::new("zip")
        .args(["-q", "-r"])
        .arg(&bundle_zip)
        .arg("vignettes")
        .current_dir(&bundle_root)
        .status()
        .unwrap();
    assert!(
        zip_status.success(),
        "could not create the LAVA multireg run bundle"
    );

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(LavaContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let lava = registry
        .build_node(
            LAVA_CONTAINER_KIND,
            serde_json::json!({
                "analysis": "multireg",
                "panel_id": LAVA_TUTORIAL_REF_PANEL,
                "input_info_file": "vignettes/data/input.info.txt",
                "loci_file": "vignettes/data/test.loci",
                "sample_overlap_file": "vignettes/data/sample.overlap.txt",
                "locus_index": 1,
                "phenotypes": ["depression", "neuro", "bmi"],
                "target": ["bmi"],
                "only_full_model": true
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "bundle".into(),
        Box::new(FileReferenceNode::new(
            bundle_zip.to_string_lossy().into_owned(),
            Some("lava_run_bundle_zip".into()),
        )),
    )
    .unwrap();
    dag.add_node("lava_multireg".into(), lava).unwrap();
    dag.add_edge("bundle", "lava_multireg", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("bundle"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("lava_multireg"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "LAVA multireg node failed: {report:#?}"
    );

    let outputs = dag.output("lava_multireg").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let tsv_text = read_published_text(storage, &tsv).await;
    assert!(tsv_text.contains("predictors\toutcome\tgamma\t"));
    assert!(tsv_text.contains("depression\tbmi\t-0.575986"));
    assert!(tsv_text.contains("neuro\tbmi\t-0.611052"));
}

async fn read_published_text(
    storage: &OpendalFileStorage,
    output: &dag_core::value::FileRef,
) -> String {
    let path = output
        .path
        .strip_prefix("vfs://")
        .expect("LAVA artifact is a VFS URI");
    let bytes = storage
        .resolve(path)
        .read(&storage.resolve_path(path))
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes.to_vec()).into_owned()
}

#[tokio::test]
#[ignore = "requires k3s PVCs, kubeconfig, and the local official coloc image"]
async fn real_official_coloc_abf_runs_in_k3s() {
    let scratch = tempfile::tempdir().unwrap();
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../bio_crates/coloc/tests/coloc_abf_fixture.tsv");
    let tsv = std::fs::read_to_string(&fixture_path).unwrap();
    let input_path = scratch.path().join("coloc-abf-input.tsv");
    std::fs::write(&input_path, tsv).unwrap();

    let (_mounted, ctx) = workspace_vfs(scratch.path());
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let mut registry = NodeRegistry::new(ctx.clone());
    registry.register(Box::new(ColocAbfContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let coloc = registry
        .build_node(
            COLOC_ABF_CONTAINER_KIND,
            serde_json::json!({
                "dataset1": {
                    "type": "quant",
                    "snp": "snp",
                    "beta": "beta1",
                    "varbeta": "varbeta1",
                    "maf": "maf",
                    "n": 1000.0,
                    "sd_y": 1.0
                },
                "dataset2": {
                    "type": "quant",
                    "snp": "snp",
                    "beta": "beta2",
                    "varbeta": "varbeta2",
                    "maf": "maf",
                    "n": 1000.0,
                    "sd_y": 1.0
                }
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "sumstats".into(),
        Box::new(FileReferenceNode::new(
            input_path.to_string_lossy().into_owned(),
            Some("tsv".into()),
        )),
    )
    .unwrap();
    dag.add_node("coloc_abf".into(), coloc).unwrap();
    dag.add_edge("sumstats", "coloc_abf", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("sumstats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("coloc_abf"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("coloc_abf").unwrap();
    let rds = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(rds.path.ends_with("/coloc_abf.RDS"));
    assert!(log.path.ends_with("/coloc_abf.log"));
    assert!(
        log.path
            .starts_with("vfs:///artifacts/coloc_abf_container/")
    );

    let virtual_path = log
        .path
        .strip_prefix("vfs://")
        .expect("coloc.abf log is a VFS URI");
    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    let published = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    let log_text = String::from_utf8_lossy(&published.to_vec()).into_owned();
    // Structural markers: the official coloc::coloc.abf() printed output must
    // contain the summary PPs and the per-SNP PP.H4 marker.
    for expected in ["coloc.abf", "PP.H4.abf", "nsnps", "SNP.PP.H4"] {
        assert!(
            log_text.contains(expected),
            "coloc.abf official baseline is missing `{expected}`:\n{log_text}"
        );
    }
}

#[tokio::test]
#[ignore = "requires k3s PVCs, kubeconfig, and the local official HyPrColoc image"]
async fn real_official_hyprcoloc_runs_in_k3s() {
    let scratch = tempfile::tempdir().unwrap();
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/hyprcoloc/fixtures/test-summary-stats.tsv");
    let tsv = std::fs::read_to_string(&fixture_path).unwrap();
    let input_path = scratch.path().join("hyprcoloc-input.tsv");
    std::fs::write(&input_path, tsv).unwrap();

    let (_mounted, ctx) = workspace_vfs(scratch.path());
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let mut registry = NodeRegistry::new(ctx.clone());
    registry.register(Box::new(HyPrColocContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let hyprcoloc = registry
        .build_node(
            HYPRCOLOC_CONTAINER_KIND,
            serde_json::json!({
                "snp_column": "snp",
                "traits": [
                    {"name": "T1", "beta": "beta_T1", "se": "se_T1"},
                    {"name": "T2", "beta": "beta_T2", "se": "se_T2"},
                    {"name": "T3", "beta": "beta_T3", "se": "se_T3"},
                    {"name": "T4", "beta": "beta_T4", "se": "se_T4"},
                    {"name": "T5", "beta": "beta_T5", "se": "se_T5"},
                    {"name": "T6", "beta": "beta_T6", "se": "se_T6"},
                    {"name": "T7", "beta": "beta_T7", "se": "se_T7"},
                    {"name": "T8", "beta": "beta_T8", "se": "se_T8"},
                    {"name": "T9", "beta": "beta_T9", "se": "se_T9"},
                    {"name": "T10", "beta": "beta_T10", "se": "se_T10"}
                ]
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "sumstats".into(),
        Box::new(FileReferenceNode::new(
            input_path.to_string_lossy().into_owned(),
            Some("tsv".into()),
        )),
    )
    .unwrap();
    dag.add_node("hyprcoloc".into(), hyprcoloc).unwrap();
    dag.add_edge("sumstats", "hyprcoloc", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("sumstats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("hyprcoloc"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("hyprcoloc").unwrap();
    let table = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let rds = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(table.path.ends_with("/hyprcoloc_results.tsv"));
    assert!(rds.path.ends_with("/hyprcoloc.RDS"));
    assert!(log.path.ends_with("/hyprcoloc.log"));
    assert!(
        table
            .path
            .starts_with("vfs:///artifacts/hyprcoloc_container/")
    );

    let virtual_path = table
        .path
        .strip_prefix("vfs://")
        .expect("HyPrColoc result is a VFS URI");
    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    let published = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    let result_text = String::from_utf8_lossy(&published.to_vec()).into_owned();
    for expected in ["T1, T2, T3, T4, T5", "rs11591147", "T6, T7, T8", "T9, T10"] {
        assert!(
            result_text.contains(expected),
            "official HyPrColoc fixture is missing `{expected}`:\n{result_text}"
        );
    }
}

#[tokio::test]
#[ignore = "requires the official PLINK2 image, the 1000G EUR Phase3 PLINK reference panel package, k3s PVCs, and kubeconfig"]
async fn real_catalog_backed_official_plink2_clump_runs_in_k3s() {
    use dag_core::dag::DagNode;

    let fixture = catalog_test_fixture().await;
    assert!(
        fixture.bundles.get(PLINK2_REF_BINARY_PANEL).is_some(),
        "plink.ref.1000g_eur.binary must be published in the catalog before this test"
    );

    let sumstats_path = std::env::var_os("AUTONOMICS_PLINK2_IT_SUMSTATS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // Fall back to the small PLINK2 chr22 fixture committed with the
            // image workflow.
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../containers/plink2/fixtures/chr22.sumstats.tsv")
                .to_path_buf()
        });
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(Plink2ClumpContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let plink2 = registry
        .build_node(
            PLINK2_CLUMP_CONTAINER_KIND,
            serde_json::json!({
                "chr": 22, // Single-chromosome run for the smoke baseline.
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "sumstats".into(),
        Box::new(FileReferenceNode::new(
            sumstats_path.to_string_lossy().into_owned(),
            Some("tsv".into()),
        )),
    )
    .unwrap();
    dag.add_node("plink2_clump".into(), plink2).unwrap();
    dag.add_edge("sumstats", "plink2_clump", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("sumstats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("plink2_clump"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("plink2_clump").unwrap();
    let log = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let clumps = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let chroms = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(log.path.ends_with("/plink2_clump.log"));
    assert!(clumps.path.ends_with("/plink2_clump.clumps"));
    assert!(chroms.path.ends_with("/plink2_clump.chromosomes.tsv"));
    assert!(
        log.path
            .starts_with("vfs:///artifacts/plink2_clump_container/")
    );

    // The chromosomes manifest must record exactly one chromosome when chr=22.
    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let chroms_vpath = chroms
        .path
        .strip_prefix("vfs://")
        .expect("chromosomes manifest is a VFS URI");
    let bytes = storage
        .resolve(chroms_vpath)
        .read(&storage.resolve_path(chroms_vpath))
        .await
        .unwrap();
    let chroms_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    assert!(
        chroms_text.contains("chr\t22"),
        "chromosomes manifest should record chr 22; got:\n{chroms_text}"
    );

    // The PLINK2 log must report successful completion against the official image.
    let log_vpath = log
        .path
        .strip_prefix("vfs://")
        .expect("PLINK2 log is a VFS URI");
    let bytes = storage
        .resolve(log_vpath)
        .read(&storage.resolve_path(log_vpath))
        .await
        .unwrap();
    let log_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    assert!(
        log_text.contains("plink2 clump completed across chromosome 22"),
        "PLINK2 clump log is missing the success banner; got:\n{log_text}"
    );

    // The committed chr22 fixture must produce a deterministic file-level
    // baseline: two index variants in the 1000G EUR chr22 panel.
    let clumps_vpath = clumps
        .path
        .strip_prefix("vfs://")
        .expect("PLINK2 clumps is a VFS URI");
    let bytes = storage
        .resolve(clumps_vpath)
        .read(&storage.resolve_path(clumps_vpath))
        .await
        .unwrap();
    let clumps_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    assert!(
        clumps_text.contains("rs7286962"),
        "PLINK2 clumps output is missing the chr22 baseline index variant; got:\n{clumps_text}"
    );
    assert!(
        clumps_text.contains("rs587743102"),
        "PLINK2 clumps output is missing the second chr22 baseline index variant; got:\n{clumps_text}"
    );

    // The 1000G EUR PLINK binary panel must have been materialized into the
    // shared PanelCache, proving the catalog pipeline is wired end-to-end.
    let cached_panel = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{PLINK2_REF_BINARY_PANEL}@"))
        });
    assert!(cached_panel, "PLINK2 reference panel should be cached");
}

#[tokio::test]
#[ignore = "requires the official susieR 0.16.6 image, the mixer.g1000_eur signed-LD panel, k3s PVCs, and kubeconfig"]
async fn real_catalog_backed_official_susie_rss_runs_in_k3s() {
    use dag_core::dag::DagNode;

    let fixture = catalog_test_fixture().await;
    assert!(
        fixture.bundles.get(SUSIE_REF_PANEL).is_some(),
        "mixer.g1000_eur must be published in the catalog before this test"
    );

    let sumstats_path = std::env::var_os("AUTONOMICS_SUSIE_IT_SUMSTATS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../containers/susie/fixtures/chr21.sumstats.tsv")
                .to_path_buf()
        });
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(SusieRssContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let susie = registry
        .build_node(
            SUSIE_RSS_CONTAINER_KIND,
            serde_json::json!({
                "l": 5,
                "n": 503,
                "max_iter": 20,
                "r2_min": 0.01,
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "sumstats".into(),
        Box::new(FileReferenceNode::new(
            sumstats_path.to_string_lossy().into_owned(),
            Some("tsv".into()),
        )),
    )
    .unwrap();
    dag.add_node("susie_rss".into(), susie).unwrap();
    dag.add_edge("sumstats", "susie_rss", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("sumstats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("susie_rss"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("susie_rss").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let rds = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(tsv.path.ends_with("/susie_rss.tsv"));
    assert!(rds.path.ends_with("/susie_rss.RDS"));
    assert!(log.path.ends_with("/susie_rss.log"));
    assert!(
        log.path
            .starts_with("vfs:///artifacts/susie_rss_container/")
    );

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let tsv_vpath = tsv
        .path
        .strip_prefix("vfs://")
        .expect("susie TSV is a VFS URI");
    let bytes = storage
        .resolve(tsv_vpath)
        .read(&storage.resolve_path(tsv_vpath))
        .await
        .unwrap();
    let tsv_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    assert!(
        tsv_text.starts_with("snp\tpip\tcs\talpha\tmu\tmu2\tlbf"),
        "susie TSV is missing the expected header; got:\n{tsv_text}"
    );
    assert!(
        tsv_text.lines().count() >= 51,
        "susie TSV should include 50 variants plus a header; got {} lines",
        tsv_text.lines().count()
    );
    for expected in [
        "21:9413839:C:T\t0.999999978798887\t1",
        "21:9413840:A:C\t0.999982089651926\t2",
        "21:9413584:A:G\t0.999997880009654\t3",
        "21:9420476:A:G\t0.999999999999999\t4",
    ] {
        assert!(
            tsv_text.contains(expected),
            "susie TSV is missing the official chr21 baseline row `{expected}`; got:\n{tsv_text}"
        );
    }

    let log_vpath = log
        .path
        .strip_prefix("vfs://")
        .expect("susie log is a VFS URI");
    let bytes = storage
        .resolve(log_vpath)
        .read(&storage.resolve_path(log_vpath))
        .await
        .unwrap();
    let log_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    assert!(
        log_text.contains("## susieR susie_rss 0.16.6"),
        "susie log is missing the official run banner; got:\n{log_text}"
    );
    assert!(
        log_text.contains("credible sets: 4"),
        "susie log is missing the chr21 credible-set baseline; got:\n{log_text}"
    );

    let cached_panel = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{SUSIE_REF_PANEL}@"))
        });
    assert!(cached_panel, "mixer signed-LD panel should be cached");
}

#[tokio::test]
#[ignore = "requires the official FUSION image, the GTEx v8/FUSION catalog panel, k3s PVCs, kubeconfig, and the local LDREF fixture"]
async fn real_catalog_backed_official_fusion_twas_runs_in_k3s() {
    use dag_core::dag::DagNode;

    let fixture = catalog_test_fixture().await;
    assert!(
        fixture.bundles.get(FUSION_GTEX_V8_PANEL).is_some(),
        "fusion.gtex_v8 must be published in the catalog before this test"
    );

    let scratch = tempfile::tempdir().unwrap();
    let sumstats_path = scratch.path().join("fusion-chr21.tsv");
    let ldref_path = std::env::var_os("AUTONOMICS_FUSION_IT_LDREF")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/mnt/data/twas_fusion/LDREF/1000G.EUR.21.bim").to_path_buf());
    let mut input = String::from("SNP\tA1\tA2\tZ\n");
    for (index, line) in std::fs::read_to_string(&ldref_path)
        .unwrap()
        .lines()
        .enumerate()
    {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert!(fields.len() >= 6, "invalid LDREF BIM row: {line}");
        let z = ((index % 97) as f64 - 48.0) / 16.0;
        input.push_str(&format!(
            "{}\t{}\t{}\t{:.6}\n",
            fields[1], fields[4], fields[5], z
        ));
    }
    std::fs::write(&sumstats_path, input).unwrap();

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(TwasFusionContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let fusion = registry
        .build_node(
            TWAS_FUSION_CONTAINER_KIND,
            serde_json::json!({
                "tissue": "Whole_Blood",
                "chr": 21,
                "force_model": "top1",
                "timeout_secs": 3600
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "sumstats".into(),
        Box::new(FileReferenceNode::new(
            sumstats_path.to_string_lossy().into_owned(),
            Some("tsv".into()),
        )),
    )
    .unwrap();
    dag.add_node("fusion_twas".into(), fusion).unwrap();
    dag.add_edge("sumstats", "fusion_twas", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("sumstats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("fusion_twas"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("fusion_twas").unwrap();
    let result = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(result.path.ends_with("/twas_fusion.dat"));
    assert!(log.path.ends_with("/twas_fusion.log"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let result_vpath = result
        .path
        .strip_prefix("vfs://")
        .expect("FUSION result is a VFS URI");
    let bytes = storage
        .resolve(result_vpath)
        .read(&storage.resolve_path(result_vpath))
        .await
        .unwrap();
    let result_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    assert!(
        result_text.starts_with("PANEL\tFILE\tID\tCHR"),
        "FUSION result is missing its official header; got:\n{result_text}"
    );

    let log_vpath = log
        .path
        .strip_prefix("vfs://")
        .expect("FUSION log is a VFS URI");
    let bytes = storage
        .resolve(log_vpath)
        .read(&storage.resolve_path(log_vpath))
        .await
        .unwrap();
    let log_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    assert!(
        log_text.contains("FUSION TWAS completed."),
        "FUSION log is missing completion marker; got:\n{log_text}"
    );

    let cached_panel = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{FUSION_GTEX_V8_PANEL}@"))
        });
    assert!(cached_panel, "FUSION GTEx panel should remain cached");
}

#[tokio::test]
#[ignore = "requires the official SMR image, Westra BESD and 1000G EUR catalog panels, k3s PVCs, and kubeconfig"]
async fn real_catalog_backed_official_smr_heidi_runs_in_k3s() {
    use dag_core::dag::DagNode;

    let fixture = catalog_test_fixture().await;
    assert!(
        fixture.bundles.get(SMR_WESTRA_EQTL_PANEL).is_some(),
        "smr.eqtl.westra_hg19 must be published in the catalog before this test"
    );
    assert!(
        fixture.bundles.get(SMR_REF_BINARY_PANEL).is_some(),
        "plink.ref.1000g_eur.binary must be published in the catalog before this test"
    );

    let sumstats_path = std::env::var_os("AUTONOMICS_SMR_IT_SUMSTATS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../containers/smr/fixtures/chr22.westra.ma")
                .to_path_buf()
        });
    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(SmrHeidiContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let smr = registry
        .build_node(
            SMR_HEIDI_CONTAINER_KIND,
            serde_json::json!({
                "chr": 22,
                "thread_num": 2,
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "gwas_sumstats".into(),
        Box::new(FileReferenceNode::new(
            sumstats_path.to_string_lossy().into_owned(),
            Some("gcta_ma".into()),
        )),
    )
    .unwrap();
    dag.add_node("smr_heidi".into(), smr).unwrap();
    dag.add_edge("gwas_sumstats", "smr_heidi", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("gwas_sumstats"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("smr_heidi"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );

    let outputs = dag.output("smr_heidi").unwrap();
    let result = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(result.path.ends_with("/smr.smr"));
    assert!(log.path.ends_with("/smr.log"));
    assert!(
        result
            .path
            .starts_with("vfs:///artifacts/smr_heidi_container/")
    );

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let result_vpath = result
        .path
        .strip_prefix("vfs://")
        .expect("SMR result is a VFS URI");
    let bytes = storage
        .resolve(result_vpath)
        .read(&storage.resolve_path(result_vpath))
        .await
        .unwrap();
    let result_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    assert!(
        result_text.starts_with("probeID\tProbeChr\tGene\tProbe_bp\ttopSNP"),
        "SMR result is missing the official header; got:\n{result_text}"
    );
    assert!(
        result_text.contains("ILMN_1765304\t22\tPSITPTE22\t17099392\trs2000473"),
        "SMR result is missing the chr22 Westra baseline row; got:\n{result_text}"
    );
    assert!(
        result_text.contains("\t2.684279e-06\t"),
        "SMR result changed from the recorded official p_SMR baseline; got:\n{result_text}"
    );

    let log_vpath = log
        .path
        .strip_prefix("vfs://")
        .expect("SMR log is a VFS URI");
    let bytes = storage
        .resolve(log_vpath)
        .read(&storage.resolve_path(log_vpath))
        .await
        .unwrap();
    let log_text = String::from_utf8_lossy(&bytes.to_vec()).into_owned();
    for expected in ["Version 1.4.2 Linux", "Results of 5 probes"] {
        assert!(
            log_text.contains(expected),
            "official SMR log is missing `{expected}`:\n{log_text}"
        );
    }

    let cached_panels = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    for panel in [SMR_WESTRA_EQTL_PANEL, SMR_REF_BINARY_PANEL] {
        assert!(
            cached_panels
                .iter()
                .any(|name| name.starts_with(&format!("{panel}@"))),
            "SMR panel `{panel}` should be cached"
        );
    }
}

async fn run_gcta_dag(
    ctx: &NodeCtx,
    registry: &NodeRegistry,
    kind: &str,
    spec: serde_json::Value,
    input_path: &Path,
) -> Vec<dag_core::value::FileRef> {
    let node = registry.build_node(kind, spec).unwrap();
    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "gwas".into(),
        Box::new(FileReferenceNode::new(
            input_path.to_string_lossy().into_owned(),
            Some("tsv".into()),
        )),
    )
    .unwrap();
    dag.add_node("gcta".into(), node).unwrap();
    dag.add_edge("gwas", "gcta", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("gwas"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("gcta"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "official GCTA `{kind}` node failed: {report:#?}"
    );
    let outputs = dag.output("gcta").unwrap();
    (0..u8::try_from(outputs.len()).expect("GCTA output port count fits u8"))
        .map(|index: u8| outputs.get(&index).unwrap().as_file().unwrap().clone())
        .collect()
}

#[tokio::test]
#[ignore = "requires the official GCTA image, the 1000G EUR and hg19 gene-list catalog panels, k3s PVCs, and kubeconfig"]
async fn real_catalog_backed_official_gcta_summary_nodes_run_in_k3s() {
    let fixture = catalog_test_fixture().await;
    for panel in [GCTA_REF_BINARY_PANEL, GCTA_GENE_LIST_PANEL] {
        assert!(
            fixture.bundles.get(panel).is_some(),
            "{panel} must be published before the GCTA baseline"
        );
    }

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(GctaContainerNodeFactory::cojo_select(
        Arc::clone(&runtime),
        Arc::clone(&panel_cache),
    )));
    registry.register(Box::new(GctaContainerNodeFactory::sblup(
        Arc::clone(&runtime),
        Arc::clone(&panel_cache),
    )));
    registry.register(Box::new(GctaContainerNodeFactory::fastbat(
        Arc::clone(&runtime),
        Arc::clone(&panel_cache),
    )));
    registry.register(Box::new(GctaContainerNodeFactory::acat(
        Arc::clone(&runtime),
        Arc::clone(&panel_cache),
    )));

    let ma =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/gcta/fixtures/chr22.ma");
    let fastgwa = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../containers/gcta/fixtures/chr22.fastGWA");
    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");

    let outputs = run_gcta_dag(
        &fixture.ctx,
        &registry,
        GCTA_COJO_SELECT_CONTAINER_KIND,
        serde_json::json!({"chr": 22, "cojo_p": 0.05}),
        &ma,
    )
    .await;
    let jma = read_published_text(storage, &outputs[0]).await;
    let cojo_log = read_published_text(storage, &outputs[3]).await;
    assert!(outputs[0].path.ends_with("/gcta_cojo.jma.cojo"));
    assert!(outputs[1].path.ends_with("/gcta_cojo.ldr.cojo"));
    assert!(outputs[2].path.ends_with("/gcta_cojo.cma.cojo"));
    assert!(outputs[3].path.ends_with("/gcta_cojo.log"));
    assert!(
        jma.contains("rs150627359\t16940793\tT"),
        "GCTA-COJO chr22 baseline changed:\n{jma}"
    );
    assert!(cojo_log.contains("version v1.95.3 Linux"));
    assert!(cojo_log.contains("Finally, 3 associated SNPs are selected"));

    let outputs = run_gcta_dag(
        &fixture.ctx,
        &registry,
        GCTA_SBLUP_CONTAINER_KIND,
        serde_json::json!({"chr": 22, "lambda": 1.33e6}),
        &ma,
    )
    .await;
    let effects = read_published_text(storage, &outputs[0]).await;
    let sblup_log = read_published_text(storage, &outputs[1]).await;
    assert!(outputs[0].path.ends_with("/gcta_sblup.sblup.cojo"));
    assert!(outputs[1].path.ends_with("/gcta_sblup.log"));
    assert!(
        effects.starts_with("rs3949130\tA\t-8.62939\t-0.201231"),
        "GCTA-SBLUP chr22 baseline changed:\n{effects}"
    );
    assert!(sblup_log.contains("Saving the joint analysis result of 199 SNPs"));

    let outputs = run_gcta_dag(
        &fixture.ctx,
        &registry,
        GCTA_FASTBAT_CONTAINER_KIND,
        serde_json::json!({"chr": 22}),
        &ma,
    )
    .await;
    let genes = read_published_text(storage, &outputs[0]).await;
    let fastbat_log = read_published_text(storage, &outputs[1]).await;
    assert!(outputs[0].path.ends_with("/gcta_fastbat.gene.fastbat"));
    assert!(outputs[1].path.ends_with("/gcta_fastbat.log"));
    assert!(
        genes.contains("CCT8L2\t22\t17071647\t17073700\t31"),
        "GCTA-fastBAT chr22 baseline changed:\n{genes}"
    );
    assert!(fastbat_log.contains("4 genes have been mapped to SNP data"));

    let outputs = run_gcta_dag(
        &fixture.ctx,
        &registry,
        GCTA_ACAT_CONTAINER_KIND,
        serde_json::json!({"max_maf": 0.05, "min_mac": 1}),
        &fastgwa,
    )
    .await;
    let acat = read_published_text(storage, &outputs[0]).await;
    let acat_log = read_published_text(storage, &outputs[1]).await;
    assert!(outputs[0].path.ends_with("/gcta_acat.acat"));
    assert!(outputs[1].path.ends_with("/gcta_acat.log"));
    assert!(
        acat.contains("22\tTPTEP1\t17082800\t17129720\t2"),
        "GCTA ACAT-V chr22 baseline changed:\n{acat}"
    );
    assert!(acat_log.contains("--acat"));

    let cached_panels = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    for panel in [GCTA_REF_BINARY_PANEL, GCTA_GENE_LIST_PANEL] {
        assert!(
            cached_panels
                .iter()
                .any(|name| name.starts_with(&format!("{panel}@"))),
            "GCTA panel `{panel}` should be cached"
        );
    }
}

#[tokio::test]
#[ignore = "requires the Garage UKB LAVA panel, k3s PVCs, kubeconfig, the local official LAVA image, and zip"]
async fn real_catalog_backed_official_lava_univ_ukb_panel_runs_in_k3s() {
    let fixture = catalog_test_fixture().await;
    assert!(fixture.bundles.get(LAVA_UKB_EUR_PANEL).is_some());

    let scratch = tempfile::tempdir().unwrap();
    let source = std::env::var_os("AUTONOMICS_LAVA_IT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/lava/LAVA")
        });
    let fixture_data = source.join("vignettes/data");
    let bundle_root = scratch.path().join("bundle");
    let bundle_data = bundle_root.join("vignettes/data");
    std::fs::create_dir_all(&bundle_data).unwrap();
    for name in [
        "input.info.txt",
        "sample.overlap.txt",
        "test.loci",
        "depression.sumstats.txt",
        "neuro.sumstats.txt",
        "bmi.sumstats.txt",
    ] {
        std::fs::copy(fixture_data.join(name), bundle_data.join(name)).unwrap();
    }
    let bundle_zip = scratch.path().join("lava-univ-ukb-bundle.zip");
    let zip_status = Command::new("zip")
        .args(["-q", "-r"])
        .arg(&bundle_zip)
        .arg("vignettes")
        .current_dir(&bundle_root)
        .status()
        .unwrap();
    assert!(
        zip_status.success(),
        "could not create the LAVA UKB run bundle"
    );

    let k3s_config = K3sConfig::from_env();
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(LavaContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let lava = registry
        .build_node(
            LAVA_CONTAINER_KIND,
            serde_json::json!({
                "analysis": "univ",
                "panel_id": LAVA_UKB_EUR_PANEL,
                "input_info_file": "vignettes/data/input.info.txt",
                "loci_file": "vignettes/data/test.loci",
                "sample_overlap_file": "vignettes/data/sample.overlap.txt",
                "locus_index": 1,
                "phenotypes": ["depression", "neuro", "bmi"]
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "bundle".into(),
        Box::new(FileReferenceNode::new(
            bundle_zip.to_string_lossy().into_owned(),
            Some("lava_run_bundle_zip".into()),
        )),
    )
    .unwrap();
    dag.add_node("lava_univ_ukb".into(), lava).unwrap();
    dag.add_edge("bundle", "lava_univ_ukb", 0, 0).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("bundle"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    assert_eq!(
        report.statuses.get("lava_univ_ukb"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "LAVA UKB univ node failed: {report:#?}"
    );

    let outputs = dag.output("lava_univ_ukb").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(tsv.path.ends_with("/lava.tsv"));
    assert!(log.path.ends_with("/lava.log"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let tsv_text = read_published_text(storage, &tsv).await;
    let log_text = read_published_text(storage, &log).await;
    assert!(tsv_text.contains("phen\th2.obs\th2.latent\tascertained\tp"));
    for expected in [
        "depression\t4.10622e-05\t6.8555e-05\tFALSE\t0.188153",
        "neuro\t6.03189e-05\tNA\tFALSE\t0.164537",
        "bmi\t9.06806e-05\tNA\tFALSE\t0.0757959",
    ] {
        assert!(
            tsv_text.contains(expected),
            "official LAVA UKB univ baseline is missing `{expected}`:\n{tsv_text}"
        );
    }
    assert!(log_text.contains("75702 SNPs shared across data sets"));

    let cached_panel = std::fs::read_dir(&k3s_config.panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{LAVA_UKB_EUR_PANEL}@"))
        });
    assert!(cached_panel, "LAVA UKB panel should be cached");
}
