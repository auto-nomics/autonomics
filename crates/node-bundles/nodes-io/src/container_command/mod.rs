//! File-to-file container command node: the spec surface.
//!
//! The module owns the declarative [`ContainerCommandSpec`] contract, its
//! validation, and the factory agents would use. Execution lives in
//! [`node`], errors in [`error`], and pure helpers in [`utils`].

mod error;
mod node;
mod utils;

pub use error::ContainerCommandError;
pub use node::ContainerCommandNode;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::node::{DagNode, NodePorts};
use dag_core::value::PortType;
use dag_core::{DataBundle, DataBundleBinding};
use dag_core::{NodeCtx, NodeFactory};

use container_runtime::{
    ContainerNetwork, DEFAULT_CONTAINER_WORKDIR, DEFAULT_TIMEOUT_SECS, PanelCache, PanelRef,
    PodmanConnection, PullPolicy,
};

use utils::validate_workspace_relative_path;

pub const CONTAINER_COMMAND_KIND: &str = "container_command";

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ContainerCommandOutputSpec {
    /// Safe path relative to `/work`. Absolute paths and `..` are rejected.
    pub path: String,
    /// Optional format label passed downstream (`bam`, `vcf`, `csv`, ...).
    pub format: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ContainerCommandSpec {
    /// OCI image reference. Prefer immutable digests for production workflows.
    pub image: String,
    /// argv passed to the image. No shell is inserted.
    pub command: Vec<String>,
    /// Optional inline script. When set, its `/work/.autonomics/script` path is
    /// inserted immediately after `command[0]` (the interpreter).
    #[serde(default)]
    pub script: Option<String>,
    /// Inline text files materialized under `/work/.autonomics/files`.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// Extra environment variables inside the workload container.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Files that must exist after a successful container run.
    pub outputs: Vec<ContainerCommandOutputSpec>,
    /// Optional persistent scratch directory. A unique scratch directory is
    /// created when omitted.
    pub workdir: Option<String>,
    /// VFS prefix used to publish declared outputs as immutable artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Immutable reference-data bundles materialized into the shared panel cache.
    #[serde(default)]
    pub panels: Vec<PanelRef>,
    /// Catalog-backed panels resolved through the existing DataBundle registry.
    /// This path is used by containerized tooling without exposing object keys in DAG specs.
    #[serde(default)]
    pub panel_bundles: Vec<ContainerPanelBundleSpec>,
    /// Network profile of the ephemeral container. The default is `isolated`
    /// (no network devices); `egress` must be justified by the tool. The
    /// legacy spelling `none` is accepted as `isolated`.
    #[serde(default = "default_network")]
    pub network: String,
    /// Whether the container root filesystem is read-only. `/work` remains
    /// writable, and the runtime supplies tmpfs mounts for `/tmp` and
    /// `/dev/shm`.
    #[serde(default = "default_true")]
    pub read_only_rootfs: bool,
    #[serde(default)]
    pub pull_policy: PullPolicy,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
    #[serde(default)]
    pub shm_size: Option<String>,
    /// GPU passthrough for images that need accelerators: `all`, a positive
    /// device count, or `device=<comma-separated indices or UUIDs>`. Absent
    /// means no GPU is visible to the container. The host needs the
    /// nvidia-container-toolkit CDI spec for rootless Podman.
    #[serde(default)]
    pub gpus: Option<String>,
    /// Advanced override for images that must run as an internal user. The
    /// default runs as the control process uid/gid while enforcing non-root.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ContainerPanelBundleSpec {
    pub panel_id: String,
    pub mount_path: String,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_network() -> String {
    "isolated".into()
}

fn default_artifact_prefix() -> String {
    "/artifacts/container-command".into()
}

fn default_true() -> bool {
    true
}

pub(crate) fn validate(spec: &ContainerCommandSpec) -> Result<(), ContainerCommandError> {
    if spec.image.trim().is_empty() {
        return Err(ContainerCommandError::Invalid(
            "`image` cannot be empty".into(),
        ));
    }
    if spec.command.is_empty() {
        return Err(ContainerCommandError::Invalid(
            "`command` cannot be empty".into(),
        ));
    }
    if spec.timeout_secs == 0 {
        return Err(ContainerCommandError::Invalid(
            "`timeout_secs` must be greater than zero".into(),
        ));
    }
    if spec.outputs.is_empty() {
        return Err(ContainerCommandError::Invalid(
            "at least one output must be declared".into(),
        ));
    }
    if let Err(error) = ContainerNetwork::parse(&spec.network) {
        return Err(ContainerCommandError::Invalid(error));
    }
    for output in &spec.outputs {
        validate_workspace_relative_path(&output.path).map_err(|e| {
            ContainerCommandError::Invalid(format!("invalid output `{}`: {e}", output.path))
        })?;
        if let Some(format) = &output.format
            && format.contains('\0')
        {
            return Err(ContainerCommandError::Invalid(format!(
                "output format `{format}` cannot contain NUL bytes"
            )));
        }
    }
    if let Some(script) = &spec.script
        && script.contains('\0')
    {
        return Err(ContainerCommandError::Invalid(
            "`script` cannot contain NUL bytes".into(),
        ));
    }
    for (path, content) in &spec.files {
        validate_workspace_relative_path(path).map_err(ContainerCommandError::Invalid)?;
        if content.contains('\0') {
            return Err(ContainerCommandError::Invalid(format!(
                "file `{path}` cannot contain NUL bytes"
            )));
        }
    }
    for (name, value) in &spec.env {
        if name.is_empty() || name.contains('=') || name.contains('\0') || value.contains('\0') {
            return Err(ContainerCommandError::Invalid(format!(
                "invalid environment variable `{name}`"
            )));
        }
        let reserved = name == "AUTONOMICS_WORKDIR"
            || name == "AUTONOMICS_SCRIPT"
            || name == "AUTONOMICS_FILES_DIR"
            || name.starts_with("AUTONOMICS_INPUT")
            || name.starts_with("AUTONOMICS_OUTPUT");
        if reserved {
            return Err(ContainerCommandError::Invalid(format!(
                "environment variable `{name}` is reserved by container_command"
            )));
        }
    }
    if let Some(cpus) = spec.cpus
        && cpus <= 0.0
    {
        return Err(ContainerCommandError::Invalid(
            "`cpus` must be positive".into(),
        ));
    }
    if let Some(pids_limit) = spec.pids_limit
        && pids_limit <= 0
    {
        return Err(ContainerCommandError::Invalid(
            "`pids_limit` must be positive".into(),
        ));
    }
    let mut panel_mounts = std::collections::BTreeSet::new();
    for panel in &spec.panels {
        panel.validate().map_err(ContainerCommandError::Invalid)?;
        if panel.mount_path == DEFAULT_CONTAINER_WORKDIR
            || !panel_mounts.insert(panel.mount_path.clone())
        {
            return Err(ContainerCommandError::Invalid(format!(
                "duplicate or invalid panel mount path `{}`",
                panel.mount_path
            )));
        }
    }
    for panel in &spec.panel_bundles {
        if panel.mount_path == DEFAULT_CONTAINER_WORKDIR
            || !Path::new(&panel.mount_path).is_absolute()
            || !panel_mounts.insert(panel.mount_path.clone())
        {
            return Err(ContainerCommandError::Invalid(format!(
                "duplicate or invalid catalog panel mount path `{}`",
                panel.mount_path
            )));
        }
    }
    Ok(())
}

pub struct ContainerCommandNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl Default for ContainerCommandNodeFactory {
    fn default() -> Self {
        let infra = container_runtime::ContainerExecutionInfra::from_env();
        Self {
            runtime: infra.runtime,
            panel_cache: infra.panel_cache,
        }
    }
}

impl NodeFactory for ContainerCommandNodeFactory {
    fn kind(&self) -> &'static str {
        CONTAINER_COMMAND_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs a file-to-file external command in an ephemeral OCI container."
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let node_spec: ContainerCommandSpec = serde_json::from_value(spec)?;
        Ok(node_spec
            .panel_bundles
            .iter()
            .enumerate()
            .map(|(index, panel)| {
                DataBundleBinding::new(format!("panel-{index}"), panel.panel_id.clone())
            })
            .collect())
    }

    fn doc(&self) -> &'static str {
        "Runs an OCI image without a shell. File inputs are staged into a private \
        workspace directory mounted at `/work`; input paths are exposed as \
        `$input0`, `$input1`, `AUTONOMICS_INPUT0`, and so on. Outputs must be \
        safe paths relative to `/work` and are exposed as `$output0`, \
        `AUTONOMICS_OUTPUT0`, and so on. The container root filesystem is \
        read-only by default, networking defaults to an isolated profile, and \
        immutable `panels` are mounted read-only from the shared panel cache. \
        Declared outputs are uploaded to VFS object storage with sha256 \
        fingerprints. Production workflows should reference images by digest."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ContainerCommandSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .set_fixed_input(false)
            .add_optional_input_port_of_type(PortType::File)
            .add_output_port_of_type(None, PortType::File)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: ContainerCommandSpec = serde_json::from_value(spec)?;
        let panel_bundles = node_spec
            .panel_bundles
            .iter()
            .enumerate()
            .map(|(index, _)| {
                node_ctx
                    .bound_data_bundle(&format!("panel-{index}"))
                    .cloned()
            })
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let node = ContainerCommandNode::new_with_catalog_panels(
            CONTAINER_COMMAND_KIND,
            node_spec,
            Arc::clone(&self.runtime) as Arc<dyn PodmanConnection>,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(node))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let node_spec: ContainerCommandSpec = serde_json::from_value(spec)?;
        validate(&node_spec)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        let mut ports = NodePorts::new()
            .set_fixed_input(false)
            .add_optional_input_port_of_type(PortType::File);
        for _ in &node_spec.outputs {
            ports = ports.add_output_port_of_type(None, PortType::File);
        }
        Ok(ports)
    }
}

