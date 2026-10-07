//! `source_gwascatalog_download` — full summary-statistics files as a
//! FileSet of VFS objects.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileFingerprint, FileRef};

use crate::nodes::util;
use crate::tools::download::FileVariant;

/// Spec for [`DownloadNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct DownloadSpec {
    /// GWAS Catalog study accession, e.g. `GCST90000061`.
    pub accession: String,
    /// Which files to fetch: `harmonised` (default), `raw`, or `all`.
    #[serde(default)]
    pub variant: Option<String>,
    /// Genome assembly for raw files: `GRCh37` or `GRCh38`. Omit to prefer
    /// GRCh38 and fall back to GRCh37.
    #[serde(default)]
    pub assembly: Option<String>,
    /// Destination directory in engine storage (default `/{accession}`).
    /// Accepts a `vfs://` URI or a bare absolute path (auto-routed through
    /// the mounted runtime VFS).
    #[serde(default)]
    pub dest_dir: Option<String>,
    /// Endpoint override for tests and mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Download the complete summary-statistics file(s) of one study.
#[derive(Clone)]
pub struct DownloadNode {
    meta: NodePorts,
    spec: DownloadSpec,
}

pub struct DownloadNodeFactory;

impl NodeFactory for DownloadNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_download"
    }

    fn desc(&self) -> &'static str {
        "Download a GWAS Catalog study's full summary-statistics files as a FileSet."
    }

    fn doc(&self) -> &'static str {
        "A source node that streams the complete `.tsv` / `.tsv.gz` summary-\
         statistics files the GWAS Catalog publishes per study (HTTPS FTP \
         mirror) into engine storage, emitting a FileSet of FileRefs.\n\n\
         By default downloads the harmonised file(s) under `harmonised/` \
         (`variant: harmonised`; recommended for cross-study analysis). Use \
         `variant: raw` for unharmonised per-build `.tsv` (`assembly` picks \
         GRCh37/GRCh38), or `variant: all` for both.\n\n\
         `dest_dir` defaults to `/{accession}`; each file lands at \
         `{dest_dir}/{harmonised/…|filename}`. Filenames embed a \
         submission-specific prefix, so the node lists the FTP directory at \
         runtime to discover them.\n\n\
         Files can be several hundred MiB — the node is long-running by \
         design. `file_to_dataframe` / `file_decompress` consume the FileSet \
         downstream. For API-paginated per-variant statistics use \
         `source_gwascatalog_summary_associations` instead."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DownloadSpec)
    }

    fn ports(&self) -> NodePorts {
        util::file_set_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: DownloadSpec = serde_json::from_value(spec)?;
        Ok(Box::new(DownloadNode {
            meta: util::file_set_port(),
            spec,
        }))
    }
}

/// Resolve the (relative path, absolute URL) download targets for a study —
/// the directory-listing logic shared with the agent tool, kept identical so
/// tool and node surface the same file set.
async fn resolve_targets(
    client: &crate::GwasCatalogClient,
    accession: &str,
    variant: FileVariant,
    assembly: Option<&str>,
) -> Result<Vec<(String, String)>, DagError> {
    let block_url = client
        .ftp_block_url(accession)
        .map_err(|e| DagError::Schedule(format!("source_gwascatalog_download: {e}")))?;
    let study_url = format!("{block_url}/{accession}");
    let root_entries = client.list_ftp_directory(&study_url).await.map_err(|e| {
        DagError::Schedule(format!(
            "source_gwascatalog_download: FTP listing failed: {e}"
        ))
    })?;

    let mut targets = Vec::new();

    if variant != FileVariant::Raw && root_entries.iter().any(|e| e.starts_with("harmonised")) {
        let harm_url = format!("{study_url}/harmonised");
        let harm_entries = client.list_ftp_directory(&harm_url).await.map_err(|e| {
            DagError::Schedule(format!(
                "source_gwascatalog_download: harmonised listing failed: {e}"
            ))
        })?;
        for name in &harm_entries {
            if name.ends_with(".h.tsv.gz") || name.ends_with(".h.tsv.gz-meta.yaml") {
                targets.push((format!("harmonised/{name}"), format!("{harm_url}/{name}")));
            }
        }
    }

    if variant != FileVariant::Harmonised {
        let mut candidates: Vec<&String> = root_entries
            .iter()
            .filter(|e| {
                e.starts_with(&format!("{accession}_build"))
                    && (e.ends_with(".tsv") || e.ends_with(".tsv.gz"))
            })
            .collect();
        match assembly {
            Some(want) => candidates.sort_by_key(|e| !e.to_ascii_uppercase().contains(want)),
            None => candidates.sort_by_key(|e| !e.to_ascii_uppercase().contains("GRCH38")),
        }
        for c in &candidates {
            targets.push((c.to_string(), format!("{study_url}/{c}")));
        }
    }

    Ok(targets)
}

