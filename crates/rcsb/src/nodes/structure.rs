use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::RcsbClient;
use crate::StructureFormat;

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct RcsbStructureSpec {
    /// Four-character PDB entry ID.
    #[serde(default)]
    pub entry_id: String,

    /// `cif`/`mmcif` (default), `pdb`, `bcif`, or `fasta`.
    #[serde(default)]
    pub format: Option<String>,

    /// `vfs://...` engine storage path or an absolute local path.
    #[serde(default)]
    pub path: String,
}

#[derive(Clone)]
pub struct RcsbStructureNode {
    meta: NodePorts,
    spec: RcsbStructureSpec,
}

pub struct RcsbStructureNodeFactory;

impl NodeFactory for RcsbStructureNodeFactory {
    fn kind(&self) -> &'static str {
        "source_rcsb_structure"
    }

    fn desc(&self) -> &'static str {
        "Download a PDB structure file for downstream computation."
    }

    fn doc(&self) -> &'static str {
        "Downloads the deposited structure from RCSB and emits a FileRef. \
        Supported formats are mmCIF (`cif`/`mmcif`), legacy PDB (`pdb`), \
        BinaryCIF (`bcif`), and FASTA. mmCIF and BinaryCIF preserve all model \
        categories and are preferred for computational nodes."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RcsbStructureSpec)
    }

    fn ports(&self) -> NodePorts {
        super::util::file_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(RcsbStructureNode {
            meta: super::util::file_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for RcsbStructureNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_rcsb_structure"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.spec.entry_id.trim().is_empty() {
            return Err(DagError::Schedule(
                "source_rcsb_structure requires `entry_id`".into(),
            ));
        }
        if self.spec.path.trim().is_empty() {
            return Err(DagError::Schedule(
                "source_rcsb_structure requires `path`".into(),
            ));
        }
        let format_name = self.spec.format.as_deref().unwrap_or("cif");
        let format = StructureFormat::parse(format_name).ok_or_else(|| {
            DagError::Schedule(format!(
                "source_rcsb_structure: invalid format {format_name:?}"
            ))
        })?;

        let bytes = RcsbClient::new()
            .structure_bytes(&self.spec.entry_id, format)
            .await
            .map_err(|e| DagError::Schedule(format!("RCSB structure download failed: {e}")))?;
        let file_ref =
            super::util::write_output(ctx, &self.spec.path, format.as_str(), bytes).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file_ref);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    #[tokio::test]
    async fn writes_local_output_with_content_hash_for_vfs_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("4hhb.cif");
        let path_str = path.to_str().unwrap().to_owned();
        let file =
            super::super::util::write_output(&ctx(), &path_str, "cif", b"data_4HHB".to_vec())
                .await
                .unwrap();

        assert_eq!(file.path, path_str);
        assert_eq!(file.format.as_deref(), Some("cif"));
        assert!(path.is_file());
    }

    #[test]
    fn spec_defaults() {
        let spec: RcsbStructureSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(spec.entry_id, "");
        assert_eq!(spec.path, "");
    }
}
