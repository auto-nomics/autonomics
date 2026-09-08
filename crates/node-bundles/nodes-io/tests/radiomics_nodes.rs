//! Unit and fake-runtime integration tests for the radiomics Stage-A nodes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow_array::{Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, DEFAULT_CONTAINER_WORKDIR,
    PanelCache, PodmanConnection,
};
use dag_core::dag::runtime::SchedulerConfig;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::SessionContext;
use nodes_io::file_reference::FileReferenceNode;
use nodes_io::radiomics::{
    RadiomicsDcmGlobNodeFactory, RadiomicsManifestNodeFactory, RadiomicsStageFileSetNodeFactory,
};
use nodes_io::radiomics_container::{RADIOMICS_PAIR_VALIDATE_KIND, RadiomicsContainerNodeFactory};
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

#[derive(Default)]
struct CopyPairsRuntime {
    workspace_root: Option<PathBuf>,
    requests: Mutex<Vec<ContainerRunRequest>>,
    /// `(AUTONOMICS_INPUT*, contents)` snapshotted during the run: the real
    /// node removes its scratch workspace after success, so staged inputs
    /// cannot be inspected on disk afterwards.
    input_contents: Mutex<Vec<(String, Vec<u8>)>>,
}

impl CopyPairsRuntime {
    fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: Some(workspace_root.to_path_buf()),
            requests: Mutex::new(Vec::new()),
            input_contents: Mutex::new(Vec::new()),
        }
    }

    fn host_path(request: &ContainerRunRequest, container_path: &str) -> PathBuf {
        let relative = Path::new(container_path)
            .strip_prefix(DEFAULT_CONTAINER_WORKDIR)
            .expect("runtime path is inside /work");
        request.workspace.host_path.join(relative)
    }
}

#[async_trait]
impl PodmanConnection for CopyPairsRuntime {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError> {
        for index in 0..2 {
            let input = request
                .env
                .iter()
                .find(|(name, _)| name.as_str() == format!("AUTONOMICS_INPUT{index}"))
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ContainerRuntimeError::Invalid("missing input".into()))?;
            let output = request
                .env
                .iter()
                .find(|(name, _)| name.as_str() == format!("AUTONOMICS_OUTPUT{index}"))
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ContainerRuntimeError::Invalid("missing output".into()))?;
            std::fs::copy(
                Self::host_path(&request, &input),
                Self::host_path(&request, &output),
            )?;
            self.input_contents.lock().unwrap().push((
                input.clone(),
                std::fs::read(Self::host_path(&request, &input))?,
            ));
        }
        self.requests.lock().unwrap().push(request);
        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "copied image and mask".into(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-radiomics-copy-pairs"
    }

    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

fn workspace_ctx(workspace: &Path) -> NodeCtx {
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "workspace".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "workspace".into(),
            source: workspace.to_string_lossy().into_owned(),
            read_only: false,
        }],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(OpendalFileStorage::with_mounts(workspace, mounted.clone()));
    let session = SessionContext::new();
    session
        .runtime_env()
        .register_object_store(ObjectStoreUrl::parse("vfs://").unwrap().as_ref(), mounted);
    NodeCtx::new(session.runtime_env(), Some(storage))
}

fn input_batch(image: &str, mask: &str) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("patient_id", DataType::Utf8, false),
            Field::new("image_id", DataType::Utf8, false),
            Field::new("image_uri", DataType::Utf8, false),
            Field::new("mask_uri", DataType::Utf8, false),
            Field::new("modality", DataType::Utf8, false),
            Field::new("roi_id", DataType::Utf8, false),
            Field::new("mask_label", DataType::Int32, false),
            Field::new("preset_id", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["STS_001"])),
            Arc::new(StringArray::from(vec!["CT"])),
            Arc::new(StringArray::from(vec![image])),
            Arc::new(StringArray::from(vec![mask])),
            Arc::new(StringArray::from(vec!["CT"])),
            Arc::new(StringArray::from(vec!["GTV_Mass"])),
            Arc::new(Int32Array::from(vec![1])),
            Arc::new(StringArray::from(vec!["pyradiomics_original_v1"])),
        ],
    )
    .unwrap()
}

fn default_manifest_spec() -> serde_json::Value {
    serde_json::json!({"validate_paths": true})
}