#[async_trait]
impl DagNode for DownloadNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_download"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        self.spec.dest_dir.as_deref()
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let accession = self.spec.accession.trim().to_string();
        if accession.is_empty() {
            return Err(DagError::Schedule(
                "source_gwascatalog_download requires `accession`".into(),
            ));
        }
        let variant = self
            .spec
            .variant
            .as_deref()
            .map(FileVariant::from_str)
            .unwrap_or_default();
        let default_dest = format!("/{accession}");
        let dest_dir = self
            .spec
            .dest_dir
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(&default_dest);
        util::validate_output_path(dest_dir)?;
        let dest_dir = util::to_vfs_uri(dest_dir);

        let client = match self.spec.endpoint.as_deref() {
            Some(url) => crate::client::GwasCatalogClient::with_base_url(url),
            None => crate::GwasCatalogClient::new(),
        };

        let targets = resolve_targets(
            &client,
            &accession,
            variant.clone(),
            self.spec.assembly.as_deref(),
        )
        .await?;
        if targets.is_empty() {
            return Err(DagError::Schedule(format!(
                "source_gwascatalog_download: no downloadable summary-statistics files found \
                 for {accession} (variant={variant:?}, assembly={:?})",
                self.spec.assembly
            )));
        }

        // Storage is required for the actual writes; resolve it up front so
        // a missing VFS fails before any network traffic.
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(
                "source_gwascatalog_download writes into engine storage; mount a runtime \
                 VFS (or run on the embedded host-FS fallback path with a bare absolute \
                 dest_dir)"
                    .into(),
            )
        })?;

        let mut files: Vec<FileRef> = Vec::new();
        for (i, (relpath, url)) in targets.iter().enumerate() {
            let storage_path = format!("{dest_dir}/{relpath}");
            // Stream through the VFS (staging + atomic rename + streamed
            // SHA256): summary-statistics files reach hundreds of MiB and
            // must never be buffered whole.
            let downloaded = client
                .download_stream_to_storage(url, storage, &storage_path, |_, _| {})
                .await
                .map_err(|e| {
                    DagError::Schedule(format!(
                        "source_gwascatalog_download: downloading {url} failed: {e}"
                    ))
                })?;
            reporter.warn(format!(
                "downloaded {} ({}/{}) — {} bytes",
                relpath,
                i + 1,
                targets.len(),
                downloaded.bytes
            ));
            files.push(FileRef {
                path: storage_path,
                format: Some("tsv".to_string()),
                fingerprint: Some(FileFingerprint {
                    size: downloaded.bytes,
                    mtime_ns: 0,
                    content_hash: Some(format!("sha256:{}", downloaded.sha256)),
                    immutable_remote: true,
                }),
            });
        }

        let mut outputs = PortOutputs::new();
        outputs.insert(0, files);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_variant_string_mapping_matches_tool_semantics() {
        assert_eq!(FileVariant::from_str("harmonised"), FileVariant::Harmonised);
        assert_eq!(FileVariant::from_str("raw"), FileVariant::Raw);
        assert_eq!(FileVariant::from_str("all"), FileVariant::All);
        // Unknown values fall back to the default, mirroring the tool.
        assert_eq!(FileVariant::from_str("bogus"), FileVariant::Harmonised);
    }

    #[test]
    fn dest_dir_defaults_and_validation() {
        // Default dest_dir is `/{accession}` — an absolute path, so it
        // validates and normalizes to vfs://.
        let dest = "/GCST90000061";
        assert!(util::validate_output_path(dest).is_ok());
        assert_eq!(util::to_vfs_uri(dest), "vfs:///GCST90000061");
        // A relative dest_dir must be rejected.
        assert!(util::validate_output_path("relative/dir").is_err());
    }
}
