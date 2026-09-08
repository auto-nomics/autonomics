//! `source_uniprot_stream` — bulk-download UniProtKB query results to a file.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileRef, PortType};

use crate::UniProtClient;
use crate::types::Format;

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

/// Spec for [`UniprotStreamNode`].
#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct UniprotStreamSpec {
    /// UniProt query expression, e.g. `"organism_id:9606 AND reviewed:true"`
    /// or `"proteome:UP000005640"` (whole human reference proteome).
    #[serde(default)]
    pub query: String,

    /// Output format: `tsv` (default), `fasta`, `list`, `gff`, `txt`, `xml`,
    /// `json`. `tsv` starts with a header row of field names.
    #[serde(default)]
    pub format: Option<String>,

    /// TSV field names to restrict the output to, e.g.
    /// `["accession", "id", "protein_name"]`. Ignored for `fasta`/`list`.
    #[serde(default)]
    pub fields: Option<Vec<String>>,

    /// Destination path: `vfs://...` for the engine's object storage, or an
    /// absolute local path.
    #[serde(default)]
    pub path: String,
}

// ---------------------------------------------------------------------------
// Node + Factory
// ---------------------------------------------------------------------------

/// Source node bulk-downloading raw UniProtKB output to a file.
#[derive(Clone)]
pub struct UniprotStreamNode {
    meta: NodePorts,
    spec: UniprotStreamSpec,
}

pub struct UniprotStreamNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for UniprotStreamNodeFactory {
    fn kind(&self) -> &'static str {
        "source_uniprot_stream"
    }

    fn desc(&self) -> &'static str {
        "Bulk-download UniProtKB query results (TSV/FASTA/…) to a file."
    }

    fn doc(&self) -> &'static str {
        "A source node that calls the UniProt REST API `/uniprotkb/stream` \
        endpoint (unpaginated, no entry cap — the API's preferred bulk \
        path) and writes the raw output to a file. No input ports; one \
        File output port.\n\n\
        Use `query` with `format=fasta` to download sequences for a set of \
        organisms/proteomes (e.g. `proteome:UP000005640`), or \
        `format=tsv` + `fields` for tabular annotation extracts (the TSV \
        starts with a header row).\n\n\
        The output FileRef composes with `file_to_dataframe` (csv/tsv, and \
        fasta via the biofusion driver) or `dataframe_to_file`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(UniprotStreamSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: UniprotStreamSpec = serde_json::from_value(spec)?;
        Ok(Box::new(UniprotStreamNode {
            meta: port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for UniprotStreamNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_uniprot_stream"
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
        if self.spec.query.trim().is_empty() {
            return Err(DagError::Schedule(
                "source_uniprot_stream requires a non-empty `query`".into(),
            ));
        }
        if self.spec.path.trim().is_empty() {
            return Err(DagError::Schedule(
                "source_uniprot_stream requires a `path` (vfs://… or absolute)".into(),
            ));
        }
        let format =
            Format::parse(self.spec.format.as_deref().unwrap_or("tsv")).ok_or_else(|| {
                DagError::Schedule(format!(
                    "source_uniprot_stream: invalid format {:?} \
                     (expected tsv, fasta, list, gff, txt, xml or json)",
                    self.spec.format
                ))
            })?;

        let client = UniProtClient::new();
        let text = client
            .stream(self.spec.query.trim(), self.spec.fields.as_deref(), format)
            .await
            .map_err(|e| DagError::Schedule(format!("UniProt stream failed: {e}")))?;

        let file_ref =
            write_output(ctx, &self.spec.path, format.as_str(), text.into_bytes()).await?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert_file(0, file_ref);
        Ok(res)
    }
}

// ---------------------------------------------------------------------------
// Output writing
// ---------------------------------------------------------------------------

/// Write `bytes` to `path`, returning a fingerprinted [`FileRef`].
///
/// `vfs://…` paths go through the engine's opendal storage (atomic
/// staging + rename); anything else must be an absolute local path.
async fn write_output(
    ctx: &NodeCtx,
    path: &str,
    format: &str,
    bytes: Vec<u8>,
) -> Result<FileRef, DagError> {
    let size = bytes.len() as u64;
    let sha256 = sha256_hex(&bytes);

    if let Some(vpath) = path.strip_prefix("vfs://") {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(format!(
                "path `{path}` requires engine file storage, but none is registered"
            ))
        })?;
        storage
            .check_writable(vpath)
            .map_err(|e| DagError::Schedule(format!("vfs path `{path}` is not writable: {e}")))?;
        storage
            .write_bytes(vpath, bytes)
            .await
            .map_err(|e| DagError::Schedule(format!("failed to write `{path}`: {e}")))?;
        Ok(FileRef::remote(
            path,
            Some(format.to_owned()),
            size,
            Some(sha256),
        ))
    } else {
        let dest = std::path::Path::new(path);
        if !dest.is_absolute() {
            return Err(DagError::Schedule(format!(
                "path `{path}` must be a `vfs://` address or an absolute local path"
            )));
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                DagError::Schedule(format!(
                    "failed to create directory `{}`: {e}",
                    parent.display()
                ))
            })?;
        }
        tokio::fs::write(dest, &bytes)
            .await
            .map_err(|e| DagError::Schedule(format!("failed to write `{path}`: {e}")))?;
        FileRef::local(dest, Some(format.to_owned()))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    #[tokio::test]
    async fn writes_local_file_with_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.tsv");
        let path_str = path.to_str().unwrap().to_owned();
        let payload = b"Entry\tEntry Name\nP01308\tINS_HUMAN".to_vec();

        let file_ref = write_output(&ctx(), &path_str, "tsv", payload.clone())
            .await
            .unwrap();

        assert_eq!(file_ref.path, path_str);
        assert_eq!(file_ref.format.as_deref(), Some("tsv"));
        let fingerprint = file_ref.fingerprint.expect("fingerprint");
        assert_eq!(fingerprint.size, payload.len() as u64);
        // Local refs fingerprint by mtime; content hashing is reserved for
        // vfs:// writes (see the `remote` fingerprint in `write_output`).
        assert!(fingerprint.content_hash.is_none());
        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(on_disk, payload);
    }

    #[tokio::test]
    async fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c/data.fasta");
        write_output(
            &ctx(),
            path.to_str().unwrap(),
            "fasta",
            b">P01308\nMALWMRLL".to_vec(),
        )
        .await
        .unwrap();
        assert!(path.is_file());
    }

    #[tokio::test]
    async fn rejects_relative_path() {
        let err = write_output(&ctx(), "relative/out.tsv", "tsv", vec![1, 2, 3])
            .await
            .unwrap_err();
        assert!(matches!(err, DagError::Schedule(_)));
    }

    #[tokio::test]
    async fn vfs_path_requires_storage() {
        let err = write_output(&ctx(), "vfs://data/x.tsv", "tsv", vec![1, 2, 3])
            .await
            .unwrap_err();
        assert!(matches!(err, DagError::Schedule(_)));
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // sha256("abc")
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn empty_spec_defaults() {
        let spec: UniprotStreamSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(spec.query, "");
        assert_eq!(spec.format, None);
        assert_eq!(spec.path, "");
    }
}
