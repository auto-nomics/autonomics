//! End-to-end file-flow tests for `dataframe_to_file -> container_command -> file_to_dataframe`,
//! chained node-by-node (the node-graph scheduler is gone; flows are the
//! executable path).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, DEFAULT_CONTAINER_WORKDIR,
    PanelCache, PodmanConnection, PullPolicy, unique_container_name,
};
use dag_core::node::{DagNode, NodeInput};
use dag_core::registry::NodeCtx;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::SessionContext;

use nodes_io::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use nodes_io::dataframe_to_file::{DataFrameToFileNode, WriteFormat};
use nodes_io::file_to_dataframe::FileToDataFrameNode;
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

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

fn workspace_vfs(workspace_root: &Path) -> NodeCtx {
    let root = workspace_root.to_string_lossy().into_owned();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "workspace".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![
            MountDefinition {
                domain: None,
                path: "/".into(),
                backend: "workspace".into(),
                source: root.clone(),
                read_only: false,
                permissions: Default::default(),
            },
            // Identity mount for the workspace tree, mirroring the runtime
            // host: bare work-dir host paths resolve to the real files.
            MountDefinition {
                domain: None,
                path: root,
                backend: "workspace".into(),
                source: workspace_root.to_string_lossy().into_owned(),
                read_only: true,
                permissions: Default::default(),
            },
        ],
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
    NodeCtx::new(session.runtime_env(), Some(storage))
}

fn container_spec(
    image: &str,
    workspace: &Path,
    artifact_prefix: Option<String>,
) -> ContainerCommandSpec {
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
        tuple_inputs: Vec::new(),
        outputs: vec![ContainerCommandOutputSpec {
            path: "copied.csv".into(),
            format: Some("csv".into()),
            tuple: Vec::new(),
            optional: false,
        }],
        workdir: Some(workspace.to_string_lossy().into_owned()),
        stage_in_mode: Default::default(),
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
        gpus: None,
        user: None,
    }
}

async fn assert_container_artifact(
    ctx: &NodeCtx,
    output: &dag_core::value::FileRef,
    workspace_output: &Path,
) {
    // Nextflow-mode outputs are the work-dir files themselves, referenced
    // by local (mtime) fingerprints.
    assert_eq!(
        Path::new(&output.path),
        workspace_output,
        "container output must be the work-dir file"
    );
    assert_eq!(output.format.as_deref(), Some("csv"));

    let expected_bytes = tokio::fs::read(workspace_output)
        .await
        .expect("read workspace output copied by the container");
    let fingerprint = output
        .fingerprint
        .as_ref()
        .expect("container output has a fingerprint");
    assert_eq!(fingerprint.size, expected_bytes.len() as u64);
    assert!(fingerprint.content_hash.is_none());
    assert!(!fingerprint.immutable_remote);

    // The downstream proof: the bare work-dir host path resolves through
    // the identity workspace mount and returns the exact bytes — the same
    // routing FileToDataFrameNode uses.
    let storage = ctx.opendal.as_ref().expect("test VFS storage");
    let host_path = &output.path;
    let actual_bytes = storage
        .resolve(host_path)
        .read(&storage.resolve_path(host_path))
        .await
        .expect("read work-dir output through the VFS identity mount");
    assert_eq!(
        actual_bytes.to_vec(),
        expected_bytes,
        "downstream nodes must be able to read the exact container output"
    );
}

#[tokio::test]
async fn dataframe_to_file_output_flows_through_container_command() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = workspace_vfs(workspace.path());
    let input_path = format!("vfs:///input-{}.csv", unique_container_name());
    let run_root = workspace.path().join("fake-run");
    std::fs::create_dir_all(&run_root).unwrap();
    let runtime = Arc::new(FakeContainerRuntime::new(workspace.path()));
    let spec = container_spec(
        "localhost/container-file-flow-test:not-present",
        &run_root,
        None,
    );

    let reporter = dag_core::dag::node_event::NodeReporter::noop();

    // source -> write_input: dataframe lands in the VFS as a CSV FileRef.
    let mut write_input = DataFrameToFileNode::new(
        input_path,
        WriteFormat::Csv,
        nodes_io::dataframe_to_file::SinkMode::Overwrite,
    );
    let dataframe = ctx.session().read_batch(sample_batch()).unwrap();
    let written = write_input
        .execute(
            &ctx,
            &[NodeInput::new_dataframe(0, dataframe)],
            &reporter,
        )
        .await
        .unwrap();
    let input_file = written.get(&0).unwrap().as_file().unwrap().clone();

    // write_input -> container: the CSV is staged into the work dir, the
    // fake runtime copies it to the declared output.
    let mut container = ContainerCommandNode::new(
        "container_command",
        spec,
        runtime.clone(),
        Arc::new(PanelCache::new(workspace.path().join("panels"))),
    )
    .unwrap();
    let container_out = container
        .execute(&ctx, &[NodeInput::file(0, input_file)], &reporter)
        .await
        .unwrap();

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

    let container_output = container_out.get(&0).unwrap().as_file().unwrap().clone();
    assert_container_artifact(&ctx, &container_output, &run_root.join("copied.csv")).await;

    // container -> read_result: the work-dir output reads back as a dataframe.
    let mut read_result = FileToDataFrameNode::new(None, None);
    let outputs = read_result
        .execute(&ctx, &[NodeInput::file(0, container_output)], &reporter)
        .await
        .unwrap();
    let dataframe = outputs.get(&0).unwrap().as_dataframe().unwrap().clone();
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
