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
pub mod evidence_merge;
pub mod literature_citations;
pub mod literature_fetch;
pub mod literature_fulltext;
pub mod literature_search;
pub mod s2_recommendations;

use dag_core::dag::DagError;
use dag_core::registry::NodeCtx;
use dag_core::value::{FileFingerprint, FileRef};
use sha2::{Digest, Sha256};

/// Validate a node output path: `vfs://` URI, `file://` URI, or absolute
/// host path. Relative paths are rejected (mirror of
/// `nodes-io::file_reference::local_path` — that helper is `pub(crate)` and
/// cannot be shared across crates). All three forms are *normalized to* a
/// `vfs://` URI by [`to_vfs_uri`] before any I/O is attempted; the engine
/// has no other writable namespace.
pub(crate) fn validate_output_path(path: &str) -> Result<(), DagError> {
    if path.starts_with("vfs://") || path.starts_with("file://") {
        return Ok(());
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err(DagError::Schedule(format!(
            "output path must be a `vfs://` URI or an absolute path (auto-routed \
             through the mounted runtime VFS so the agent and engine share one \
             object-store namespace); got `{path}`"
        )));
    }
    Ok(())
}

/// Resolve a node output path to its canonical `vfs://` form so the engine
/// and the agent share one mounted object-store namespace. Bare absolute
/// paths are transparently re-prefixed with `vfs://`; the legacy `file://`
/// prefix is normalized identically (it was never anything other than an
/// alias for an absolute host path). Relative paths fall through to the
/// caller unchanged so the validation error stays informative.
///
/// Callers must run [`validate_output_path`] first; this function does not
/// re-check absoluteness so an unsupported path can still be reported with
/// the original wording.
pub(crate) fn to_vfs_uri(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("vfs://") {
        format!("vfs://{stripped}")
    } else if let Some(stripped) = path.strip_prefix("file://") {
        format!("vfs://{stripped}")
    } else if std::path::Path::new(path).is_absolute() {
        format!("vfs://{path}")
    } else {
        path.to_string()
    }
}

/// Read file bytes through the node context.
///
/// Routing, in order:
/// 1. An explicit `vfs://` URI **requires** the mounted opendal storage
///    (fail-closed: the caller asked for the shared namespace by name).
/// 2. Any other accepted path form (bare absolute, `file://`) routes
///    through the mounted VFS when one exists — the engine and the agent
///    share one object-store namespace — and falls back to a host-FS read
///    when the engine runs without a VFS (embedded / test engines).
pub(crate) async fn read_file_bytes(ctx: &NodeCtx, path: &str) -> Result<Vec<u8>, DagError> {
    if !path.starts_with("vfs://") && ctx.opendal.is_none() {
        let local = path.strip_prefix("file://").unwrap_or(path);
        return tokio::fs::read(local)
            .await
            .map_err(|error| DagError::Schedule(format!("cannot read file `{path}`: {error}")));
    }
    let virtual_path = to_vfs_uri(path).strip_prefix("vfs://").unwrap().to_string();
    let storage = ctx.opendal.as_ref().ok_or_else(|| {
        DagError::Schedule(format!("VFS input `{path}` requires a mounted runtime VFS"))
    })?;
    let operator = storage.resolve(&virtual_path);
    operator
        .read(&storage.resolve_path(&virtual_path))
        .await
        .map(|buffer| buffer.to_vec())
        .map_err(|error| DagError::Schedule(format!("cannot read VFS file `{path}`: {error}")))
}

