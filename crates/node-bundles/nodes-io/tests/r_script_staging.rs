use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow_array::{Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use container_runtime::{
    ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PanelCache, PodmanConnection,
};
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::NodeInput;
use dag_core::registry::NodeCtx;
use dag_core::registry::NodeFactory;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::SessionContext;
use nodes_io::script_nodes::ScriptNodeFactory;
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

#[derive(Default)]
struct CopyCsvRuntime {
    workspace_root: Option<PathBuf>,
    seen_inputs: Mutex<Vec<PathBuf>>,
}

impl CopyCsvRuntime {
    fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: Some(workspace_root.to_path_buf()),
            seen_inputs: Mutex::new(Vec::new()),
        }
    }

    fn host_path(&self, request: &ContainerRunRequest, container_path: &str) -> PathBuf {
        let relative = Path::new(container_path)
            .strip_prefix("/work")
            .expect("container path is inside /work");
        request.workspace.host_path.join(relative)
    }
}

#[async_trait]
impl PodmanConnection for CopyCsvRuntime {
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
        let input_host = self.host_path(&request, &input);
        let output_host = self.host_path(&request, &output);

        assert!(
            input_host.is_file(),
            "staged input is missing: {input_host:?}"
        );
        self.seen_inputs.lock().unwrap().push(input_host.clone());
        std::fs::copy(input_host, output_host)?;

        Ok(ContainerRunResult {
            exit_code: 0,
            stdout: "copied by fake R runtime".into(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "fake-r-script-staging"
    }

    fn workspace_root(&self) -> &Path {
        self.workspace_root
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"))
    }
}

#[tokio::test]
async fn legacy_r_script_stages_a_one_row_dataframe() {
    let root = tempfile::tempdir().unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "r-script-staging".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/".into(),
            backend: "r-script-staging".into(),
            source: root.path().to_string_lossy().into_owned(),
            read_only: false,
        }],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = Arc::new(OpendalFileStorage::with_mounts(
        root.path(),
        mounted.clone(),
    ));
    let session = SessionContext::new();
    session
        .runtime_env()
        .register_object_store(ObjectStoreUrl::parse("vfs://").unwrap().as_ref(), mounted);
    let ctx = NodeCtx::new(session.runtime_env(), Some(storage));
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("value", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1])),
            Arc::new(Float64Array::from(vec![2.5])),
        ],
    )
    .unwrap();
    let input = session.read_batch(batch).unwrap();
    let runtime = Arc::new(CopyCsvRuntime::new(root.path()));
    let mut node = ScriptNodeFactory::r(
        runtime,
        Arc::new(PanelCache::new(root.path().join("panels"))),
    )
    .build(
        serde_json::json!({
            "code": "output_table <- input_table",
            "artifact_prefix": "/artifacts/r-script-staging-test",
            "timeout_s": 60
        }),
        ctx.clone(),
    )
    .unwrap();

    let outputs = node
        .execute(
            &ctx,
            &[NodeInput::new_dataframe(0, input)],
            &NodeReporter::noop(),
        )
        .await
        .unwrap();

    let output = outputs.dataframe(0).unwrap();
    assert_eq!(output.clone().count().await.unwrap(), 1);
}
