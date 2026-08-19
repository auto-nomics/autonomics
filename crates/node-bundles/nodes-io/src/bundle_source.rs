//! Logical data-bundle source node.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, DataBundle, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileFingerprint, FileRef, PortType};

/// Expose one logical [`DataBundle`] as a file-valued DAG output.
///
/// The node resolves and validates the bundle, but does not download it. Its
/// output path remains a VFS virtual path for consumers that can read through
/// the runtime storage layer.
pub struct BundleSourceNode {
    ports: NodePorts,
    bundle: DataBundle,
    format: Option<String>,
}

impl BundleSourceNode {
    pub fn new(bundle: DataBundle, format: Option<String>) -> Self {
        Self {
            ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
            bundle,
            format,
        }
    }

    pub fn bundle(&self) -> &DataBundle {
        &self.bundle
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct BundleSourceNodeSpec {
    pub ident: String,
    pub desc: String,
    /// Runtime VFS path, for example `/bundles/panels/panel.parquet`.
    pub vpath: String,
    pub format: Option<String>,
}

pub struct BundleSourceNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type(None, PortType::File)
}

#[async_trait]
impl DagNode for BundleSourceNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule("bundle source requires a registered runtime VFS".to_string())
        })?;
        let resolved = self.bundle.resolve(storage);
        let metadata = resolved
            .operator
            .stat(&resolved.key)
            .await
            .map_err(|error| {
                DagError::Schedule(format!(
                    "cannot stat bundle `{}` at `{}`: {error}",
                    self.bundle.ident, self.bundle.vpath
                ))
            })?;

        if metadata.is_dir() {
            return Err(DagError::Schedule(format!(
                "bundle `{}` points to a directory; pass a concrete file path",
                self.bundle.ident
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

        let mut outputs = PortOutputs::new();
        outputs.insert_file(
            0,
            FileRef {
                path: self.bundle.vpath.clone(),
                format: self.format.clone(),
                fingerprint: Some(FileFingerprint {
                    size: metadata.content_length(),
                    mtime_ns,
                    content_hash: metadata.etag().map(str::to_string),
                }),
            },
        );

        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            bundle: self.bundle.clone(),
            format: self.format.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        "bundle_source"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl NodeFactory for BundleSourceNodeFactory {
    fn kind(&self) -> &'static str {
        "bundle_source"
    }

    fn desc(&self) -> &'static str {
        "Exposes a runtime VFS data bundle as a DAG file artifact."
    }

    fn doc(&self) -> &'static str {
        "A source node that resolves a logical DataBundle through the runtime \
        VFS, validates its object metadata, and emits a virtual file reference. \
        The node does not download the bundle; consumers read it through VFS \
        or request local staging from the data plane."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BundleSourceNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: BundleSourceNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(BundleSourceNode::new(
            DataBundle::new(node_spec.ident, node_spec.desc, node_spec.vpath),
            node_spec.format,
        )))
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

    struct MountedVfsHarness {
        storage: Arc<OpendalFileStorage>,
        data_dir: tempfile::TempDir,
        _source_dir: tempfile::TempDir,
    }

    fn mounted_vfs() -> MountedVfsHarness {
        let data_dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        std::fs::write(source_dir.path().join("panel.txt"), b"panel-data").unwrap();

        let manifest = VfsManifest {
            backend: vec![
                BackendDefinition {
                    id: "runtime".into(),
                    config: BackendConfig::local(data_dir.path().to_string_lossy().to_string()),
                },
                BackendDefinition {
                    id: "external".into(),
                    config: BackendConfig::local(source_dir.path().to_string_lossy().to_string()),
                },
            ],
            mount: vec![
                MountDefinition {
                    path: "/".into(),
                    backend: "runtime".into(),
                    source: "/".into(),
                    read_only: false,
                },
                MountDefinition {
                    path: "/bundles/panels".into(),
                    backend: "external".into(),
                    source: "/".into(),
                    read_only: true,
                },
            ],
        };
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(data_dir.path(), mounted));

        MountedVfsHarness {
            storage,
            data_dir,
            _source_dir: source_dir,
        }
    }

    #[tokio::test]
    async fn dag_outputs_a_virtual_bundle_file() {
        let harness = mounted_vfs();
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), Some(harness.storage));
        let mut dag = DAG::default();
        dag.add_node(
            "source".into(),
            Box::new(BundleSourceNode::new(
                DataBundle::new("panels", "Reference panels", "/bundles/panels/panel.txt"),
                Some("txt".to_string()),
            )),
        )
        .unwrap();

        dag.run(&SchedulerConfig::default(), &ctx, None)
            .await
            .unwrap();

        let output = dag.output("source").unwrap();
        let file = output.get(&0).unwrap().as_file().unwrap();
        assert_eq!(file.path, "/bundles/panels/panel.txt");
        assert_eq!(file.format.as_deref(), Some("txt"));
        assert_eq!(
            file.fingerprint.as_ref().unwrap().size,
            b"panel-data".len() as u64
        );
        assert!(!harness.data_dir.path().join("panel.txt").exists());
    }

    #[tokio::test]
    async fn missing_vfs_rejects_execution() {
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let mut node = BundleSourceNode::new(
            DataBundle::new("panels", "Reference panels", "/bundles/panels/panel.txt"),
            None,
        );
        let reporter =
            dag_core::dag::node_event::NodeReporter::new("source", tokio::sync::mpsc::channel(1).0);

        let error = node.execute(&ctx, &[], &reporter).await.unwrap_err();

        assert!(error.to_string().contains("runtime VFS"));
    }

    #[test]
    fn factory_builds_bundle_source() {
        let spec = serde_json::json!({
            "ident": "panels",
            "desc": "Reference panels",
            "vpath": "/bundles/panels/panel.txt",
            "format": "txt"
        });
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);

        let node = BundleSourceNodeFactory {}.build(spec, ctx).unwrap();

        assert_eq!(node.kind(), "bundle_source");
    }
}
