//! Binds an existing file to a [`FileRef`] without parsing its payload.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileFingerprint, FileRef, PortType};

pub const FILE_REFERENCE_KIND: &str = "file_reference";

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct FileReferenceNodeSpec {
    /// Concrete file path. Use `vfs://...` for runtime-mounted object
    /// storage, or an absolute path for a local development input.
    pub path: String,
    /// Optional format label passed downstream, for example `sumstats_gz`.
    pub format: Option<String>,
}

pub struct FileReferenceNode {
    ports: NodePorts,
    path: String,
    format: Option<String>,
}

impl FileReferenceNode {
    pub fn new(path: impl Into<String>, format: Option<String>) -> Self {
        Self {
            ports: port_layout(format.as_deref()),
            path: path.into(),
            format,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

fn port_layout(format: Option<&str>) -> NodePorts {
    match format {
        Some(format) => NodePorts::new().add_output_port_of_type_with_label_and_format(
            None,
            PortType::File,
            "file",
            format,
        ),
        None => NodePorts::new().add_output_port_of_type(None, PortType::File),
    }
}

#[async_trait]
impl DagNode for FileReferenceNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let file = resolve_file(ctx, &self.path, self.format.clone()).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            path: self.path.clone(),
            format: self.format.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        FILE_REFERENCE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

async fn resolve_file(
    ctx: &NodeCtx,
    path: &str,
    format: Option<String>,
) -> Result<FileRef, DagError> {
    if path.trim().is_empty() {
        return Err(DagError::Schedule("file path cannot be empty".into()));
    }

    if path.starts_with("vfs://") {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(format!("file `{path}` requires a registered runtime VFS"))
        })?;
        let virtual_path = path
            .strip_prefix("vfs://")
            .expect("path starts with vfs://");
        return vfs_file(storage, path, virtual_path, format).await;
    }

    if let Some(storage) = ctx.opendal.as_ref()
        && let Some(file) = vfs_file(storage, path, path, format.clone()).await.ok()
    {
        return Ok(file);
    }

    let local_path = local_path(path)?;
    let file = FileRef::local(local_path, format).map_err(|error| {
        DagError::Schedule(format!(
            "file_reference cannot resolve `{path}` through VFS or the local host: {error}"
        ))
    })?;
    Ok(file)
}

async fn vfs_file(
    storage: &vfs::OpendalFileStorage,
    output_path: &str,
    virtual_path: &str,
    format: Option<String>,
) -> Result<FileRef, DagError> {
    let operator = storage.resolve(virtual_path).clone();
    let key = storage.resolve_path(virtual_path);
    let metadata = operator.stat(&key).await.map_err(|error| {
        DagError::Schedule(format!("cannot stat file `{output_path}`: {error}"))
    })?;
    if metadata.is_dir() {
        return Err(DagError::Schedule(format!(
            "file_reference path is a directory, not a file: `{output_path}`"
        )));
    }

    let mtime_ns = metadata
        .last_modified()
        .and_then(|timestamp| {
            std::time::SystemTime::from(timestamp)
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
        })
        .map(|duration| duration.as_nanos() as i128)
        .unwrap_or_default();

    Ok(FileRef {
        path: output_path.to_string(),
        format,
        fingerprint: Some(FileFingerprint {
            size: metadata.content_length(),
            mtime_ns,
            content_hash: metadata.etag().map(str::to_string),
        }),
    })
}

fn local_path(path: &str) -> Result<&std::path::Path, DagError> {
    let local = path
        .strip_prefix("file://")
        .unwrap_or(path)
        .trim_start_matches("//");
    let path = std::path::Path::new(local);
    if !path.is_absolute() {
        return Err(DagError::Schedule(format!(
            "file_reference path must be a `vfs://` URI or absolute path, got `{}`",
            path.display()
        )));
    }
    Ok(path)
}

pub struct FileReferenceNodeFactory {}

impl NodeFactory for FileReferenceNodeFactory {
    fn kind(&self) -> &'static str {
        FILE_REFERENCE_KIND
    }