/// Write bytes through the node context and return a `FileRef` whose
/// fingerprint carries a `sha256:{hex}` content hash — the
/// content-addressed identity downstream nodes rely on for incremental
/// invalidation.
///
/// Routing mirrors [`read_file_bytes`]: an explicit `vfs://` URI requires
/// the mounted storage; a bare absolute path routes through the mounted
/// VFS when one exists (canonical — object-store artifacts are immutable
/// by construction, mtimes are worker-local and meaningless, so the
/// recorded hash is permanently clean and `immutable_remote: true`), and
/// falls back to a host-FS write when the engine runs without a VFS.
pub(crate) async fn write_artifact(
    ctx: &NodeCtx,
    path: &str,
    bytes: Vec<u8>,
    format: &str,
) -> Result<FileRef, DagError> {
    validate_output_path(path)?;
    let size = bytes.len() as u64;
    let content_hash = format!("sha256:{:x}", Sha256::digest(&bytes));

    if !path.starts_with("vfs://") && ctx.opendal.is_none() {
        // Legacy embedded-engine path: no mounted VFS, write the host FS.
        let local = path.strip_prefix("file://").unwrap_or(path);
        if let Some(parent) = std::path::Path::new(local).parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
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
        return Ok(FileRef {
            path: path.to_string(),
            format: Some(format.to_string()),
            fingerprint: Some(FileFingerprint {
                size,
                mtime_ns,
                content_hash: Some(content_hash),
                immutable_remote: false,
            }),
        });
    }

    let vfs_uri = to_vfs_uri(path);
    let virtual_path = vfs_uri.strip_prefix("vfs://").unwrap().to_string();
    let storage = ctx.opendal.as_ref().ok_or_else(|| {
        DagError::Schedule(format!(
            "VFS output `{path}` requires a mounted runtime VFS"
        ))
    })?;
    let operator = storage.resolve(&virtual_path);
    operator
        .write(&storage.resolve_path(&virtual_path), bytes)
        .await
        .map_err(|error| DagError::Schedule(format!("cannot write VFS file `{path}`: {error}")))?;

    Ok(FileRef {
        path: vfs_uri,
        format: Some(format.to_string()),
        fingerprint: Some(FileFingerprint {
            size,
            mtime_ns: 0,
            content_hash: Some(content_hash),
            immutable_remote: true,
        }),
    })
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
             no SharedInfra installed one in this process"
                .into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> (NodeCtx, Arc<vfs::OpendalFileStorage>) {
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage.clone()),
        );
        (ctx, storage)
    }

    #[test]
    fn rejects_relative_output_paths() {
        assert!(validate_output_path("relative/out.json").is_err());
        assert!(validate_output_path("out.json").is_err());
        assert!(validate_output_path("/abs/out.json").is_ok());
        assert!(validate_output_path("file:///abs/out.json").is_ok());
        assert!(validate_output_path("vfs://artifacts/out.json").is_ok());
    }

    #[test]
    fn absolute_path_normalizes_to_vfs_uri() {
        assert_eq!(to_vfs_uri("/abs/out.json"), "vfs:///abs/out.json");
        assert_eq!(
            to_vfs_uri("vfs://artifacts/out.json"),
            "vfs://artifacts/out.json"
        );
        assert_eq!(to_vfs_uri("file:///abs/out.json"), "vfs:///abs/out.json");
        // Relative paths fall through so the validator's error stays informative.
        assert_eq!(to_vfs_uri("rel.json"), "rel.json");
    }

    #[tokio::test]
    async fn write_artifact_attaches_sha256_fingerprint() {
        let (ctx, storage) = test_ctx();
        let path = format!(
            "/vfs-test/evidence-artifact-{}/out.json",
            uuid::Uuid::new_v4()
        );

        let file = write_artifact(&ctx, &path, br#"{"x":1}"#.to_vec(), "evidence")
            .await
            .unwrap();
        assert_eq!(file.format.as_deref(), Some("evidence"));
        // The FileRef's path is the canonical vfs:// URI the agent and engine share.
        assert!(file.path.starts_with("vfs://"));
        let fingerprint = file.fingerprint.expect("fingerprint attached");
        assert_eq!(fingerprint.size, 7);
        assert!(
            fingerprint.content_hash.as_deref().is_some_and(
                |hash| hash.starts_with("sha256:") && hash.len() == "sha256:".len() + 64
            )
        );
        assert!(fingerprint.immutable_remote);

        // Round-trip through the read helper.
        let bytes = read_file_bytes(&ctx, &path).await.unwrap();
        assert_eq!(bytes, br#"{"x":1}"#);
        // Temp storage is dropped at scope exit (no host FS cleanup needed).
        drop(storage);
    }

    #[tokio::test]
    async fn vfs_uri_requires_mounted_vfs_bare_path_falls_back_to_host() {
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        // Explicit vfs:// on an unmounted engine: fail closed.
        let err = write_artifact(
            &ctx,
            "vfs://artifacts/out.json",
            br#"{}"#.to_vec(),
            "evidence",
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("mounted runtime VFS"));
        let err = read_file_bytes(&ctx, "vfs://artifacts/none.json")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("mounted runtime VFS"));

        // Bare absolute path on an unmounted engine: legacy host-FS write.
        let host_dir = std::env::temp_dir().join(format!("evidence-host-{}", uuid::Uuid::new_v4()));
        let host_path = host_dir.join("out.json");
        let file = write_artifact(
            &ctx,
            host_path.to_str().unwrap(),
            br#"{"x":1}"#.to_vec(),
            "evidence",
        )
        .await
        .unwrap();
        // No normalization applied — the recorded path stays the host path.
        assert_eq!(file.path, host_path.to_str().unwrap());
        assert!(!file.fingerprint.as_ref().unwrap().immutable_remote);
        let bytes = read_file_bytes(&ctx, &file.path).await.unwrap();
        assert_eq!(bytes, br#"{"x":1}"#);
        std::fs::remove_dir_all(&host_dir).ok();
    }
}