#[tokio::test]
async fn manifest_validates_paths_and_stages_ordered_file_set() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("image.nii.gz"), b"image").unwrap();
    std::fs::write(workspace.path().join("mask.nii.gz"), b"mask").unwrap();
    let image = workspace
        .path()
        .join("image.nii.gz")
        .to_string_lossy()
        .into_owned();
    let mask = workspace
        .path()
        .join("mask.nii.gz")
        .to_string_lossy()
        .into_owned();
    let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);

    let mut manifest = RadiomicsManifestNodeFactory
        .build(default_manifest_spec(), ctx.clone())
        .unwrap();
    let df = ctx
        .session()
        .read_batch(input_batch(&image, &mask))
        .unwrap();
    let outputs = manifest
        .execute(
            &ctx,
            &[NodeInput::new_dataframe(0, df)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
    let manifest_rows = outputs
        .dataframe(0)
        .unwrap()
        .clone()
        .collect()
        .await
        .unwrap();
    assert_eq!(manifest_rows[0].num_rows(), 1);
    assert_eq!(
        manifest_rows[0]
            .column_by_name("status")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(0),
        "valid"
    );

    let mut stage = RadiomicsStageFileSetNodeFactory
        .build(serde_json::json!({"column":"mask_uri"}), ctx.clone())
        .unwrap();
    let manifest_df = outputs.dataframe(0).unwrap().clone();
    let staged = stage
        .execute(
            &ctx,
            &[NodeInput::new_dataframe(0, manifest_df)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
    let files = staged.get(&0).unwrap().as_file_set().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, mask);
}

#[tokio::test]
async fn container_wrapper_accepts_multiple_upstream_file_edges() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("image.mha"), b"image").unwrap();
    std::fs::write(workspace.path().join("mask.mha"), b"mask").unwrap();
    let ctx = workspace_ctx(workspace.path());
    let runtime = Arc::new(CopyPairsRuntime::new(workspace.path()));
    let panel_cache = Arc::new(PanelCache::new(workspace.path().join("panels")));
    let node = RadiomicsContainerNodeFactory::pair_validate(runtime.clone(), panel_cache)
        .build(
            serde_json::json!({"extraction_id":"sts001_ct","mask_label":1,"minimum_mask_voxels":1}),
            ctx.clone(),
        )
        .unwrap();

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "image".into(),
        Box::new(FileReferenceNode::new(
            workspace
                .path()
                .join("image.mha")
                .to_string_lossy()
                .into_owned(),
            Some("mha".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "mask".into(),
        Box::new(FileReferenceNode::new(
            workspace
                .path()
                .join("mask.mha")
                .to_string_lossy()
                .into_owned(),
            Some("mha".into()),
        )),
    )
    .unwrap();
    dag.add_node("validate".into(), node).unwrap();
    dag.add_edge("image", "validate", 0, 0).unwrap();
    dag.add_edge("mask", "validate", 0, 1).unwrap();

    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("validate"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    let outputs = dag.output("validate").unwrap();
    assert!(
        outputs
            .get(&0)
            .unwrap()
            .as_file()
            .unwrap()
            .path
            .contains("pair_validation.parquet")
    );
    assert!(
        outputs
            .get(&1)
            .unwrap()
            .as_file()
            .unwrap()
            .path
            .contains("geometry.json")
    );

    let requests = runtime.requests.lock().unwrap();
    let request = &requests[0];
    let input0 = request
        .env
        .iter()
        .find(|(name, _)| name == "AUTONOMICS_INPUT0")
        .map(|(_, value)| value.clone())
        .unwrap();
    let input1 = request
        .env
        .iter()
        .find(|(name, _)| name == "AUTONOMICS_INPUT1")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert_ne!(input0, input1);
    let snapshot = runtime.input_contents.lock().unwrap();
    let staged_content = |container_path: &str| {
        snapshot
            .iter()
            .find(|(path, _)| path == container_path)
            .map(|(_, bytes)| bytes.as_slice())
            .unwrap()
    };
    assert_eq!(staged_content(&input0), b"image".as_slice());
    assert_eq!(staged_content(&input1), b"mask".as_slice());
}

#[tokio::test]
async fn mask_ingest_documents_and_stages_port_order() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("image.mha"), b"reference-image").unwrap();
    std::fs::write(workspace.path().join("rtstruct.dcm"), b"rtstruct").unwrap();
    let ctx = workspace_ctx(workspace.path());
    let runtime = Arc::new(CopyPairsRuntime::new(workspace.path()));
    let panel_cache = Arc::new(PanelCache::new(workspace.path().join("panels")));
    let factory = RadiomicsContainerNodeFactory::mask_ingest(runtime.clone(), panel_cache);
    let node = factory
        .build(serde_json::json!({"roi_name":"GTV_Mass"}), ctx.clone())
        .unwrap();
    let ports = node.ports();
    assert_eq!(
        ports.input_port(0).unwrap().label.as_deref(),
        Some("reference_image")
    );
    assert_eq!(
        ports.input_port(1).unwrap().label.as_deref(),
        Some("mask_or_rtstruct")
    );

    let mut dag = dag_core::dag::DAG::default();
    dag.add_node(
        "image".into(),
        Box::new(FileReferenceNode::new(
            workspace
                .path()
                .join("image.mha")
                .to_string_lossy()
                .into_owned(),
            Some("mha".into()),
        )),
    )
    .unwrap();
    dag.add_node(
        "rtstruct".into(),
        Box::new(FileReferenceNode::new(
            workspace
                .path()
                .join("rtstruct.dcm")
                .to_string_lossy()
                .into_owned(),
            Some("dicom".into()),
        )),
    )
    .unwrap();
    dag.add_node("mask_ingest".into(), node).unwrap();
    dag.add_edge("image", "mask_ingest", 0, 0).unwrap();
    dag.add_edge("rtstruct", "mask_ingest", 0, 1).unwrap();

    let report = dag
        .run(&SchedulerConfig::default(), &ctx, None)
        .await
        .unwrap();
    assert_eq!(
        report.statuses.get("mask_ingest"),
        Some(&dag_core::dag::RuntimeStatus::Success)
    );
    let requests = runtime.requests.lock().unwrap();
    let request = requests
        .iter()
        .find(|request| {
            request.env.iter().any(|(name, value)| {
                name == "RADIOMICS_MASK_SETTINGS" && value.contains("GTV_Mass")
            })
        })
        .expect("mask ingestion request");
    let input = |index: usize| {
        request
            .env
            .iter()
            .find(|(name, _)| *name == format!("AUTONOMICS_INPUT{index}"))
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert!(
        input(0).ends_with("input-0.mha"),
        "AUTONOMICS_INPUT0={}; AUTONOMICS_INPUT1={}",
        input(0),
        input(1)
    );
    assert!(input(1).ends_with("input-1.dcm"));
}

#[tokio::test]
async fn dcm_glob_reads_mounted_vfs_files() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("000001.dcm"), b"one").unwrap();
    std::fs::write(workspace.path().join("000002.dcm"), b"two").unwrap();
    let ctx = workspace_ctx(workspace.path());
    let mut node = RadiomicsDcmGlobNodeFactory
        .build(
            serde_json::json!({"pattern":"vfs:///000*.dcm"}),
            ctx.clone(),
        )
        .unwrap();
    let outputs = node
        .execute(&ctx, &[], &dag_core::dag::node_event::NodeReporter::noop())
        .await
        .unwrap();
    let files = outputs.get(&0).unwrap().as_file_set().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "vfs:///000001.dcm");
    assert_eq!(files[1].path, "vfs:///000002.dcm");
}

