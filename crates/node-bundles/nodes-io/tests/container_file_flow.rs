//! End-to-end file-flow tests for `dataframe_to_file -> container_command -> file_to_dataframe`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use container_runtime::{
    ContainerExecutionInfra, ContainerNetwork, ContainerRunRequest, ContainerRunResult,
    ContainerRuntimeError, DEFAULT_CONTAINER_WORKDIR, PanelCache, PodmanConfig, PodmanConnection,
    PodmanRuntime, PullPolicy, unique_container_name,
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
use nodes_io::hdl_l_container::{
    HDL_L_CONTAINER_KIND, HDL_UKB_EUR_PANEL, HdlLContainerNodeFactory,
};
use nodes_io::hdl_l_scan_container::{HDL_L_SCAN_CONTAINER_KIND, HdlLScanContainerNodeFactory};
use nodes_io::hyprcoloc_container::{HYPRCOLOC_CONTAINER_KIND, HyPrColocContainerNodeFactory};
use nodes_io::lava_container::{
    LAVA_CONTAINER_KIND, LAVA_TUTORIAL_REF_PANEL, LAVA_UKB_EUR_PANEL, LavaContainerNodeFactory,
};
use nodes_io::lava_scan_container::{LAVA_SCAN_CONTAINER_KIND, LavaScanContainerNodeFactory};
use nodes_io::ldsc_h2_container::{LDSC_H2_CONTAINER_KIND, LdscH2ContainerNodeFactory};
use nodes_io::ldsc_munge_container::{LDSC_MUNGE_CONTAINER_KIND, LdscMungeContainerNodeFactory};
use nodes_io::ldsc_rg_container::{LDSC_RG_CONTAINER_KIND, LdscRgContainerNodeFactory};
use nodes_io::magma_annotate_container::{
    MAGMA_ANNOTATE_CONTAINER_KIND, MAGMA_GENE_LOC_PANEL, MagmaAnnotateContainerNodeFactory,
};
use nodes_io::mixer_container::{
    MIXER_FIT1_CONTAINER_KIND, MIXER_FIT2_CONTAINER_KIND, MIXER_G1000_EUR_RSID_PANEL,
    MixerFit1ContainerNodeFactory, MixerFit2ContainerNodeFactory,
};
use nodes_io::mrpresso_container::{MRPRESSO_CONTAINER_KIND, MrpressoContainerNodeFactory};
use nodes_io::mtag_container::{MTAG_CONTAINER_KIND, MTAG_LD_REF_PANEL, MtagContainerNodeFactory};
use nodes_io::mvmr_container::{MVMR_CONTAINER_KIND, MvmrContainerNodeFactory};
use nodes_io::plink2_clump_container::{
    PLINK2_CLUMP_CONTAINER_KIND, PLINK2_REF_BINARY_PANEL, Plink2ClumpContainerNodeFactory,
};
use nodes_io::smr_heidi_container::{
    SMR_EQTLGEN_PANEL, SMR_HEIDI_CONTAINER_KIND, SMR_REF_BINARY_PANEL, SMR_WESTRA_EQTL_PANEL,
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
impl PodmanConnection for FakeContainerRuntime {
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
                Arc::new(PanelCache::new(workspace.path().join("panels"))),
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
#[ignore = "requires a working rootless Podman runtime, the MTAG image, the MTAG LD panel, and baseline sumstats"]
async fn real_catalog_backed_official_mtag_runs_in_podman() {
    let fixture = catalog_test_fixture().await;
    assert!(
        fixture.bundles.get(MTAG_LD_REF_PANEL).is_some(),
        "the MTAG LD panel must be published before the E2E baseline"
    );

    let input1 = std::env::var_os("AUTONOMICS_MTAG_IT_SUMSTATS1")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/tmp/autonomics-mtag-it/trait1.sumstats.tsv").to_path_buf());
    let input2 = std::env::var_os("AUTONOMICS_MTAG_IT_SUMSTATS2")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/tmp/autonomics-mtag-it/trait2.sumstats.tsv").to_path_buf());
    assert!(
        input1.is_file(),
        "missing MTAG trait 1 fixture: {}",
        input1.display()
    );
    assert!(
        input2.is_file(),
        "missing MTAG trait 2 fixture: {}",
        input2.display()
    );

    let podman_state = tempfile::tempdir().unwrap();
    let workspace_root = podman_state.path().join("workspace");
    let panel_root = podman_state.path().join("panels");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&panel_root).unwrap();
    let runtime: Arc<dyn PodmanConnection> = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
        workspace_root: workspace_root.clone(),
        panel_cache_root: panel_root.clone(),
    }));
    let panel_cache = Arc::new(PanelCache::new(panel_root.clone()));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(MtagContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    // The production wrapper always pins the manifest digest. Local Podman
    // tests resolve that digest through the equivalently named localhost repo.
    // SAFETY: ignored E2E tests are run one at a time by the MTAG test script.
    unsafe {
        std::env::set_var(
            nodes_io::image_registry::ACR_ENDPOINT_ENV,
            std::env::var("AUTONOMICS_MTAG_IMAGE_ENDPOINT").unwrap_or_else(|_| "localhost".into()),
        );
    }
    let mtag = registry
        .build_node(MTAG_CONTAINER_KIND, serde_json::json!({}))
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "trait1".into(),
        Box::new(FileReferenceNode::new(
            input1.to_string_lossy().into_owned(),
            Some("mtag_sumstats".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "trait2".into(),
        Box::new(FileReferenceNode::new(
            input2.to_string_lossy().into_owned(),
            Some("mtag_sumstats".into()),
        )),
    )
    .unwrap();
    dag.add_node("mtag".into(), mtag).unwrap();
    dag.add_edge("trait1", "mtag", 0, 0).unwrap();
    dag.add_edge("trait2", "mtag", 0, 1).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("mtag"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "MTAG node failed: {report:#?}"
    );

    let outputs = dag.output("mtag").unwrap();
    let result1 = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let result2 = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(result1.path.ends_with("/mtag_trait_1.txt"));
    assert!(result2.path.ends_with("/mtag_trait_2.txt"));
    assert!(log.path.ends_with("/mtag.log"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let result1_text = read_published_text(storage, &result1).await;
    let result2_text = read_published_text(storage, &result2).await;
    let log_text = read_published_text(storage, &log).await;
    for (name, text) in [("trait1", &result1_text), ("trait2", &result2_text)] {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default();
        assert!(
            header.contains("SNP")
                && header.contains("mtag_beta")
                && header.contains("mtag_se")
                && header.contains("mtag_z")
                && header.contains("mtag_pval"),
            "official MTAG {name} header changed:\n{header}"
        );
        assert_eq!(text.lines().count(), 199_648);
    }
    assert!(result1_text.contains(
        "rs4075116\t1\t1003629\tT\tC\t0.0066\t1316440.0\t0.263852242744\t0.0003543020856258398\t0.0013731145897079259\t0.2580280540906663\t0.7963852560981179"
    ));
    assert!(result2_text.contains(
        "rs4075116\t1\t1003629\tT\tC\t1.95\t680426.0\t0.263852242744\t0.0037649988753020768\t0.001860444510962918\t2.0237093087787987\t0.04300007022939045"
    ));
    for expected in ["199647", "1.833", "4.555"] {
        assert!(
            log_text.contains(expected),
            "official MTAG numerical baseline is missing `{expected}`:\n{log_text}"
        );
    }
    assert!(
        log_text.contains("MTAG: Multi-trait Analysis of GWAS"),
        "official MTAG masthead is missing:\n{log_text}"
    );
    assert!(
        log_text.contains("MTAG complete."),
        "official MTAG run did not complete:\n{log_text}"
    );

    let cached_panel = std::fs::read_dir(&panel_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{MTAG_LD_REF_PANEL}@"))
        });
    assert!(cached_panel, "MTAG panel should be cached");
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
#[ignore = "requires rootless Podman and the local official HDL image"]
async fn real_official_hdl_l_runs_in_podman_with_local_panel() {
    let scratch = tempfile::tempdir().unwrap();
    let fixture_root = scratch.path().join("official-fixture");
    let panel_staging = fixture_root.join("panel-staging");
    std::fs::create_dir_all(panel_staging.join("LD")).unwrap();
    std::fs::create_dir_all(panel_staging.join("bim")).unwrap();

    // The fixture panel is deliberately small. It has the official HDL-L LD
    // SVD object names and layout, so the full runtime path still executes the
    // official HDL::HDL.L implementation without downloading the 1.8 GiB UKB
    // panel for a smoke test.
    let generator = r#"
set.seed(123)
n <- 40
ids <- sprintf("rs%06d", 100001:100040)
rho <- 0.8
ld <- rho^abs(outer(seq_len(n), seq_len(n), "-"))
LDsc <- as.numeric(colSums(ld))
eig <- eigen(ld, symmetric = TRUE)
lam <- pmax(eig$values, 1e-8)
V <- eig$vectors
rownames(V) <- ids
save(LDsc, lam, V, file = "/work/panel-staging/LD/ukb_chr1.3_fixture_LDSVD.rda")
NEWLOC <- data.frame(CHR = 1L, piece = 3L)
save(NEWLOC, file = "/work/panel-staging/LD/HDLL_LOC_snps.RData")
bim <- data.frame(
  chr = 1L,
  id = ids,
  cm = 0L,
  pos = seq_len(n) * 100L,
  A1 = "A",
  A2 = "G",
  stringsAsFactors = FALSE
)
write.table(
  bim,
  "/work/panel-staging/bim/ukb_chr1.3_fixture.bim",
  sep = "\t",
  quote = FALSE,
  row.names = FALSE,
  col.names = FALSE
)
z <- as.numeric(scale(LDsc)) * 4
gwas1 <- data.frame(SNP = ids, A1 = "A", A2 = "G", N = 100000, Z = z)
gwas2 <- data.frame(SNP = ids, A1 = "A", A2 = "G", N = 100000, Z = z + 0.1)
write.table(gwas1, "/work/gwas1.tsv", sep = "\t", quote = FALSE, row.names = FALSE)
write.table(gwas2, "/work/gwas2.tsv", sep = "\t", quote = FALSE, row.names = FALSE)
"#;
    let podman_program =
        std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into());
    let image = std::env::var("AUTONOMICS_HDL_IMAGE")
        .unwrap_or_else(|_| nodes_io::hdl_l_container::HDL_ORIGINAL_IMAGE.into());
    let fixture_mount = format!("{}:/work", fixture_root.display());
    let status = Command::new(&podman_program)
        .args([
            "run",
            "--rm",
            "--volume",
            &fixture_mount,
            &image,
            "-e",
            generator,
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "official HDL image failed to build fixture"
    );

    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("population".to_string(), "EUR".to_string());
    metadata.insert(
        "description".to_string(),
        "HDL-L official-layout local Podman smoke panel".to_string(),
    );
    let panel_package = scratch.path().join("panel-package");
    let built = data_catalog::package::build_package(
        &panel_staging,
        &panel_package,
        data_catalog::package::BuildOptions {
            id: Some(HDL_UKB_EUR_PANEL.to_string()),
            version: Some("v1.0-podman-fixture".to_string()),
            kind: Some("hdl_ld_svd_ref".to_string()),
            metadata,
            payload: [
                ("ld_dir".to_string(), "LD".into()),
                ("bim_dir".to_string(), "bim".into()),
                (
                    "ld_file_template".to_string(),
                    "LD/ukb_chr{chr}.{piece}_fixture_LDSVD.rda".into(),
                ),
            ]
            .into_iter()
            .collect(),
            force: false,
        },
    )
    .unwrap();

    let panel_runtime = scratch.path().join("panel-runtime");
    std::fs::create_dir_all(panel_runtime.join("LD")).unwrap();
    std::fs::create_dir_all(panel_runtime.join("bim")).unwrap();
    std::fs::copy(
        panel_package.join("manifest.json"),
        panel_runtime.join("manifest.json"),
    )
    .unwrap();
    for file in [
        "LD/ukb_chr1.3_fixture_LDSVD.rda",
        "LD/HDLL_LOC_snps.RData",
        "bim/ukb_chr1.3_fixture.bim",
    ] {
        std::fs::copy(
            panel_package.join("payload").join(file),
            panel_runtime.join(file),
        )
        .unwrap();
    }

    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "hdl-podman-test-local".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![
            MountDefinition {
                path: "/".into(),
                backend: "hdl-podman-test-local".into(),
                source: scratch.path().to_string_lossy().into_owned(),
                read_only: false,
            },
            MountDefinition {
                path: "/catalog/hdl_ref".into(),
                backend: "hdl-podman-test-local".into(),
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
    session
        .runtime_env()
        .register_object_store(ObjectStoreUrl::parse("vfs://").unwrap().as_ref(), mounted);
    let ctx = NodeCtx::new(session.runtime_env(), Some(storage));

    let mut panel = dag_core::DataBundle::new(
        HDL_UKB_EUR_PANEL,
        "HDL-L official-format local smoke panel",
        "/catalog/hdl_ref",
    );
    panel.source = Some("/catalog/hdl_ref".into());
    panel.digest = built.manifest.digest.clone();
    let registry_ctx = ctx.clone().with_data_bundle_catalog(Arc::new(
        dag_core::DataBundleCatalog::from_bundles([panel]).unwrap(),
    ));

    let workspace_root = scratch.path().join("podman-workspace");
    let panel_cache_root = scratch.path().join("podman-panels");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&panel_cache_root).unwrap();
    let runtime: Arc<dyn PodmanConnection> = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: podman_program,
        workspace_root,
        panel_cache_root: panel_cache_root.clone(),
    }));
    let panel_cache = Arc::new(PanelCache::new(panel_cache_root.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(HdlLContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let hdl_l = registry
        .build_node(
            HDL_L_CONTAINER_KIND,
            serde_json::json!({
                "chr": 1,
                "piece": 3,
                "trait1_name": "trait1",
                "trait2_name": "trait2"
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "trait1".into(),
        Box::new(FileReferenceNode::new(
            fixture_root
                .join("gwas1.tsv")
                .to_string_lossy()
                .into_owned(),
            Some("hdl_l_sumstats".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "trait2".into(),
        Box::new(FileReferenceNode::new(
            fixture_root
                .join("gwas2.tsv")
                .to_string_lossy()
                .into_owned(),
            Some("hdl_l_sumstats".into()),
        )),
    )
    .unwrap();
    dag.add_node("hdl_l".into(), hdl_l).unwrap();
    dag.add_edge("trait1", "hdl_l", 0, 0).unwrap();
    dag.add_edge("trait2", "hdl_l", 0, 1).unwrap();
    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("hdl_l"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "HDL-L node failed: {report:#?}"
    );

    let outputs = dag.output("hdl_l").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let rds = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(tsv.path.ends_with("/hdl_l.tsv"));
    assert!(rds.path.ends_with("/hdl_l.RDS"));
    assert!(log.path.ends_with("/hdl_l.log"));
    assert!(tsv.path.starts_with("vfs:///artifacts/hdl_l_container/"));

    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    let tsv_text = read_published_text(storage, &tsv).await;
    let log_text = read_published_text(storage, &log).await;
    for expected in [
        "Trait1",
        "Trait2",
        "Heritability_1",
        "Heritability_2",
        "Genetic_Correlation",
    ] {
        assert!(
            tsv_text.contains(expected),
            "official HDL-L result is missing `{expected}`:\n{tsv_text}"
        );
    }
    for expected in ["Analysis starts on", "Analysis finished at"] {
        assert!(
            log_text.contains(expected),
            "official HDL-L log is missing `{expected}`:\n{log_text}"
        );
    }

    let cached_panel = std::fs::read_dir(&panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{HDL_UKB_EUR_PANEL}@"))
        });
    assert!(cached_panel, "HDL UKB panel should be cached");
}

#[tokio::test]
#[ignore = "requires the published HDL UKB catalog panel, rootless Podman, the official HDL image, and HDL-L sumstats"]
async fn real_catalog_backed_official_hdl_l_runs_in_podman_with_published_panel() {
    let fixture = catalog_test_fixture().await;
    let panel = fixture
        .bundles
        .get(HDL_UKB_EUR_PANEL)
        .expect("published hdl.ref.ukb_eur panel must be current");
    assert_eq!(
        panel.digest.as_deref(),
        Some("sha256:411c7ae1db876ec3e17941367a74567175ca151f8f93dc6e5e1d06bb8f3a3f54")
    );

    let input1 = std::env::var_os("AUTONOMICS_HDL_REAL_IT_SUMSTATS1")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/tmp/autonomics-hdl-real-e2e/gwas1.tsv").to_path_buf());
    let input2 = std::env::var_os("AUTONOMICS_HDL_REAL_IT_SUMSTATS2")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/tmp/autonomics-hdl-real-e2e/gwas2.tsv").to_path_buf());
    assert!(
        input1.is_file() && input2.is_file(),
        "missing official HDL-L sumstats: {} and {}",
        input1.display(),
        input2.display()
    );

    let scratch = tempfile::tempdir().unwrap();
    let workspace_root = scratch.path().join("podman-workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let external_panel_cache =
        std::env::var_os("AUTONOMICS_HDL_REAL_IT_PANEL_CACHE").map(PathBuf::from);
    let panel_cache_root = external_panel_cache.unwrap_or_else(|| scratch.path().join("panels"));
    std::fs::create_dir_all(&panel_cache_root).unwrap();

    let podman_program =
        std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into());
    let runtime: Arc<dyn PodmanConnection> = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: podman_program,
        workspace_root,
        panel_cache_root: panel_cache_root.clone(),
    }));
    let panel_cache = Arc::new(PanelCache::new(panel_cache_root.clone()));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(HdlLContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let hdl_l = registry
        .build_node(
            HDL_L_CONTAINER_KIND,
            serde_json::json!({
                "chr": 1,
                "piece": 9,
                "trait1_name": "trait1",
                "trait2_name": "trait2"
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node("hdl_l".into(), hdl_l).unwrap();
    for (port, input) in [input1, input2].into_iter().enumerate() {
        let source = format!("trait{}", port + 1);
        dag.add_node(
            source.clone(),
            Box::new(FileReferenceNode::new(
                input.to_string_lossy().into_owned(),
                Some("hdl_l_sumstats".into()),
            )),
        )
        .unwrap();
        dag.add_edge(&source, "hdl_l", 0, u8::try_from(port).unwrap())
            .unwrap();
    }
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("hdl_l"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "published-panel HDL-L node failed: {report:#?}"
    );

    let outputs = dag.output("hdl_l").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let rds = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(tsv.path.ends_with("/hdl_l.tsv"));
    assert!(rds.path.ends_with("/hdl_l.RDS"));
    assert!(log.path.ends_with("/hdl_l.log"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let tsv_text = read_published_text(storage, &tsv).await;
    let log_text = read_published_text(storage, &log).await;
    for expected in [
        "Trait1",
        "Trait2",
        "Heritability_1",
        "Heritability_2",
        "Genetic_Correlation",
    ] {
        assert!(
            tsv_text.contains(expected),
            "published-panel HDL-L result is missing `{expected}`:\n{tsv_text}"
        );
    }
    let result_fields = tsv_text
        .lines()
        .nth(1)
        .unwrap_or_default()
        .split('\t')
        .collect::<Vec<_>>();
    for (index, expected) in [
        "trait1",
        "trait2",
        "1",
        "9",
        "0.99",
        "0.000107249476351492",
        "0.25964719975359",
        "1.34562333962111e-05",
        "0.954243705297244",
        "-3.79891298321915e-05",
        "-1",
        "-1",
        "1",
        "0.328067237903238",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            result_fields.get(index),
            Some(&expected),
            "official chr1:9 result baseline changed:\n{tsv_text}"
        );
    }
    assert_eq!(
        log_text
            .matches("642 out of 642 (100%) SNPs in reference panel")
            .count(),
        2,
        "official chr1:9 panel overlap changed:\n{log_text}"
    );
    assert!(log_text.contains("Analysis finished at"));

    let cache_key = format!("{HDL_UKB_EUR_PANEL}@{}", panel.digest.as_deref().unwrap());
    assert!(
        panel_cache_root.join(cache_key).is_dir(),
        "published HDL UKB panel should be cached"
    );
}

#[tokio::test]
#[ignore = "requires the published HDL UKB catalog panel, rootless Podman, the official HDL image, and HDL-L sumstats"]
async fn real_catalog_backed_official_hdl_l_scan_runs_in_podman_with_published_panel() {
    let fixture = catalog_test_fixture().await;
    let panel = fixture
        .bundles
        .get(HDL_UKB_EUR_PANEL)
        .expect("published hdl.ref.ukb_eur panel must be current");
    assert_eq!(
        panel.digest.as_deref(),
        Some("sha256:411c7ae1db876ec3e17941367a74567175ca151f8f93dc6e5e1d06bb8f3a3f54")
    );

    let input1 = std::env::var_os("AUTONOMICS_HDL_REAL_IT_SUMSTATS1")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/tmp/autonomics-hdl-real-e2e/gwas1.tsv").to_path_buf());
    let input2 = std::env::var_os("AUTONOMICS_HDL_REAL_IT_SUMSTATS2")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/tmp/autonomics-hdl-real-e2e/gwas2.tsv").to_path_buf());
    assert!(
        input1.is_file() && input2.is_file(),
        "missing official HDL-L sumstats: {} and {}",
        input1.display(),
        input2.display()
    );

    let scratch = tempfile::tempdir().unwrap();
    let workspace_root = scratch.path().join("podman-workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let panel_cache_root = std::env::var_os("AUTONOMICS_HDL_REAL_IT_PANEL_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch.path().join("panels"));
    std::fs::create_dir_all(&panel_cache_root).unwrap();

    let runtime: Arc<dyn PodmanConnection> = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
        workspace_root,
        panel_cache_root: panel_cache_root.clone(),
    }));
    let panel_cache = Arc::new(PanelCache::new(panel_cache_root.clone()));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(HdlLScanContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let scan = registry
        .build_node(
            HDL_L_SCAN_CONTAINER_KIND,
            serde_json::json!({
                "chr": 1,
                "pieces": [9],
                "trait1_name": "trait1",
                "trait2_name": "trait2"
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node("hdl_l_scan".into(), scan).unwrap();
    for (port, input) in [input1, input2].into_iter().enumerate() {
        let source = format!("trait{}", port + 1);
        dag.add_node(
            source.clone(),
            Box::new(FileReferenceNode::new(
                input.to_string_lossy().into_owned(),
                Some("hdl_l_sumstats".into()),
            )),
        )
        .unwrap();
        dag.add_edge(&source, "hdl_l_scan", 0, u8::try_from(port).unwrap())
            .unwrap();
    }
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("hdl_l_scan"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "published-panel HDL-L scan failed: {report:#?}"
    );

    let outputs = dag.output("hdl_l_scan").unwrap();
    let tsv = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let rds = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&2).unwrap().as_file().unwrap().clone();
    assert!(tsv.path.ends_with("/hdl_l_scan.tsv"));
    assert!(rds.path.ends_with("/hdl_l_scan.RDS"));
    assert!(log.path.ends_with("/hdl_l_scan.log"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let tsv_text = read_published_text(storage, &tsv).await;
    let log_text = read_published_text(storage, &log).await;
    assert_eq!(tsv_text.lines().count(), 2, "scan TSV:\n{tsv_text}");
    let result_fields = tsv_text
        .lines()
        .nth(1)
        .unwrap()
        .split('\t')
        .collect::<Vec<_>>();
    assert_eq!(result_fields.len(), 14);
    assert_eq!(result_fields[2], "1");
    assert_eq!(result_fields[3], "9");
    assert_eq!(result_fields[5], "0.000107249476351492");
    assert_eq!(result_fields[13], "0.328067237903238");
    assert_eq!(
        log_text
            .matches("642 out of 642 (100%) SNPs in reference panel")
            .count(),
        2,
        "official chr1:9 scan overlap changed:\n{log_text}"
    );
    assert!(log_text.contains("Processed 1 official blocks; 0 failed"));
}

#[tokio::test]
#[ignore = "requires the Garage LAVA tutorial panel, rootless Podman, and the local official LAVA image"]
async fn real_catalog_backed_official_lava_scan_runs_in_podman() {
    let fixture = catalog_test_fixture().await;
    assert!(fixture.bundles.get(LAVA_TUTORIAL_REF_PANEL).is_some());

    let source = std::env::var_os("AUTONOMICS_LAVA_IT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../containers/lava/LAVA")
        });
    let fixture_data = source.join("vignettes/data");

    let scratch = tempfile::tempdir().unwrap();
    let workspace_root = scratch.path().join("podman-workspace");
    let panel_cache_root = std::env::var_os("AUTONOMICS_LAVA_SCAN_IT_PANEL_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch.path().join("panels"));
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&panel_cache_root).unwrap();

    let runtime: Arc<dyn PodmanConnection> = Arc::new(PodmanRuntime::new(PodmanConfig {
        program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
        workspace_root,
        panel_cache_root: panel_cache_root.clone(),
    }));
    let panel_cache = Arc::new(PanelCache::new(panel_cache_root.clone()));
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(LavaScanContainerNodeFactory::new(
        runtime,
        panel_cache,
    )));
    let scan = registry
        .build_node(
            LAVA_SCAN_CONTAINER_KIND,
            serde_json::json!({
                "panel_id": LAVA_TUTORIAL_REF_PANEL,
                "sample_overlap": true,
                "phenotypes": ["depression", "neuro", "bmi"],
                "locus_ids": ["100", "230"]
            }),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    for (name, filename, format) in [
        ("input_info", "input.info.txt", "lava_input_info"),
        ("loci", "test.loci", "lava_loci"),
        ("overlap", "sample.overlap.txt", "lava_sample_overlap"),
        ("depression", "depression.sumstats.txt", "lava_sumstats"),
        ("neuro", "neuro.sumstats.txt", "lava_sumstats"),
        ("bmi", "bmi.sumstats.txt", "lava_sumstats"),
    ] {
        dag.add_node(
            name.into(),
            Box::new(FileReferenceNode::new(
                fixture_data.join(filename).to_string_lossy().into_owned(),
                Some(format.into()),
            )),
        )
        .unwrap();
    }
    dag.add_node("lava_scan".into(), scan).unwrap();
    for (source, target_port) in [
        ("input_info", 0),
        ("loci", 1),
        ("overlap", 2),
        ("depression", 3),
        ("neuro", 4),
        ("bmi", 5),
    ] {
        dag.add_edge(source, "lava_scan", 0, target_port).unwrap();
    }
    let report = dag
        .run(&SchedulerConfig::default(), &fixture.ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("lava_scan"),
        Some(&dag_core::dag::RuntimeStatus::Success),
        "LAVA scan node failed: {report:#?}"
    );

    let outputs = dag.output("lava_scan").unwrap();
    let univ = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let bivar = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let rds = outputs.get(&2).unwrap().as_file().unwrap().clone();
    let log = outputs.get(&3).unwrap().as_file().unwrap().clone();
    assert!(univ.path.ends_with("/lava_scan.univ.tsv"));
    assert!(bivar.path.ends_with("/lava_scan.bivar.tsv"));
    assert!(rds.path.ends_with("/lava_scan.RDS"));
    assert!(log.path.ends_with("/lava_scan.log"));

    let storage = fixture
        .ctx
        .opendal
        .as_ref()
        .expect("test storage is registered");
    let univ_text = read_published_text(storage, &univ).await;
    let bivar_text = read_published_text(storage, &bivar).await;
    let log_text = read_published_text(storage, &log).await;
    assert!(univ_text.contains("locus\tchr\tstart\tstop\tn.snps\tn.pcs\tphen\t"));
    assert!(univ_text.contains("100\t1\t113418038"));
    assert!(univ_text.contains("depression\t8.45733e-05"));
    assert!(univ_text.contains("230\t2\t26894103"));
    assert!(bivar_text.contains("locus\tchr\tstart\tstop\tn.snps\tn.pcs\tphen1\tphen2\t"));
    assert!(bivar_text.contains("100\t1\t113418038"));
    assert!(log_text.contains("Starting official LAVA scan for 2 loci"));
    assert!(log_text.contains("Finished official LAVA scan: 2 requested;"));
    assert!(log_text.contains("98667 SNPs shared across data sets"));

    let cached_panel = std::fs::read_dir(&panel_cache_root)
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
#[ignore = "requires the official gsa-mixer fixtures, rootless Podman, and the ACR MiXeR image"]
async fn real_official_mixer_fit1_and_fit2_run_in_podman_and_match_baselines() {
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
            id: Some(MIXER_G1000_EUR_RSID_PANEL.to_string()),
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
        MIXER_G1000_EUR_RSID_PANEL,
        "MiXeR GRCh37 EUR migration fixture",
        "/catalog/mixer_g1000_eur",
    );
    panel.source = Some("/catalog/mixer_g1000_eur".into());
    panel.digest = built.manifest.digest.clone();
    let registry_ctx = ctx.clone().with_data_bundle_catalog(Arc::new(
        dag_core::DataBundleCatalog::from_bundles([panel]).unwrap(),
    ));

    let workspace_root = scratch.path().join("podman-workspace");
    let panel_cache_root = scratch.path().join("podman-panels");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&panel_cache_root).unwrap();
    let podman_config = PodmanConfig {
        program: "podman".into(),
        workspace_root,
        panel_cache_root: panel_cache_root.clone(),
    };
    let runtime: Arc<dyn PodmanConnection> = Arc::new(PodmanRuntime::new(podman_config));
    let panel_cache = Arc::new(PanelCache::new(panel_cache_root.clone()));
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
        "{MIXER_G1000_EUR_RSID_PANEL}@{}",
        built.manifest.digest.as_deref().unwrap()
    );
    assert!(
        panel_cache_root.join(cache_key).is_dir(),
        "MiXeR fixture panel should remain in PanelCache"
    );
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
#[ignore = "requires the official SMR image, eQTLGen and 1000G EUR catalog panels, and a configured Podman backend"]
async fn real_catalog_backed_official_smr_heidi_eqtlgen_runs_with_container_backend() {
    use dag_core::dag::DagNode;

    let fixture = catalog_test_fixture().await;
    assert!(
        fixture.bundles.get(SMR_EQTLGEN_PANEL).is_some(),
        "smr.eqtl.eqtlgen_hg19 must be published in the catalog before this test"
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
    let infra = ContainerExecutionInfra::from_env();
    let panel_cache_root = infra.config.panel_cache_root.clone();
    let registry_ctx = fixture
        .ctx
        .clone()
        .with_data_bundle_catalog(Arc::new(fixture.bundles.clone()));
    let mut registry = NodeRegistry::new(registry_ctx);
    registry.register(Box::new(SmrHeidiContainerNodeFactory::new(
        infra.runtime,
        infra.panel_cache,
    )));
    let smr = registry
        .build_node(
            SMR_HEIDI_CONTAINER_KIND,
            serde_json::json!({
                "eqtl_source": "eqtlgen",
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
    if !report.ok {
        eprintln!("SMR container-backend run report: {report:#?}");
    }
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
        "eQTLGen SMR result is missing the official header; got:\n{result_text}"
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
    assert!(
        log_text.contains("Version 1.4.2 Linux"),
        "official SMR log is missing version marker:\n{log_text}"
    );

    let cached_panels = std::fs::read_dir(&panel_cache_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    for panel in [SMR_EQTLGEN_PANEL, SMR_REF_BINARY_PANEL] {
        assert!(
            cached_panels
                .iter()
                .any(|name| name.starts_with(&format!("{panel}@"))),
            "SMR panel `{panel}` should be cached"
        );
    }
}
