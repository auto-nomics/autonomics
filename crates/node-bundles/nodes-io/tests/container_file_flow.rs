//! End-to-end file-flow tests for `sink_file -> container_command -> source_file`.

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

use nodes_io::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use nodes_io::file_ref_source::FileRefSourceNode;
use nodes_io::lava_container::{
    LAVA_CONTAINER_KIND, LAVA_TUTORIAL_REF_PANEL, LavaContainerNodeFactory,
};
use nodes_io::ldsc_h2_container::{LDSC_H2_CONTAINER_KIND, LdscH2ContainerNodeFactory};
use nodes_io::ldsc_rg_container::{LDSC_RG_CONTAINER_KIND, LdscRgContainerNodeFactory};
use nodes_io::magma_annotate_container::{
    MAGMA_ANNOTATE_CONTAINER_KIND, MAGMA_GENE_LOC_PANEL, MagmaAnnotateContainerNodeFactory,
};
use nodes_io::mrpresso_container::{MRPRESSO_CONTAINER_KIND, MrpressoContainerNodeFactory};
use nodes_io::mvmr_container::{MVMR_CONTAINER_KIND, MvmrContainerNodeFactory};
use nodes_io::sink_file::{FileSinkNode, WriteFormat};
use nodes_io::source_file::FileSourceNode;
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
        Box::new(FileRefSourceNode::new(
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
async fn sink_file_output_flows_through_container_command_in_dag() {
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
        Box::new(FileSinkNode::new(
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
        Box::new(FileSourceNode::new(None, None)),
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
async fn real_k3s_container_receives_upstream_sink_file_output() {
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
    let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
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
        Box::new(FileSinkNode::new(
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
        Box::new(FileSourceNode::new(None, None)),
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
    let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
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
    let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
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
        Box::new(FileRefSourceNode::new(
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
    let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
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
        Box::new(FileRefSourceNode::new(
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
    let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
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
        Box::new(FileRefSourceNode::new(
            asthma_path.to_string_lossy().into_owned(),
            Some("sumstats_gz".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "trait2".into(),
        Box::new(FileRefSourceNode::new(
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
    let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
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
        Box::new(FileRefSourceNode::new(
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
    let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
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
        Box::new(FileRefSourceNode::new(
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
