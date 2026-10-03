//! DAG nodes for the evidence channel.
//!
//! Three kinds live here, all producing/consuming `FileRef` values tagged
//! with the [`bib_types::evidence::FORMAT`] format contract:
//!
//! - [`literature_search`]: `source_literature` — fan a structured query out
//!   through the [`LiteratureGateway`](crate::query::LiteratureGateway) and
//!   emit one deduplicated evidence file.
//! - [`evidence_merge`]: merge any number of evidence files into one
//!   deduplicated set.
//! - [`evidence_export`]: render an evidence file as bibtex / ris / markdown.
//!
//! Shared artifact plumbing lives in this module: every produced `FileRef`
//! carries a `sha256:{hex}` content hash so downstream fingerprints are
//! content-addressed (see `dag_core::fingerprint` encoding).

use std::sync::{Arc, OnceLock};

pub mod evidence_export;
pub mod literature_citations;
pub mod literature_fetch;
pub mod literature_fulltext;
pub mod evidence_merge;
pub mod literature_search;
pub mod s2_recommendations;

use dag_core::dag::DagError;
use dag_core::registry::NodeCtx;
use dag_core::value::{FileFingerprint, FileRef};
use sha2::{Digest, Sha256};

/// Validate a node output path: `vfs://` URI, `file://` URI, or absolute
/// host path. Relative paths are rejected (mirror of
/// `nodes-io::file_reference::local_path` — that helper is `pub(crate)` and
/// cannot be shared across crates).
pub(crate) fn validate_output_path(path: &str) -> Result<(), DagError> {
    if path.starts_with("vfs://") {
        return Ok(());
    }
    let local = path.strip_prefix("file://").unwrap_or(path);
    if !std::path::Path::new(local).is_absolute() {
        return Err(DagError::Schedule(format!(
            "output path must be a `vfs://` URI or absolute path, got `{path}`"
        )));
    }
    Ok(())
}

/// Read file bytes through the node context: `vfs://` URIs resolve through
/// the mounted opendal storage, everything else reads the host filesystem.
pub(crate) async fn read_file_bytes(ctx: &NodeCtx, path: &str) -> Result<Vec<u8>, DagError> {
    if let Some(virtual_path) = path.strip_prefix("vfs://") {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(format!("VFS input `{path}` requires a mounted runtime VFS"))
        })?;
        let operator = storage.resolve(virtual_path);
        return operator
            .read(&storage.resolve_path(virtual_path))
            .await
            .map(|buffer| buffer.to_vec())
            .map_err(|error| {
                DagError::Schedule(format!("cannot read VFS file `{path}`: {error}"))
            });
    }
    let local = path.strip_prefix("file://").unwrap_or(path);
    tokio::fs::read(local)
        .await
        .map_err(|error| DagError::Schedule(format!("cannot read file `{path}`: {error}")))
}

/// Write bytes through the node context (VFS or host filesystem) and return
/// a `FileRef` whose fingerprint carries a `sha256:{hex}` content hash —
/// the content-addressed identity downstream nodes rely on for incremental
/// invalidation.
pub(crate) async fn write_artifact(
    ctx: &NodeCtx,
    path: &str,
    bytes: Vec<u8>,
    format: &str,
) -> Result<FileRef, DagError> {
    validate_output_path(path)?;
    let size = bytes.len() as u64;
    let content_hash = format!("sha256:{:x}", Sha256::digest(&bytes));

    if let Some(virtual_path) = path.strip_prefix("vfs://") {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(format!("VFS output `{path}` requires a mounted runtime VFS"))
        })?;
        let operator = storage.resolve(virtual_path);
        operator
            .write(&storage.resolve_path(virtual_path), bytes)
            .await
            .map_err(|error| {
                DagError::Schedule(format!("cannot write VFS file `{path}`: {error}"))
            })?;
        // Object-store artifacts are immutable by construction: mtimes are
        // worker-local and meaningless, a recorded hash is permanently clean.
        return Ok(FileRef {
            path: path.to_string(),
            format: Some(format.to_string()),
            fingerprint: Some(FileFingerprint {
                size,
                mtime_ns: 0,
                content_hash: Some(content_hash),
                immutable_remote: true,
            }),
        });
    }

    let local = path.strip_prefix("file://").unwrap_or(path);
    if let Some(parent) = std::path::Path::new(local).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| {
                DagError::Schedule(format!("cannot create `{}`: {error}", parent.display()))
            })?;
    }
    tokio::fs::write(local, &bytes)
        .await
        .map_err(|error| DagError::Schedule(format!("cannot write file `{path}`: {error}")))?;
    let mtime_ns = tokio::fs::metadata(local)
        .await
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128)
        .unwrap_or_default();
    Ok(FileRef {
        path: path.to_string(),
        format: Some(format.to_string()),
        fingerprint: Some(FileFingerprint {
            size,
            mtime_ns,
            content_hash: Some(content_hash),
            immutable_remote: false,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    #[test]
    fn rejects_relative_output_paths() {
        assert!(validate_output_path("relative/out.json").is_err());
        assert!(validate_output_path("out.json").is_err());
        assert!(validate_output_path("/abs/out.json").is_ok());
        assert!(validate_output_path("file:///abs/out.json").is_ok());
        assert!(validate_output_path("vfs://artifacts/out.json").is_ok());
    }

    #[tokio::test]
    async fn write_artifact_attaches_sha256_fingerprint() {
        let dir = std::env::temp_dir().join(format!("evidence-artifact-{}", uuid::Uuid::new_v4()));
        let path = dir.join("out.json");
        let path_str = path.to_str().unwrap().to_string();

        let file = write_artifact(&test_ctx(), &path_str, br#"{"x":1}"#.to_vec(), "evidence")
            .await
            .unwrap();
        assert_eq!(file.format.as_deref(), Some("evidence"));
        let fingerprint = file.fingerprint.expect("fingerprint attached");
        assert_eq!(fingerprint.size, 7);
        assert!(fingerprint
            .content_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("sha256:") && hash.len() == "sha256:".len() + 64));
        assert!(!fingerprint.immutable_remote);
        assert!(fingerprint.mtime_ns > 0);

        // Round-trip through the read helper.
        let bytes = read_file_bytes(&test_ctx(), &path_str).await.unwrap();
        assert_eq!(bytes, br#"{"x":1}"#);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn read_file_bytes_rejects_missing_vfs_without_storage() {
        let err = read_file_bytes(&test_ctx(), "vfs://artifacts/none.json")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("mounted runtime VFS"));
    }
}


// ---------------------------------------------------------------------------
// Shared bibliography handle
// ---------------------------------------------------------------------------

static SHARED_BIB: OnceLock<Arc<crate::shared::BibShared>> = OnceLock::new();

/// Install the runtime host's shared bibliography for
/// `literature_fulltext` (and future library-backed nodes). Called by
/// `SharedInfra::open`; idempotent — the first install wins.
pub fn set_shared_bib(shared: Arc<crate::shared::BibShared>) {
    let _ = SHARED_BIB.set(shared);
}

/// The host-installed shared bibliography, or a fail-closed error explaining
/// what is missing.
pub fn shared_bib() -> Result<Arc<crate::shared::BibShared>, dag_core::dag::DagError> {
    SHARED_BIB.get().cloned().ok_or_else(|| {
        dag_core::dag::DagError::Schedule(
            "literature_fulltext requires the runtime host's shared bibliography; \
             no SharedInfra installed one in this process".into(),
        )
    })
}