    fn desc(&self) -> &'static str {
        "Exposes an existing VFS or local file as a FileRef without parsing it."
    }

    fn doc(&self) -> &'static str {
        "A file reference for binary or already-normalized inputs. It validates \
        that the configured `vfs://` or absolute path names a concrete file, \
        attaches size/mtime metadata, and emits a FileRef. Unlike \
        `file_to_dataframe`, it never reads the payload into a DataFrame. This is \
        the intended input node for file-backed dedicated container nodes such as \
        `ldsc_h2_container`. Set `format` whenever it is known; downstream ports \
        can reject mismatched files before execution."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FileReferenceNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: FileReferenceNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(FileReferenceNode::new(spec.path, spec.format)))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: FileReferenceNodeSpec = serde_json::from_value(spec)?;
        Ok(port_layout(spec.format.as_deref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dag_core::dag::DAG;
    use dag_core::dag::runtime::SchedulerConfig;
    use datafusion::prelude::SessionContext;
    use std::sync::Arc;
    use vfs::{
        BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
        VfsManifest,
    };

    fn mounted_ctx(root: &std::path::Path) -> (Arc<OpendalFileStorage>, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "workspace".into(),
                config: BackendConfig::local("/"),
            }],
            mount: vec![MountDefinition {
                path: "/".into(),
                backend: "workspace".into(),
                source: root.to_string_lossy().into_owned(),
                read_only: true,
            }],
        };
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(workspace.path(), mounted));
        (storage, workspace)
    }

    #[tokio::test]
    async fn emits_vfs_file_without_parsing_it() {
        let input = tempfile::tempdir().unwrap();
        std::fs::write(input.path().join("input.sumstats.gz"), b"binary-payload").unwrap();
        let (storage, _workspace) = mounted_ctx(input.path());
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), Some(storage));
        let mut node =
            FileReferenceNode::new("vfs:///input.sumstats.gz", Some("sumstats_gz".into()));

        let outputs = node
            .execute(&ctx, &[], &dag_core::dag::node_event::NodeReporter::noop())
            .await
            .unwrap();

        let file = outputs.get(&0).unwrap().as_file().unwrap();
        assert_eq!(file.path, "vfs:///input.sumstats.gz");
        assert_eq!(file.format.as_deref(), Some("sumstats_gz"));
        let fingerprint = file.fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, b"binary-payload".len() as u64);
    }

    #[test]
    fn resolves_spec_format_into_port_contract() {
        let ports = FileReferenceNodeFactory {}
            .ports_for_spec(serde_json::json!({
                "path": "/workspace/data/munged.sumstats.gz",
                "format": "sumstats_gz"
            }))
            .unwrap();
        let output = ports.output_port(0).unwrap();
        assert_eq!(output.label.as_deref(), Some("file"));
        assert_eq!(output.format.as_deref(), Some("sumstats_gz"));
    }

    #[tokio::test]
    async fn missing_vfs_file_fails() {
        let input = tempfile::tempdir().unwrap();
        let (storage, _workspace) = mounted_ctx(input.path());
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), Some(storage));
        let mut node = FileReferenceNode::new("vfs:///missing.gz", None);

        let error = node
            .execute(&ctx, &[], &dag_core::dag::node_event::NodeReporter::noop())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("cannot stat file"));
    }

    #[tokio::test]
    async fn local_file_flows_through_dag() {
        let input = tempfile::tempdir().unwrap();
        let path = input.path().join("input.txt");
        std::fs::write(&path, b"local-payload").unwrap();
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let mut dag = DAG::default();
        dag.add_node(
            "source".into(),
            Box::new(FileReferenceNode::new(
                path.to_string_lossy().into_owned(),
                Some("txt".into()),
            )),
        )
        .unwrap();

        dag.run(&SchedulerConfig::default(), &ctx, None)
            .await
            .unwrap();

        let output = dag.output("source").unwrap().clone();
        let file = output.get(&0).unwrap().as_file().unwrap();
        assert_eq!(
            std::path::Path::new(&file.path),
            std::path::Path::new(&path)
        );
        assert_eq!(file.fingerprint.as_ref().unwrap().size, 13);
    }
}