#[tokio::test]
async fn qc_and_assemble_produce_stage_b_contract() {
    let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("extraction_id", DataType::Utf8, false),
            Field::new("patient_id", DataType::Utf8, false),
            Field::new("original_firstorder_mean", DataType::Float64, false),
            Field::new("original_shape_voxelvolume", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["case1", "case2"])),
            Arc::new(StringArray::from(vec!["patient1", "patient1"])),
            Arc::new(Float64Array::from(vec![1.0, 3.0])),
            Arc::new(Float64Array::from(vec![10.0, 12.0])),
        ],
    )
    .unwrap();
    let df = ctx.session().read_batch(batch).unwrap();

    let mut qc = nodes_io::radiomics::RadiomicsQcNodeFactory
        .build(serde_json::json!({}), ctx.clone())
        .unwrap();
    let qc_outputs = qc
        .execute(
            &ctx,
            &[NodeInput::new_dataframe(0, df)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
    let filtered = qc_outputs.dataframe(2).unwrap().clone();

    let mut assemble = nodes_io::radiomics::RadiomicsFeatureSetAssembleNodeFactory
        .build(
            serde_json::json!({"feature_set_id":"ibsi_smoke","feature_set_version":"v1"}),
            ctx.clone(),
        )
        .unwrap();
    let assembled = assemble
        .execute(
            &ctx,
            &[NodeInput::new_dataframe(0, filtered)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
    let rows = assembled
        .dataframe(0)
        .unwrap()
        .clone()
        .collect()
        .await
        .unwrap();
    let metadata = assembled
        .dataframe(1)
        .unwrap()
        .clone()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows[0].num_rows(), 2);
    assert_eq!(metadata[0].num_rows(), 2);
}