#[cfg(test)]
mod tests {
    use super::node::stage_inputs;
    use super::utils::{container_path, hex, staged_path};
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use container_runtime::{
        ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, PodmanConfig, PodmanRuntime,
    };
    use dag_core::node::NodeInput;
    use dag_core::value::{FileRef, NodeValue};
    use sha2::{Digest, Sha256};
    use vfs::OpendalFileStorage;

    struct TestEnv {
        workspace: tempfile::TempDir,
        #[allow(dead_code)]
        objects: tempfile::TempDir,
        ctx: NodeCtx,
    }

    fn test_env() -> TestEnv {
        let workspace = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        let storage = Arc::new(OpendalFileStorage::new(objects.path()));
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage),
        );
        TestEnv {
            workspace,
            objects,
            ctx,
        }
    }

    fn input_file(path: &Path) -> NodeInput {
        NodeInput::file(0, FileRef::local(path, Some("txt".into())).unwrap())
    }

    #[derive(Default)]
    struct FakeRuntime {
        workspace_root: Option<PathBuf>,
        requests: Mutex<Vec<ContainerRunRequest>>,
    }

    impl FakeRuntime {
        fn new(workspace_root: &Path) -> Self {
            Self {
                workspace_root: Some(workspace_root.to_path_buf()),
                ..Default::default()
            }
        }
    }

    fn host_path(request: &ContainerRunRequest, container_path: &str) -> PathBuf {
        let relative = Path::new(container_path)
            .strip_prefix(DEFAULT_CONTAINER_WORKDIR)
            .expect("path is inside /work");
        request.workspace.host_path.join(relative)
    }

    #[async_trait]
    impl PodmanConnection for FakeRuntime {
        async fn run(
            &self,
            request: ContainerRunRequest,
        ) -> Result<ContainerRunResult, ContainerRuntimeError> {
            let output = request
                .env
                .iter()
                .find(|(key, _)| key == "AUTONOMICS_OUTPUT0")
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ContainerRuntimeError::Invalid("missing output binding".into()))?;
            let input = request
                .env
                .iter()
                .find(|(key, _)| key == "AUTONOMICS_INPUT0")
                .map(|(_, value)| value.clone());
            let host_output = host_path(&request, &output);
            if let Some(input) = input {
                let host_input = host_path(&request, &input);
                std::fs::copy(host_input, host_output).unwrap();
            } else {
                std::fs::write(host_output, "container-result").unwrap();
            }
            self.requests.lock().unwrap().push(request);
            Ok(ContainerRunResult {
                exit_code: 0,
                stdout: "container stdout".into(),
                stderr: String::new(),
            })
        }

        fn name(&self) -> &'static str {
            "fake"
        }

        fn workspace_root(&self) -> &Path {
            self.workspace_root
                .as_deref()
                .unwrap_or_else(|| Path::new("/tmp"))
        }
    }

    /// Helper function for creating container node spec
    fn spec(image: &str, command: Vec<String>, output: &str) -> ContainerCommandSpec {
        ContainerCommandSpec {
            image: image.into(),
            command,
            script: None,
            files: BTreeMap::new(),
            env: BTreeMap::new(),
            outputs: vec![ContainerCommandOutputSpec {
                path: output.into(),
                format: Some("txt".into()),
            }],
            workdir: None,
            artifact_prefix: "/artifacts/test".into(),
            timeout_secs: 10,
            panels: Vec::new(),
            panel_bundles: Vec::new(),
            network: default_network(),
            read_only_rootfs: true,
            pull_policy: PullPolicy::Missing,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            gpus: None,
            user: None,
        }
    }

    #[test]
    fn staged_path_preserves_compound_gzip_extensions() {
        let path = staged_path(
            Path::new("/tmp/stage"),
            "input",
            7,
            "/data/case/image.nii.gz",
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("input-7.nii.gz")
        );

        let path = staged_path(Path::new("/tmp/stage"), "input", 8, "/data/only.gz");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("input-8.gz")
        );
    }

    #[test]
    fn container_path_maps_every_file_set_member() {
        let workspace = Path::new("/host/workspace/run");
        let host_paths = format!(
            "{},{}",
            workspace.join("image-0.nii.gz").display(),
            workspace.join("image-1.nii.gz").display()
        );
        assert_eq!(
            container_path(workspace, &host_paths),
            format!(
                "{}/image-0.nii.gz,{}/image-1.nii.gz",
                DEFAULT_CONTAINER_WORKDIR, DEFAULT_CONTAINER_WORKDIR
            )
        );
    }

    #[tokio::test]
    async fn stage_inputs_assigns_input_ranges_by_declared_port() {
        let env = test_env();
        let source = tempfile::tempdir().unwrap();
        let image_one = source.path().join("image1.nii.gz");
        let image_two = source.path().join("image2.nii.gz");
        let mask_one = source.path().join("mask1.nii.gz");
        let manifest = source.path().join("manifest.csv");
        std::fs::write(&image_one, "image-one").unwrap();
        std::fs::write(&image_two, "image-two").unwrap();
        std::fs::write(&mask_one, "mask-one").unwrap();
        std::fs::write(&manifest, "manifest").unwrap();

        let image_file = |path: &Path| FileRef::local(path, Some("nifti_gz".into())).unwrap();
        let manifest_input =
            NodeInput::file(2, FileRef::local(&manifest, Some("csv".into())).unwrap());
        let image_input = NodeInput {
            port: 0,
            data: NodeValue::FileSet(vec![image_file(&image_one), image_file(&image_two)]),
        };
        let mask_input = NodeInput {
            port: 1,
            data: NodeValue::FileSet(vec![image_file(&mask_one)]),
        };

        let staged = stage_inputs(
            &env.ctx,
            env.workspace.path(),
            &[manifest_input, image_input, mask_input],
        )
        .await
        .unwrap();

        assert_eq!(
            staged.iter().map(|input| input.port).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let images = staged[0].data.as_file_set().unwrap();
        let masks = staged[1].data.as_file_set().unwrap();
        let staged_manifest = staged[2].data.as_file().unwrap();
        assert_eq!(
            images
                .iter()
                .map(|file| Path::new(&file.path).file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["input-0.nii.gz", "input-1.nii.gz"]
        );
        assert_eq!(
            Path::new(&masks[0].path)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("input-2.nii.gz")
        );
        assert_eq!(
            Path::new(&staged_manifest.path)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("input-3.csv")
        );
    }

    #[tokio::test]
    async fn stages_inputs_and_collects_declared_outputs() {
        let env = test_env();
        let dir = env.workspace.path().join("run");
        std::fs::create_dir_all(&dir).unwrap();
        let input = env.workspace.path().join("input.txt");
        std::fs::write(&input, "hello-container").unwrap();
        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut node_spec = spec(
            "quay.io/example/tool@sha256:abcdef",
            vec![
                "tool".into(),
                "--input".into(),
                "$input0".into(),
                "--output".into(),
                "$output0".into(),
            ],
            "result.txt",
        );
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        let mut node = ContainerCommandNode::new(
            "container_command",
            node_spec,
            runtime.clone(),
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
        )
        .unwrap();

        let outputs = node
            .execute(
                &env.ctx,
                &[input_file(&input)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let request = runtime.requests.lock().unwrap().last().unwrap().clone();
        assert!(request.command.contains(&"--input".into()));
        assert!(request.command.contains(&format!(
            "{DEFAULT_CONTAINER_WORKDIR}/.autonomics/inputs/input-0.txt"
        )));
        assert!(
            request
                .command
                .contains(&format!("{DEFAULT_CONTAINER_WORKDIR}/result.txt"))
        );
        assert_eq!(request.network, ContainerNetwork::Isolated);
        assert!(request.read_only_rootfs);
        assert!(
            request
                .workspace
                .host_path
                .ends_with(dir.file_name().unwrap())
        );

        let output = dir.join("result.txt");
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "hello-container");
        let file = outputs.get(&0).unwrap().as_file().unwrap();
        assert!(file.path.starts_with("vfs:///artifacts/test/"));
        let fingerprint = file.fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, "hello-container".len() as u64);
        assert!(
            fingerprint
                .content_hash
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        let object = env
            .ctx
            .opendal
            .as_ref()
            .unwrap()
            .resolve(file.path.strip_prefix("vfs://").unwrap())
            .read(
                &env.ctx
                    .opendal
                    .as_ref()
                    .unwrap()
                    .resolve_path(file.path.strip_prefix("vfs://").unwrap()),
            )
            .await
            .unwrap();
        assert_eq!(object.to_vec(), b"hello-container");
    }

    #[tokio::test]
    async fn script_mode_materializes_private_assets() {
        let env = test_env();
        let dir = env.workspace.path().join("script-run");
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut node_spec = spec(
            "quay.io/example/bash",
            vec!["bash".into(), "-e".into()],
            "script-result.txt",
        );
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        node_spec.script = Some("cat \"$AUTONOMICS_INPUT0\" > \"$AUTONOMICS_OUTPUT0\"".into());
        node_spec.files.insert("helper.txt".into(), "helper".into());
        let mut node = ContainerCommandNode::new(
            "container_command",
            node_spec,
            runtime.clone(),
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
        )
        .unwrap();

        node.execute(
            &env.ctx,
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        assert!(dir.join(".autonomics/script").is_file());
        assert!(dir.join(".autonomics/files/helper.txt").is_file());
        let requests = runtime.requests.lock().unwrap();
        assert!(
            requests
                .last()
                .unwrap()
                .command
                .contains(&format!("{DEFAULT_CONTAINER_WORKDIR}/.autonomics/script"))
        );
    }

    /// Serializes tests that mutate `AUTONOMICS_KEEP_WORKSPACE`, so a parallel
    /// run cannot observe the transient value and skip its own cleanup.
    /// Async-aware because the guarded section spans an `.await`.
    static GC_ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

    struct EnvReset(&'static str);
    impl Drop for EnvReset {
        fn drop(&mut self) {
            // SAFETY: process-global env mutation in a single-threaded test
            // section guarded by `GC_ENV_LOCK`.
            unsafe { std::env::remove_var(self.0) };
        }
    }

    #[tokio::test]
    async fn successful_unique_scratch_run_removes_workspace() {
        let _guard = GC_ENV_LOCK.lock().await;
        let env = test_env();
        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut node = ContainerCommandNode::new(
            "container_command",
            spec("quay.io/example/tool", vec!["tool".into()], "out.txt"),
            runtime,
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
        )
        .unwrap();

        node.execute(
            &env.ctx,
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(env.workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            leftovers.is_empty(),
            "unique scratch must be removed after success, found {leftovers:?}"
        );
    }

    #[tokio::test]
    async fn keep_workspace_env_retains_scratch_for_debugging() {
        let _guard = GC_ENV_LOCK.lock().await;
        let _reset = EnvReset(container_runtime::KEEP_WORKSPACE_ENV);
        // SAFETY: guarded by `GC_ENV_LOCK`; restored on drop.
        unsafe { std::env::set_var(container_runtime::KEEP_WORKSPACE_ENV, "1") };
        let env = test_env();
        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut node = ContainerCommandNode::new(
            "container_command",
            spec("quay.io/example/tool", vec!["tool".into()], "out.txt"),
            runtime,
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
        )
        .unwrap();

        node.execute(
            &env.ctx,
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let scratch: Vec<_> = std::fs::read_dir(env.workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(scratch.len(), 1, "exactly one scratch directory expected");
        assert!(scratch[0].join("out.txt").is_file());
    }

    #[test]
    fn rejects_output_path_escape() {
        let error = match ContainerCommandNode::new(
            "container_command",
            spec("tool", vec!["tool".into()], "../escape.txt"),
            Arc::new(FakeRuntime::default()),
            Arc::new(PanelCache::new("/tmp/autonomics-cache")),
        ) {
            Ok(_) => panic!("output path escape must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("safe relative path"));
    }

    #[test]
    fn factory_resolves_dynamic_output_ports() {
        let ports = ContainerCommandNodeFactory::default()
            .ports_for_spec(serde_json::json!({
                "image": "quay.io/example/tool:1",
                "command": ["tool"],
                "outputs": [
                    {"path": "one.txt", "format": "txt"},
                    {"path": "two.csv", "format": "csv"}
                ]
            }))
            .unwrap();

        assert_eq!(ports.output_ports().len(), 2);
    }

    #[tokio::test]
    async fn concurrent_container_nodes_publish_distinct_artifacts() {
        let env = test_env();
        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut nodes = Vec::new();
        for run in ["a", "b"] {
            let dir = env.workspace.path().join(run);
            std::fs::create_dir_all(&dir).unwrap();
            let mut node_spec = spec("tool", vec!["tool".into()], "result.txt");
            node_spec.workdir = Some(dir.to_string_lossy().into_owned());
            nodes.push(
                ContainerCommandNode::new(
                    "container_command",
                    node_spec,
                    runtime.clone(),
                    Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
                )
                .unwrap(),
            );
        }

        let mut second_node = nodes.pop().unwrap();
        let mut first_node = nodes.pop().unwrap();
        let first_reporter = dag_core::dag::node_event::NodeReporter::noop();
        let second_reporter = dag_core::dag::node_event::NodeReporter::noop();
        let (first, second) = tokio::join!(
            first_node.execute(&env.ctx, &[], &first_reporter,),
            second_node.execute(&env.ctx, &[], &second_reporter,)
        );
        first.unwrap();
        second.unwrap();

        assert_eq!(runtime.requests.lock().unwrap().len(), 2);
        let paths: Vec<_> = runtime
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.name.clone())
            .collect();
        assert_ne!(paths[0], paths[1]);
    }

    #[tokio::test]
    async fn catalog_panel_bundle_resolves_to_readonly_panel_ref() {
        let env = test_env();
        let panel_content = b"catalog-panel";
        let digest = format!("sha256:{}", hex(&Sha256::digest(panel_content)));
        let source = "/catalog/1000g_eur";
        let panel_root = env.objects.path().join("catalog/1000g_eur");
        std::fs::create_dir_all(panel_root.join("chr22")).unwrap();
        std::fs::write(panel_root.join("chr22/panel.txt"), panel_content).unwrap();
        std::fs::write(
            panel_root.join("manifest.json"),
            serde_json::to_vec(&container_runtime::PanelManifest {
                schema_version: 1,
                id: "1000g_eur".into(),
                version: "v3".into(),
                digest: format!("sha256:{}", "3".repeat(64)),
                files: vec![container_runtime::PanelFile {
                    path: "chr22/panel.txt".into(),
                    size: panel_content.len() as u64,
                    sha256: digest,
                }],
            })
            .unwrap(),
        )
        .unwrap();

        let mut bundle = DataBundle::new("1000g_eur", "1000G EUR", "/bundles/1000g_eur");
        bundle.source = Some(source.into());
        bundle.digest = Some(format!("sha256:{}", "3".repeat(64)));
        let dir = env.workspace.path().join("catalog-run");
        std::fs::create_dir_all(&dir).unwrap();
        let mut node_spec = spec("tool", vec!["tool".into()], "result.txt");
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        node_spec.panel_bundles = vec![ContainerPanelBundleSpec {
            panel_id: "1000g_eur".into(),
            mount_path: "/panels/1000g_eur".into(),
        }];

        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut node = ContainerCommandNode::new_with_catalog_panels(
            "container_command",
            node_spec,
            runtime.clone(),
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
            vec![bundle],
        )
        .unwrap();
        node.execute(
            &env.ctx,
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let request = runtime.requests.lock().unwrap().last().unwrap().clone();
        let panel = request.panels.first().unwrap();
        assert_eq!(panel.id, "1000g_eur");
        assert!(panel.host_path.to_string_lossy().contains("1000g_eur"));
        assert_eq!(panel.mount_path, "/panels/1000g_eur");
        assert!(
            runtime
                .requests
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .panels
                .iter()
                .all(|candidate| candidate
                    .host_path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains('@')))
        );
    }

    #[tokio::test]
    async fn missing_output_fails_after_successful_exit() {
        struct MissingOutputRuntime {
            workspace_root: PathBuf,
        }
        #[async_trait]
        impl PodmanConnection for MissingOutputRuntime {
            async fn run(
                &self,
                _request: ContainerRunRequest,
            ) -> Result<ContainerRunResult, ContainerRuntimeError> {
                Ok(ContainerRunResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
            fn workspace_root(&self) -> &Path {
                &self.workspace_root
            }
        }

        let env = test_env();
        let dir = env.workspace.path().join("missing");
        std::fs::create_dir_all(&dir).unwrap();
        let mut node_spec = spec("tool", vec!["tool".into()], "missing.txt");
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        // A wrapper-style kind must travel into the DagError instead of the
        // generic `container_command` marker.
        let mut node = ContainerCommandNode::new(
            "mtag_container",
            node_spec,
            Arc::new(MissingOutputRuntime {
                workspace_root: env.workspace.path().to_path_buf(),
            }),
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
        )
        .unwrap();
        let error = node
            .execute(
                &env.ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("missing.txt"));
        let dag_core::dag::DagError::NodeError { node_type, .. } = &error else {
            panic!("missing output must surface as a NodeError, got {error:?}");
        };
        assert_eq!(node_type, "mtag_container");
    }

    #[tokio::test]
    async fn failed_container_exit_is_reported_with_captured_output() {
        struct FailingRuntime {
            workspace_root: PathBuf,
        }
        #[async_trait]
        impl PodmanConnection for FailingRuntime {
            async fn run(
                &self,
                request: ContainerRunRequest,
            ) -> Result<ContainerRunResult, ContainerRuntimeError> {
                let output = request
                    .env
                    .iter()
                    .find(|(key, _)| key == "AUTONOMICS_OUTPUT0")
                    .map(|(_, value)| value.clone())
                    .expect("output binding");
                std::fs::write(
                    host_path(&request, &output),
                    format!(
                        "{}\nTraceback (most recent call last):\nValueError: malformed sumstats",
                        "diagnostic noise\n".repeat(100)
                    ),
                )
                .unwrap();
                Err(ContainerRuntimeError::ExitStatus {
                    exit_code: 42,
                    stderr: String::new(),
                    stdout: String::new(),
                })
            }
            fn workspace_root(&self) -> &Path {
                &self.workspace_root
            }
        }

        let env = test_env();
        let dir = env.workspace.path().join("failed");
        std::fs::create_dir_all(&dir).unwrap();
        let mut node_spec = spec("tool", vec!["tool".into()], "result.txt");
        // A persistent workspace makes this test independent of scratch naming.
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        let mut node = ContainerCommandNode::new(
            "container_command",
            node_spec,
            Arc::new(FailingRuntime {
                workspace_root: env.workspace.path().to_path_buf(),
            }),
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
        )
        .unwrap();

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);
        let reporter = dag_core::dag::node_event::NodeReporter::new("container", event_tx);
        let error = node.execute(&env.ctx, &[], &reporter).await.unwrap_err();
        let report = error.to_report();

        assert_eq!(report.kind, "node_error");
        assert!(report.message.contains("status 42"), "{}", report.message);
        assert!(
            report.message.contains("result.txt tail"),
            "{}",
            report.message
        );
        assert!(
            report.message.contains("ValueError: malformed sumstats"),
            "{}",
            report.message
        );
        assert!(
            !report.message.contains("no stdout or stderr captured"),
            "{}",
            report.message
        );
        assert!(
            event_rx.try_recv().is_ok_and(|event| matches!(
                event.kind,
                dag_core::dag::node_event::NodeEventKind::Log { .. }
            )),
            "container failure should also be emitted as a live node log"
        );
    }

    #[tokio::test]
    async fn container_publishes_logs_and_details_on_success() {
        let env = test_env();
        let dir = env.workspace.path().join("logs-ok");
        std::fs::create_dir_all(&dir).unwrap();
        let mut node_spec = spec(
            &format!("registry.example/test/tool@sha256:{}", "a".repeat(64)),
            vec!["tool".into()],
            "result.txt",
        );
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        let mut node = ContainerCommandNode::new(
            "container_command",
            node_spec,
            Arc::new(FakeRuntime::new(env.workspace.path())),
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
        )
        .unwrap();

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let reporter = dag_core::dag::node_event::NodeReporter::new("container", event_tx);
        node.execute(&env.ctx, &[], &reporter).await.unwrap();

        let details = reporter.take_run_details().expect("run details recorded");
        assert_eq!(details.exit_code, Some(0));
        assert_eq!(
            details.image_digest.as_deref(),
            Some(format!("sha256:{}", "a".repeat(64)).as_str())
        );
        let run_name = details.run_name.clone().unwrap();
        assert!(run_name.starts_with("autonomics-container-command-"));

        // FakeRuntime emits stdout and a blank stderr → exactly one log file.
        let stdout_log = details.stdout_log.expect("stdout persisted");
        assert!(
            stdout_log
                .path
                .ends_with(&format!("{run_name}/.autonomics-logs/stdout.log")),
            "unexpected log path {}",
            stdout_log.path
        );
        assert!(details.stderr_log.is_none(), "blank stderr must be skipped");

        // The persisted object is readable through the same VFS path and its
        // recorded fingerprint matches the content.
        let virtual_path = stdout_log.path.strip_prefix("vfs://").unwrap();
        let storage = env.ctx.opendal.as_ref().unwrap();
        let length = storage.content_length(virtual_path).await.unwrap();
        let stored = storage.read_range(virtual_path, 0..length).await.unwrap();
        assert_eq!(stored.to_vec(), b"container stdout");
        let fingerprint = stdout_log.fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, "container stdout".len() as u64);
        assert_eq!(
            fingerprint.content_hash.as_deref(),
            Some(format!("sha256:{}", hex(&Sha256::digest(b"container stdout"))).as_str())
        );
    }

    #[tokio::test]
    async fn container_failure_publishes_logs_and_details() {
        struct FailingWithOutputRuntime {
            workspace_root: PathBuf,
        }
        #[async_trait]
        impl PodmanConnection for FailingWithOutputRuntime {
            async fn run(
                &self,
                _request: ContainerRunRequest,
            ) -> Result<ContainerRunResult, ContainerRuntimeError> {
                Err(ContainerRuntimeError::ExitStatus {
                    exit_code: 42,
                    stderr: "boom".into(),
                    stdout: "partial output".into(),
                })
            }
            fn workspace_root(&self) -> &Path {
                &self.workspace_root
            }
        }

        let env = test_env();
        let dir = env.workspace.path().join("logs-failed");
        std::fs::create_dir_all(&dir).unwrap();
        let mut node_spec = spec("tool", vec!["tool".into()], "result.txt");
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        let mut node = ContainerCommandNode::new(
            "container_command",
            node_spec,
            Arc::new(FailingWithOutputRuntime {
                workspace_root: env.workspace.path().to_path_buf(),
            }),
            Arc::new(PanelCache::new(env.workspace.path().join("cache"))),
        )
        .unwrap();

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let reporter = dag_core::dag::node_event::NodeReporter::new("container", event_tx);
        let error = node.execute(&env.ctx, &[], &reporter).await.unwrap_err();
        assert!(error.to_string().contains("42"));

        // A failed execution is as auditable as a successful one: exit code,
        // both captured streams persisted, and the scratch failure-logs still
        // written for the diagnostic message.
        let details = reporter
            .take_run_details()
            .expect("failed run records details");
        assert_eq!(details.exit_code, Some(42));
        let stdout_log = details.stdout_log.expect("stdout persisted on failure");
        let stderr_log = details.stderr_log.expect("stderr persisted on failure");
        let storage = env.ctx.opendal.as_ref().unwrap();
        let stdout_path = stdout_log.path.strip_prefix("vfs://").unwrap();
        let stderr_path = stderr_log.path.strip_prefix("vfs://").unwrap();
        let stdout_len = storage.content_length(stdout_path).await.unwrap();
        let stderr_len = storage.content_length(stderr_path).await.unwrap();
        assert_eq!(
            storage
                .read_range(stdout_path, 0..stdout_len)
                .await
                .unwrap()
                .to_vec(),
            b"partial output"
        );
        assert_eq!(
            storage
                .read_range(stderr_path, 0..stderr_len)
                .await
                .unwrap()
                .to_vec(),
            b"boom"
        );
        assert!(
            dir.join(".autonomics")
                .join("failure-logs")
                .join("stdout.log")
                .exists()
        );
    }

    #[tokio::test]
    #[ignore = "requires a working rootless Podman runtime and may pull an OCI image"]
    async fn real_podman_copies_input_to_declared_output() {
        let image = std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
            .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into());
        let env = test_env();
        let state = tempfile::tempdir().unwrap();
        let workspace_root = state.path().join("workspace");
        let dir = workspace_root.join("podman-it");
        std::fs::create_dir_all(&dir).unwrap();
        let input = env.workspace.path().join("input.txt");
        std::fs::write(&input, "podman-container-command").unwrap();
        let mut node_spec = spec(
            &image,
            vec![
                "cp".into(),
                "--".into(),
                "$input0".into(),
                "$output0".into(),
            ],
            "result.txt",
        );
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        node_spec.timeout_secs = 120;
        let runtime = PodmanRuntime::new(PodmanConfig {
            program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
            workspace_root: workspace_root.clone(),
            panel_cache_root: state.path().join("panels"),
        });
        let mut node = ContainerCommandNode::new(
            "container_command",
            node_spec,
            Arc::new(runtime),
            Arc::new(PanelCache::new(state.path().join("panels"))),
        )
        .unwrap();

        let outputs = node
            .execute(
                &env.ctx,
                &[input_file(&input)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("result.txt")).unwrap(),
            "podman-container-command"
        );
        let file = outputs.get(&0).unwrap().as_file().unwrap();
        assert!(file.path.starts_with("vfs:///artifacts/test/"));
    }
}
