//! End-to-end file-flow tests for `sink_file -> container_command -> source_file`.

use std::path::{Path, PathBuf};
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
use dag_core::registry::NodeCtx;
use data_catalog::{CatalogConfig, CatalogRuntime};
use datafusion::common::HashMap;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::SessionContext;

use nodes_io::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
    ContainerPanelBundleSpec,
};
use nodes_io::sink_file::{FileSinkNode, WriteFormat};
use nodes_io::source_file::FileSourceNode;
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
async fn real_catalog_backed_original_ldsc_h2_runs_in_k3s() {
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
    let panel_bundles = panel_ids
        .iter()
        .map(|id| {
            bundles
                .get(id)
                .cloned()
                .unwrap_or_else(|| panic!("catalog is missing {id}"))
        })
        .collect::<Vec<_>>();

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
    let run_suffix = unique_container_name();
    let workspace = k3s_config
        .workspace_root
        .join(format!("ldsc-panel-smoke-{run_suffix}"));
    std::fs::create_dir_all(&workspace).unwrap();
    let image = std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
        .unwrap_or_else(|_| "localhost/atc/ldsc:3.0".into());
    let mut spec = container_spec(
        &image,
        &workspace,
        format!("/artifacts/ldsc-panel-smoke/{run_suffix}"),
    );
    spec.command = vec!["sh".into()];
    spec.outputs = vec![
        ContainerCommandOutputSpec {
            path: "panel-inventory.txt".into(),
            format: Some("txt".into()),
        },
        ContainerCommandOutputSpec {
            path: "ldsc_h2.log".into(),
            format: Some("ldsc_log".into()),
        },
    ];
    spec.script = Some(
        r#"set -eu
ref_scores=$(find /panels/ref_ld -maxdepth 1 -type f -name 'LDscore.*.l2.ldscore.gz' | wc -l)
ref_m=$(find /panels/ref_ld -maxdepth 1 -type f -name 'LDscore.*.l2.M' | wc -l)
ref_m_5_50=$(find /panels/ref_ld -maxdepth 1 -type f -name 'LDscore.*.l2.M_5_50' | wc -l)
wld_scores=$(find /panels/w_ld -maxdepth 1 -type f -name 'weights.hm3_noMHC.*.l2.ldscore.gz' | wc -l)
test "$ref_scores" -eq 22
test "$ref_m" -eq 22
test "$ref_m_5_50" -eq 22
test "$wld_scores" -eq 22
gzip -t /panels/ref_ld/LDscore.*.l2.ldscore.gz
gzip -t /panels/w_ld/weights.hm3_noMHC.*.l2.ldscore.gz
{
  echo "ref_ld_scores=$ref_scores"
  echo "ref_ld_M=$ref_m"
  echo "ref_ld_M_5_50=$ref_m_5_50"
  echo "w_ld_scores=$wld_scores"
  find /panels/ref_ld /panels/w_ld -maxdepth 1 -type f | sort
} > "$AUTONOMICS_OUTPUT0"
ldsc \
  --h2 "$AUTONOMICS_INPUT0" \
  --ref-ld-chr /panels/ref_ld/LDscore. \
  --w-ld-chr /panels/w_ld/weights.hm3_noMHC. \
  --out "$AUTONOMICS_WORKDIR/ldsc_h2" \
  > "$AUTONOMICS_OUTPUT1" 2>&1"#
            .into(),
    );
    spec.panel_bundles = vec![
        ContainerPanelBundleSpec {
            panel_id: panel_ids[0].into(),
            mount_path: "/panels/ref_ld".into(),
        },
        ContainerPanelBundleSpec {
            panel_id: panel_ids[1].into(),
            mount_path: "/panels/w_ld".into(),
        },
    ];

    let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
    let panel_cache = Arc::new(PanelCache::new(
        k3s_config.panel_cache_root.clone(),
        k3s_config.panel_pvc_prefix,
    ));
    let mut node =
        ContainerCommandNode::new_with_catalog_panels(spec, runtime, panel_cache, panel_bundles)
            .unwrap();
    let input = dag_core::value::FileRef::local(&sumstats_path, Some("sumstats_gz".into()))
        .expect("LDSC integration test sumstats must exist");
    let outputs = node
        .execute(
            &ctx,
            &[NodeInput::file(0, input)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

    let inventory_path = workspace.join("panel-inventory.txt");
    let inventory = std::fs::read_to_string(&inventory_path).unwrap();
    for expected in [
        "ref_ld_scores=22",
        "ref_ld_M=22",
        "ref_ld_M_5_50=22",
        "w_ld_scores=22",
    ] {
        assert!(
            inventory.lines().any(|line| line == expected),
            "panel inventory is missing `{expected}`:\n{inventory}"
        );
    }

    let output = outputs.get(&0).unwrap().as_file().unwrap().clone();
    assert!(
        output
            .path
            .starts_with("vfs:///artifacts/ldsc-panel-smoke/"),
        "unexpected LDSC panel artifact path: {}",
        output.path
    );
    let virtual_path = output
        .path
        .strip_prefix("vfs://")
        .expect("LDSC panel artifact is a VFS URI");
    let storage = ctx.opendal.as_ref().expect("test storage is registered");
    let published = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    assert_eq!(published.to_vec(), inventory.as_bytes());

    let output = outputs.get(&1).unwrap().as_file().unwrap().clone();
    assert!(
        output.path.ends_with("/ldsc_h2.log"),
        "unexpected LDSC h2 artifact path: {}",
        output.path
    );
    let virtual_path = output
        .path
        .strip_prefix("vfs://")
        .expect("LDSC h2 artifact is a VFS URI");
    let log_bytes = storage
        .resolve(virtual_path)
        .read(&storage.resolve_path(virtual_path))
        .await
        .unwrap();
    let log = String::from_utf8_lossy(&log_bytes.to_vec()).into_owned();
    assert!(
        log.contains("Total Observed scale h2: 0.0196 (0.0014)"),
        "unexpected LDSC h2 result:\n{log}"
    );
    assert!(
        log.contains("Intercept: 1.1516 (0.0132)"),
        "unexpected LDSC intercept:\n{log}"
    );

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
