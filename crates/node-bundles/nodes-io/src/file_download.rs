//! Streaming, resumable, allowlist-gated file download node.
//!
//! `file_download` is the large-file sibling of [`crate::http_fetch::HttpFetchNode`]:
//! the same deny-by-default network allowlist gates the initial URL and every
//! redirect hop (see [`crate::net_policy`] — this node deliberately shares
//! that policy file instead of bypassing it), but the body is never buffered
//! in memory. Bytes stream to a staging object in fixed-size chunks, SHA256
//! is computed while streaming, and the artifact is published by an atomic
//! rename only after the full body arrived and every verification passed. A
//! staging file left behind by a cancelled or failed run is safely resumed on
//! the next run when the server offers a strong validator — a non-weak ETag
//! (`W/"…"` is never strong) or a Last-Modified date at least 60s in the past
//! (RFC 9110 §13.1.5) — sent as If-Range; otherwise the staging prefix is
//! dropped and the download restarts from zero rather than append to an
//! unverifiable prefix.
//!
//! The target is guarded by a `{path}.lock` claim for the duration of one
//! download. On filesystem-backed sinks (local paths and fs-mounted VFS) the
//! claim is an exclusive `O_EXCL` create, so two concurrent `overwrite=false`
//! runs can never both publish over each other: the second fails fast with
//! [`FileDownloadError::TargetLocked`], and the publish itself is a no-clobber
//! link (a plain rename when `overwrite=true`). Backends without host paths
//! have no exclusive create, so they run a claim election under a
//! `{key}.lock/` prefix where the smallest claim key holds the target. A
//! stale claim naming a dead process on this host is reclaimed; anything
//! else is reported for manual removal. The claim is released through a
//! guard whose `Drop` also removes it, so a cancelled run cannot leave its
//! own live pid holding the target.
//!
//! A sidecar manifest (`<path>.download.json`) records provenance for the
//! run ledger: accession/release when the caller knows them, original and
//! final URL, HTTP status, validator headers, bytes, SHA256, resume offset,
//! attempt count, and start/completion times. It is staged next to the data
//! first; the data publish is the commit point of the pair; the staged
//! manifest replaces the old one only after the data is ours. Failures
//! therefore leave either a complete pair (old or new) or nothing at all —
//! never old data described by a rewritten manifest, and never data whose
//! manifest write failed blocking a retry.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::value::{FileFingerprint, FileRef, PortType};
use dag_core::{
    dag::DagError,
    dag::graph::PortOutputs,
    registry::{NodeCtx, NodeFactory},
};

use crate::net_policy::{
    NetworkPolicyError, ensure_host_allowed, load_allowlist_from_env, redirect_policy,
};

pub const FILE_DOWNLOAD_KIND: &str = "file_download";

const DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024; // 64 GiB
const DEFAULT_RETRIES: u32 = 3;
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 120;
const MAX_BACKOFF_MS: u64 = 8_000;
/// Chunk size used when re-reading a staging prefix into the hasher on
/// resume. Peak memory stays at one chunk, never the whole file.
const RESUME_REREAD_CHUNK: u64 = 1024 * 1024;

#[derive(Debug, Error)]
pub enum FileDownloadError {
    #[error("invalid file_download spec: {message}")]
    InvalidSpec { message: String },

    #[error(transparent)]
    Policy(#[from] NetworkPolicyError),

    #[error("HTTP request to `{url}` failed: {source}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("stream of `{url}` stalled for {secs}s (idle timeout)")]
    IdleTimeout { url: String, secs: u64 },

    #[error("redirect from `{url}` was refused: {reason}")]
    RedirectRefused { url: String, reason: String },

    #[error("HTTP `{url}` answered {status}")]
    HttpStatus {
        url: String,
        status: reqwest::StatusCode,
    },

    #[error("`{url}` body exceeds file_download max_bytes ({max_bytes})")]
    TooLarge { url: String, max_bytes: u64 },

    #[error(
        "disk budget: `{path}` has {available} bytes free but the download needs \
         {required}; refusing to start"
    )]
    DiskBudget {
        path: String,
        required: u64,
        available: u64,
    },

    #[error(
        "checksum mismatch for `{url}` -> `{path}`: expected sha256 {expected}, got {actual}; \
         nothing was published"
    )]
    ChecksumMismatch {
        url: String,
        path: String,
        expected: String,
        actual: String,
    },

    #[error(
        "size mismatch for `{url}` -> `{path}`: expected {expected} bytes, got {actual}; \
         nothing was published"
    )]
    SizeMismatch {
        url: String,
        path: String,
        expected: u64,
        actual: u64,
    },

    #[error("cannot write `{path}`: {reason}")]
    Write { path: String, reason: String },

    #[error("output `{path}` already exists; pass overwrite=true to replace it")]
    AlreadyExists { path: String },

    #[error(
        "another download holds the lock on `{path}` (holder: {holder}); if that run is \
         finished, remove `{path}.lock`"
    )]
    TargetLocked { path: String, holder: String },

    #[error(
        "download of `{url}` failed after {attempts} attempt(s): {last}; staging file kept \
         for resume"
    )]
    AttemptsExhausted {
        url: String,
        attempts: u32,
        last: String,
    },
}

impl ::dag_core::dag::NodeError for FileDownloadError {
    fn node_type(&self) -> &str {
        "file_download"
    }
}

/// Free-form provenance the caller attaches to the download so the run
/// ledger can tie bytes back to an accession and release.
#[derive(Debug, Clone, Default, JsonSchema, Deserialize, Serialize)]
pub struct DownloadProvenance {
    /// Accession (or other stable identifier) this file belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accession: Option<String>,
    /// Upstream release / version tag when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// Human-readable source name (registry, database, study).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct FileDownloadSpec {
    /// Absolute http(s) URL to download.
    pub url: String,
    /// Output path for the file (`/…`, `file://…`, or `vfs://…`).
    pub path: String,
    /// Publisher-side SHA256 (hex). Verified before publish; a mismatch
    /// deletes the staging file and publishes nothing.
    #[serde(default)]
    pub expected_sha256: Option<String>,
    /// Expected exact byte count, verified against Content-Length and the
    /// received body before publish.
    #[serde(default)]
    pub expected_bytes: Option<u64>,
    /// Hard cap on the downloaded size. Bodies known or discovered to exceed
    /// it are refused (default 64 GiB).
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
    /// Extra attempts after the first one (default 3). Network errors, 5xx,
    /// 429, 408 and mid-stream resets are retried with backoff and safe
    /// resume.
    #[serde(default = "default_retries")]
    pub retries: u32,
    /// Per-read idle timeout: a stalled stream errors out after this many
    /// seconds without bytes (default 120). There is no total-time timeout —
    /// large files on slow links are the point of this node.
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// Resume from a leftover `.part` staging file when the server offered a
    /// strong validator (a non-weak ETag, or a Last-Modified date ≥60s in
    /// the past); otherwise restart from zero (default true).
    #[serde(default = "default_true")]
    pub resume: bool,
    /// Replace an existing output at `path` (default false — fail instead).
    #[serde(default)]
    pub overwrite: bool,
    /// Accession / release / source recorded in the sidecar manifest.
    #[serde(default)]
    pub provenance: DownloadProvenance,
}

fn default_max_bytes() -> u64 {
    DEFAULT_MAX_BYTES
}
fn default_retries() -> u32 {
    DEFAULT_RETRIES
}
fn default_idle_timeout_secs() -> u64 {
    DEFAULT_IDLE_TIMEOUT_SECS
}
fn default_true() -> bool {
    true
}

#[derive(Clone)]
pub struct FileDownloadNode {
    meta: NodePorts,
    spec: FileDownloadSpec,
}

pub struct FileDownloadNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type_with_label(None, PortType::File, "manifest")
}

/// Outcome of one request/response cycle: a verified body, a retryable
/// failure (consumes an attempt, staging kept for resume), or a terminal
/// failure (aborts the node).
enum AttemptOutcome {
    Done(Box<CompletedBody>),
    Retry(String),
    Fatal(FileDownloadError),
}

struct CompletedBody {
    bytes: u64,
    sha256: String,
    final_url: String,
    http_status: u16,
    etag: Option<String>,
    last_modified: Option<String>,
    date: Option<String>,
    resumed_from: u64,
}

/// Validator memory kept beside the staging file so a later attempt (or a
/// later run) can resume safely. Written when the response headers arrive,
/// before any body bytes stream. The response `Date` is recorded because
/// Last-Modified strength is judged relative to it (RFC 9110 §8.8.2.2), not
/// to any local clock.
#[derive(Serialize, Deserialize)]
struct StagingMeta {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

enum StagingWriter {
    Vfs(opendal::Writer),
    Local(tokio::fs::File),
}

impl StagingWriter {
    async fn write_all(
        &mut self,
        bytes: &[u8],
        path_for_errors: &str,
    ) -> Result<(), FileDownloadError> {
        match self {
            StagingWriter::Vfs(writer) => writer
                .write(opendal::Buffer::from(bytes.to_vec()))
                .await
                .map_err(|error| FileDownloadError::Write {
                    path: path_for_errors.to_string(),
                    reason: error.to_string(),
                }),
            StagingWriter::Local(file) => {
                file.write_all(bytes)
                    .await
                    .map_err(|error| FileDownloadError::Write {
                        path: path_for_errors.to_string(),
                        reason: error.to_string(),
                    })
            }
        }
    }

    async fn finish(self, path_for_errors: &str) -> Result<(), FileDownloadError> {
        match self {
            StagingWriter::Vfs(mut writer) => {
                writer
                    .close()
                    .await
                    .map(|_| ())
                    .map_err(|error| FileDownloadError::Write {
                        path: path_for_errors.to_string(),
                        reason: error.to_string(),
                    })
            }
            StagingWriter::Local(file) => {
                file.sync_all()
                    .await
                    .map_err(|error| FileDownloadError::Write {
                        path: path_for_errors.to_string(),
                        reason: error.to_string(),
                    })
            }
        }
    }

    /// Best-effort close on an error path; the staging file stays for resume.
    async fn abort(self) {
        match self {
            StagingWriter::Vfs(mut writer) => {
                let _ = writer.close().await;
            }
            StagingWriter::Local(mut file) => {
                let _ = file.flush().await;
            }
        }
    }
}

/// Write sink for one target: an OpenDAL operator + object keys (VFS), or
/// host-filesystem paths (engines without a mounted VFS).
enum Sink {
    Vfs {
        op: opendal::Operator,
        final_key: String,
        staging_key: String,
    },
    Local {
        final_path: PathBuf,
        staging_path: PathBuf,
    },
}

fn staging_meta_path(staging_path: &std::path::Path) -> PathBuf {
    let mut os_string = staging_path.as_os_str().to_os_string();
    os_string.push(".meta.json");
    PathBuf::from(os_string)
}

/// Liveness claim recorded in `{final}.lock` while a download owns a target.
#[derive(Serialize, Deserialize)]
struct LockContent {
    hostname: String,
    pid: u32,
    started_unix_s: u64,
}

impl LockContent {
    fn describe(&self) -> String {
        format!(
            "pid {} on {} since unix {}",
            self.pid, self.hostname, self.started_unix_s
        )
    }
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    let rc = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if rc != 0 {
        return "unknown-host".to_string();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// Human-readable holder for a TargetLocked error: the parsed claim when
/// possible, otherwise the raw bytes (a torn or foreign lock file).
fn lock_holder(held: &[u8]) -> String {
    serde_json::from_slice::<LockContent>(held)
        .map(|content| content.describe())
        .unwrap_or_else(|_| String::from_utf8_lossy(held).into_owned())
}

/// Signal-0 liveness probe: `EPERM` means alive but owned by another user.
fn pid_alive(pid: u32) -> bool {
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// `true` when a held lock's bytes name a process on this host that is
/// demonstrably dead (a crashed or killed run): the claim is stale and may
/// be reclaimed.
fn host_lock_reclaimable(held: &[u8], owner: &LockContent) -> bool {
    serde_json::from_slice::<LockContent>(held)
        .map(|content| content.hostname == owner.hostname && !pid_alive(content.pid))
        .unwrap_or(false)
}

/// Ownership of a successfully claimed target lock. The explicit
/// [`LockGuard::release`] consumes the guard; if the download future is
/// dropped (cancelled) before that, `Drop` removes the claim anyway — a
/// cancelled run must not leave its own live pid holding the target and
/// blocking every retry with `TargetLocked`.
struct LockGuard(Option<GuardInner>);

enum GuardInner {
    /// A `{final}.lock` host file (local filesystem or fs-backed VFS).
    HostFile(std::path::PathBuf),
    /// A claim object under `{final_key}.lock/` on a backend without host
    /// paths (object stores).
    Claim { op: opendal::Operator, key: String },
}

impl LockGuard {
    async fn release(mut self) {
        if let Some(inner) = self.0.take() {
            match inner {
                GuardInner::HostFile(path) => {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                GuardInner::Claim { op, key } => {
                    let _ = op.delete(&key).await;
                }
            }
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let Some(inner) = self.0.take() else {
            return;
        };
        match inner {
            // Sync removal works from any context, cancellation included.
            GuardInner::HostFile(path) => {
                let _ = std::fs::remove_file(&path);
            }
            GuardInner::Claim { op, key } => {
                // Async delete needs a runtime: spawn best-effort when one
                // is ambient. Outside any runtime the claim leaks — it
                // records hostname/pid, so dead-owner reclaim (or manual
                // removal) settles it on a later run.
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let _ = op.delete(&key).await;
                    });
                }
            }
        }
    }
}

/// A strong validator per RFC 9110 §13.1.5 / §8.8.2.2, as an If-Range value:
/// an entity-tag without the `W/` weak prefix; or — only when the response
/// carried NO entity-tag at all — a Last-Modified date that the response's
/// own `Date` header places at least 60 seconds in the past (a modification
/// date newer than that cannot be trusted to distinguish revisions). A weak
/// entity-tag can never be upgraded: when the server sent one it is the
/// revision identity, and falling back to the date is forbidden. Strength is
/// judged against the recorded response `Date`, never a local clock —
/// elapsed time must not promote a validator that was weak when served.
/// `None` means the staged prefix cannot be safely continued and must
/// restart from zero.
fn strong_validator(
    etag: Option<&str>,
    last_modified: Option<&str>,
    response_date: Option<&str>,
) -> Option<String> {
    if let Some(etag) = etag.map(str::trim).filter(|etag| !etag.is_empty()) {
        let weak = etag.starts_with("W/") || etag.starts_with("w/");
        return (!weak).then(|| etag.to_string());
    }
    let modified_raw = last_modified?.trim();
    let modified = chrono::DateTime::parse_from_rfc2822(modified_raw).ok()?;
    let served = chrono::DateTime::parse_from_rfc2822(response_date?.trim()).ok()?;
    let margin = served
        .with_timezone(&chrono::Utc)
        .signed_duration_since(modified.with_timezone(&chrono::Utc));
    (margin.num_seconds() >= 60).then(|| modified_raw.to_string())
}

impl Sink {
    fn describe(&self) -> String {
        match self {
            Sink::Vfs { final_key, .. } => format!("vfs://{final_key}"),
            Sink::Local { final_path, .. } => final_path.display().to_string(),
        }
    }

    async fn staging_len(&self) -> Result<Option<u64>, FileDownloadError> {
        match self {
            Sink::Vfs {
                op, staging_key, ..
            } => match op.stat(staging_key).await {
                Ok(meta) => Ok(Some(meta.content_length())),
                Err(error) if error.kind() == opendal::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(FileDownloadError::Write {
                    path: staging_key.clone(),
                    reason: error.to_string(),
                }),
            },
            Sink::Local { staging_path, .. } => match tokio::fs::metadata(staging_path).await {
                Ok(meta) => Ok(Some(meta.len())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(FileDownloadError::Write {
                    path: staging_path.display().to_string(),
                    reason: error.to_string(),
                }),
            },
        }
    }

    async fn delete_staging(&self) {
        match self {
            Sink::Vfs {
                op, staging_key, ..
            } => {
                let _ = op.delete(staging_key).await;
                let _ = op.delete(&format!("{staging_key}.meta.json")).await;
            }
            Sink::Local { staging_path, .. } => {
                let _ = tokio::fs::remove_file(staging_path).await;
                let _ = tokio::fs::remove_file(staging_meta_path(staging_path)).await;
            }
        }
    }

    async fn output_exists(&self) -> Result<bool, FileDownloadError> {
        match self {
            Sink::Vfs { op, final_key, .. } => match op.stat(final_key).await {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == opendal::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(FileDownloadError::Write {
                    path: final_key.clone(),
                    reason: error.to_string(),
                }),
            },
            Sink::Local { final_path, .. } => Ok(final_path.exists()),
        }
    }

    /// Host-filesystem layout (final, staging, lock) when the bytes land on a
    /// local directory — directly for [`Sink::Local`], or through the opendal
    /// `fs` backend's root for a VFS sink. `None` for remote object-store
    /// backends, which have neither exclusive-create nor link primitives.
    fn host_paths(&self) -> Option<(PathBuf, PathBuf, PathBuf)> {
        match self {
            Sink::Local {
                final_path,
                staging_path,
            } => {
                let mut lock = final_path.as_os_str().to_os_string();
                lock.push(".lock");
                Some((
                    final_path.clone(),
                    staging_path.clone(),
                    PathBuf::from(lock),
                ))
            }
            Sink::Vfs {
                op,
                final_key,
                staging_key,
            } => {
                let info = op.info();
                if info.scheme() != "fs" {
                    return None;
                }
                // Backend keys are absolute ("/a/b"); joining an absolute
                // path onto the fs root would discard the root entirely, so
                // strip to a relative segment first.
                let root = PathBuf::from(info.root());
                let relative = |key: &str| PathBuf::from(key.trim_start_matches('/'));
                Some((
                    root.join(relative(final_key)),
                    root.join(relative(staging_key)),
                    root.join(relative(&format!("{final_key}.lock"))),
                ))
            }
        }
    }

    /// Claim the target for one download. On filesystems (local paths and
    /// fs-backed VFS mounts, via [`Sink::host_paths`]) the claim is an
    /// exclusive `O_EXCL` create of `{final}.lock` — two concurrent runs can
    /// never both hold it. Backends without host paths have no exclusive
    /// create at all, so they run a claim election (see
    /// [`Sink::acquire_claim_lock`]). A pre-existing winner that names a
    /// dead process on this host is reclaimed; anything else fails fast
    /// with [`FileDownloadError::TargetLocked`]. Cross-host staleness is
    /// not guessable and is reported for manual removal.
    async fn acquire_lock(&self, owner: &LockContent) -> Result<LockGuard, FileDownloadError> {
        if let Some((_final, _staging, lock_host)) = self.host_paths() {
            // The lock is created before any staging writer runs, so the
            // parent directories of a fresh target must exist now: a
            // download into a not-yet-existing directory used to fail with
            // ENOENT on the lock itself.
            if let Some(parent) = lock_host.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|error| {
                    FileDownloadError::Write {
                        path: parent.display().to_string(),
                        reason: format!("cannot create lock parent directory: {error}"),
                    }
                })?;
            }
            let payload = serde_json::to_vec(owner).map_err(|error| FileDownloadError::Write {
                path: "<lock>".into(),
                reason: error.to_string(),
            })?;
            for _round in 0..2 {
                // Round 1: take the lock; round 2 (only after reclaiming a
                // dead-owner lock): take it for ourselves.
                match tokio::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&lock_host)
                    .await
                {
                    Ok(mut file) => {
                        file.write_all(&payload).await.map_err(|error| {
                            FileDownloadError::Write {
                                path: lock_host.display().to_string(),
                                reason: error.to_string(),
                            }
                        })?;
                        return Ok(LockGuard(Some(GuardInner::HostFile(lock_host))));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let held = tokio::fs::read(&lock_host).await.map_err(|error| {
                            FileDownloadError::Write {
                                path: lock_host.display().to_string(),
                                reason: error.to_string(),
                            }
                        })?;
                        if !host_lock_reclaimable(&held, owner) {
                            return Err(FileDownloadError::TargetLocked {
                                path: self.describe(),
                                holder: lock_holder(&held),
                            });
                        }
                        let _ = tokio::fs::remove_file(&lock_host).await;
                    }
                    Err(error) => {
                        return Err(FileDownloadError::Write {
                            path: lock_host.display().to_string(),
                            reason: error.to_string(),
                        });
                    }
                }
            }
            return Err(FileDownloadError::Write {
                path: self.describe(),
                reason: "lock kept being recreated while reclaiming it".into(),
            });
        }
        let Sink::Vfs { op, final_key, .. } = self else {
            unreachable!("Local sinks always have host paths");
        };
        self.acquire_claim_lock(op, final_key, owner).await
    }

    /// Claim election for backends without host paths (object stores).
    /// Every contender writes a uniquely-named claim object under
    /// `{final_key}.lock/`; the lexicographically smallest claim key holds
    /// the lock (a zero-padded start time makes the ordering chronological,
    /// with hostname/pid/sequence as tie-breakers). Losers delete their own
    /// claim and fail with `TargetLocked`. Unlike a stat-then-write "lock",
    /// two concurrent contenders can never both observe themselves as the
    /// holder.
    async fn acquire_claim_lock(
        &self,
        op: &opendal::Operator,
        final_key: &str,
        owner: &LockContent,
    ) -> Result<LockGuard, FileDownloadError> {
        static CLAIM_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = CLAIM_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let host_slug: String = owner
            .hostname
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let claim_key = format!(
            "{final_key}.lock/{:020}-{host_slug}-{}-{seq}",
            owner.started_unix_s, owner.pid
        );
        let payload = serde_json::to_vec(owner).map_err(|error| FileDownloadError::Write {
            path: "<lock>".into(),
            reason: error.to_string(),
        })?;
        op.write(&claim_key, payload)
            .await
            .map_err(|error| FileDownloadError::Write {
                path: claim_key.clone(),
                reason: error.to_string(),
            })?;
        let prefix = format!("{final_key}.lock/");
        for _round in 0..4 {
            let claims = op
                .list(&prefix)
                .await
                .map_err(|error| FileDownloadError::Write {
                    path: prefix.clone(),
                    reason: format!("cannot list lock claims: {error}"),
                })?;
            // Backends disagree on leading slashes in listed paths; compare
            // on the normalized form so the winner check cannot silently
            // fail for everyone.
            let Some(winner) = claims
                .iter()
                .min_by_key(|meta| meta.path().trim_start_matches('/'))
            else {
                // Our claim was written one step ago; an empty listing means
                // this backend's list is not read-after-write. Retry a few
                // rounds rather than assume exclusivity.
                continue;
            };
            let winner_key = winner.path();
            if winner_key.trim_start_matches('/') == claim_key.trim_start_matches('/') {
                return Ok(LockGuard(Some(GuardInner::Claim {
                    op: op.clone(),
                    key: claim_key,
                })));
            }
            let held = op
                .read(winner_key)
                .await
                .map_err(|error| FileDownloadError::Write {
                    path: winner_key.to_string(),
                    reason: error.to_string(),
                })?
                .to_vec();
            if !host_lock_reclaimable(&held, owner) {
                let _ = op.delete(&claim_key).await;
                return Err(FileDownloadError::TargetLocked {
                    path: self.describe(),
                    holder: lock_holder(&held),
                });
            }
            // Dead-owner claim: evict it and re-run the election.
            let _ = op.delete(winner_key).await;
        }
        let _ = op.delete(&claim_key).await;
        Err(FileDownloadError::Write {
            path: self.describe(),
            reason: "lock election did not settle within 4 rounds".into(),
        })
    }

    /// Best-effort free-space probe for the pre-flight disk budget check.
    /// `None` when the target is not on a probeable local filesystem (a
    /// remote object-store backend) — the streaming byte cap still guards.
    fn available_bytes(&self) -> Option<u64> {
        let probe: std::path::PathBuf = match self {
            Sink::Local { final_path, .. } => final_path
                .parent()
                .unwrap_or(std::path::Path::new("/"))
                .to_path_buf(),
            Sink::Vfs { op, .. } => {
                let info = op.info();
                if info.scheme() != "fs" {
                    return None;
                }
                std::path::PathBuf::from(info.root())
            }
        };
        statvfs_available(&probe)
    }

    async fn read_staging_meta(&self) -> Option<StagingMeta> {
        match self {
            Sink::Vfs {
                op, staging_key, ..
            } => {
                let bytes = op
                    .read(&format!("{staging_key}.meta.json"))
                    .await
                    .ok()?
                    .to_vec();
                serde_json::from_slice(&bytes).ok()
            }
            Sink::Local { staging_path, .. } => {
                let bytes = tokio::fs::read(staging_meta_path(staging_path))
                    .await
                    .ok()?;
                serde_json::from_slice(&bytes).ok()
            }
        }
    }

    async fn write_staging_meta(&self, meta: &StagingMeta) -> Result<(), FileDownloadError> {
        let bytes = serde_json::to_vec_pretty(meta).map_err(|error| FileDownloadError::Write {
            path: "<staging meta>".into(),
            reason: error.to_string(),
        })?;
        match self {
            Sink::Vfs {
                op, staging_key, ..
            } => {
                let key = format!("{staging_key}.meta.json");
                op.write(&key, bytes)
                    .await
                    .map_err(|error| FileDownloadError::Write {
                        path: key,
                        reason: error.to_string(),
                    })?;
            }
            Sink::Local { staging_path, .. } => {
                tokio::fs::write(staging_meta_path(staging_path), &bytes)
                    .await
                    .map_err(|error| FileDownloadError::Write {
                        path: staging_meta_path(staging_path).display().to_string(),
                        reason: error.to_string(),
                    })?;
            }
        }
        Ok(())
    }

    /// Feed the existing staging prefix (`resumed_from` bytes) into `hasher`
    /// with bounded re-reads, then append the streamed body and flush.
    /// Returns the total byte count and the streamed SHA256.
    async fn stream_body(
        &self,
        response: reqwest::Response,
        resumed_from: u64,
        max_bytes: u64,
        idle_timeout: Duration,
        url: &str,
    ) -> Result<(u64, String), FileDownloadError> {
        let mut hasher = Sha256::new();
        if resumed_from > 0 {
            self.hash_staging_prefix(resumed_from, &mut hasher).await?;
        }

        let writer = self.open_staging_writer(resumed_from > 0).await?;
        let mut writer = writer;
        let staging_desc = match self {
            Sink::Vfs { staging_key, .. } => staging_key.clone(),
            Sink::Local { staging_path, .. } => staging_path.display().to_string(),
        };

        let mut written = resumed_from;
        let mut stream = response.bytes_stream();
        loop {
            let next = tokio::time::timeout(idle_timeout, stream.next()).await;
            let chunk = match next {
                Err(_) => {
                    writer.abort().await;
                    return Err(FileDownloadError::IdleTimeout {
                        url: url.to_string(),
                        secs: idle_timeout.as_secs(),
                    });
                }
                Ok(None) => break,
                Ok(Some(Err(source))) => {
                    writer.abort().await;
                    return Err(FileDownloadError::Request {
                        url: url.to_string(),
                        source,
                    });
                }
                Ok(Some(Ok(chunk))) => chunk,
            };
            written += chunk.len() as u64;
            if written > max_bytes {
                writer.abort().await;
                return Err(FileDownloadError::TooLarge {
                    url: url.to_string(),
                    max_bytes,
                });
            }
            hasher.update(&chunk);
            writer.write_all(&chunk, &staging_desc).await?;
        }
        writer.finish(&staging_desc).await?;
        Ok((written, format!("{:x}", hasher.finalize())))
    }

    /// Re-read the staged prefix into `hasher` in bounded chunks (ranged
    /// reads for VFS, a buffered reader for the local sink).
    async fn hash_staging_prefix(
        &self,
        len: u64,
        hasher: &mut Sha256,
    ) -> Result<(), FileDownloadError> {
        match self {
            Sink::Vfs {
                op, staging_key, ..
            } => {
                let mut offset = 0u64;
                while offset < len {
                    let end = (offset + RESUME_REREAD_CHUNK).min(len);
                    let chunk =
                        op.read_with(staging_key)
                            .range(offset..end)
                            .await
                            .map_err(|error| FileDownloadError::Write {
                                path: staging_key.clone(),
                                reason: error.to_string(),
                            })?;
                    hasher.update(chunk.to_vec());
                    offset = end;
                }
            }
            Sink::Local { staging_path, .. } => {
                let file = tokio::fs::File::open(staging_path).await.map_err(|error| {
                    FileDownloadError::Write {
                        path: staging_path.display().to_string(),
                        reason: error.to_string(),
                    }
                })?;
                let mut reader =
                    tokio::io::BufReader::with_capacity(RESUME_REREAD_CHUNK as usize, file);
                let mut offset = 0u64;
                while offset < len {
                    let want = RESUME_REREAD_CHUNK.min(len - offset) as usize;
                    let mut buffer = vec![0u8; want];
                    let read = reader.read(&mut buffer).await.map_err(|error| {
                        FileDownloadError::Write {
                            path: staging_path.display().to_string(),
                            reason: error.to_string(),
                        }
                    })?;
                    if read == 0 {
                        return Err(FileDownloadError::Write {
                            path: staging_path.display().to_string(),
                            reason: format!(
                                "staging file truncated at {offset} while resuming from {len}"
                            ),
                        });
                    }
                    hasher.update(&buffer[..read]);
                    offset += read as u64;
                }
            }
        }
        Ok(())
    }

    /// Open the staging object for append (`resuming`) or truncate-create.
    async fn open_staging_writer(
        &self,
        resuming: bool,
    ) -> Result<StagingWriter, FileDownloadError> {
        match self {
            Sink::Vfs {
                op, staging_key, ..
            } => {
                let writer =
                    op.writer_with(staging_key)
                        .append(resuming)
                        .await
                        .map_err(|error| FileDownloadError::Write {
                            path: staging_key.clone(),
                            reason: error.to_string(),
                        })?;
                Ok(StagingWriter::Vfs(writer))
            }
            Sink::Local { staging_path, .. } => {
                if let Some(parent) = staging_path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|error| {
                        FileDownloadError::Write {
                            path: staging_path.display().to_string(),
                            reason: format!("cannot create `{}`: {error}", parent.display()),
                        }
                    })?;
                }
                let file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .append(resuming)
                    .truncate(!resuming)
                    .open(staging_path)
                    .await
                    .map_err(|error| FileDownloadError::Write {
                        path: staging_path.display().to_string(),
                        reason: error.to_string(),
                    })?;
                Ok(StagingWriter::Local(file))
            }
        }
    }

    /// Publish staging -> final, then drop the staging meta.
    ///
    /// `overwrite=false` uses a **no-clobber link**: if the target appeared
    /// after the pre-flight existence check (a concurrent download that held
    /// the lock first, or a foreign writer), the link fails with `EEXIST`,
    /// the verified staging is dropped, and nothing of ours replaces what is
    /// already there. `overwrite=true` is a plain rename. On object-store
    /// backends without a link primitive this degrades to stat-then-rename
    /// with the residual race accepted in the open (fs-backed mounts, the
    /// production case, always take the link path via [`Sink::host_paths`]).
    async fn publish(&self, overwrite: bool) -> Result<(), FileDownloadError> {
        let describe_io_error = |error: std::io::Error| FileDownloadError::Write {
            path: self.describe(),
            reason: format!("publish failed: {error}"),
        };
        if let Some((final_host, staging_host, _lock)) = self.host_paths() {
            if overwrite {
                tokio::fs::rename(&staging_host, &final_host)
                    .await
                    .map_err(describe_io_error)?;
            } else {
                match tokio::fs::hard_link(&staging_host, &final_host).await {
                    Ok(()) => {
                        let _ = tokio::fs::remove_file(&staging_host).await;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        // Someone published first; our staging is moot.
                        self.delete_staging().await;
                        return Err(FileDownloadError::AlreadyExists {
                            path: self.describe(),
                        });
                    }
                    Err(error) => return Err(describe_io_error(error)),
                }
            }
            self.delete_staging_meta().await;
            return Ok(());
        }
        let Sink::Vfs {
            op,
            final_key,
            staging_key,
        } = self
        else {
            unreachable!("Local sinks always have host paths");
        };
        if !overwrite && op.stat(final_key).await.is_ok() {
            self.delete_staging().await;
            return Err(FileDownloadError::AlreadyExists {
                path: self.describe(),
            });
        }
        op.rename(staging_key, final_key)
            .await
            .map_err(|error| FileDownloadError::Write {
                path: final_key.clone(),
                reason: format!("rename staging into place failed: {error}"),
            })?;
        self.delete_staging_meta().await;
        Ok(())
    }

    /// Remove only the `{staging}.meta.json` sidecar (after a publish moved
    /// the staging file itself).
    async fn delete_staging_meta(&self) {
        match self {
            Sink::Vfs {
                op, staging_key, ..
            } => {
                let _ = op.delete(&format!("{staging_key}.meta.json")).await;
            }
            Sink::Local { staging_path, .. } => {
                let _ = tokio::fs::remove_file(staging_meta_path(staging_path)).await;
            }
        }
    }

    /// Best-effort removal of the published final object. Only used to undo
    /// our own no-clobber publish when the paired manifest finalization
    /// failed moments later — the target lock is still held, so the final
    /// link is provably ours to remove.
    async fn remove_final(&self) {
        match self {
            Sink::Vfs { op, final_key, .. } => {
                let _ = op.delete(final_key).await;
            }
            Sink::Local { final_path, .. } => {
                let _ = tokio::fs::remove_file(final_path).await;
            }
        }
    }
}

/// statvfs(2)-based available-space probe (bytes available to unprivileged
/// writes). `None` when the call fails for any reason.
fn statvfs_available(path: &std::path::Path) -> Option<u64> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) };
    if rc != 0 {
        return None;
    }
    Some(stats.f_bavail as u64 * stats.f_frsize as u64)
}

/// reqwest's own Display for redirect failures hides the policy message (it
/// lives in the source chain); surface it so refusals explain themselves.
fn describe_request_error(url: &str, source: reqwest::Error) -> FileDownloadError {
    if source.is_redirect() {
        let reason = std::error::Error::source(&source)
            .map(|cause| cause.to_string())
            .unwrap_or_else(|| "no reason given".to_string());
        return FileDownloadError::RedirectRefused {
            url: url.to_string(),
            reason,
        };
    }
    FileDownloadError::Request {
        url: url.to_string(),
        source,
    }
}

fn parse_url(url: &str) -> Result<reqwest::Url, FileDownloadError> {
    let parsed = reqwest::Url::parse(url).map_err(|error| FileDownloadError::InvalidSpec {
        message: format!("cannot parse url `{url}`: {error}"),
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(FileDownloadError::InvalidSpec {
            message: format!(
                "unsupported scheme `{}` in `{url}`; only http and https are allowed",
                parsed.scheme()
            ),
        });
    }
    Ok(parsed)
}

fn validate_spec(spec: &FileDownloadSpec) -> Result<(), FileDownloadError> {
    if spec.max_bytes == 0 {
        return Err(FileDownloadError::InvalidSpec {
            message: "max_bytes must be > 0".into(),
        });
    }
    if spec.retries > 8 {
        return Err(FileDownloadError::InvalidSpec {
            message: format!("retries must be <= 8, got {}", spec.retries),
        });
    }
    if spec.idle_timeout_secs == 0 {
        return Err(FileDownloadError::InvalidSpec {
            message: "idle_timeout_secs must be > 0".into(),
        });
    }
    if let Some(expected) = spec.expected_sha256.as_deref() {
        let normalized = expected.trim().to_ascii_lowercase();
        if normalized.len() != 64 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(FileDownloadError::InvalidSpec {
                message: format!("expected_sha256 must be 64 hex characters, got `{expected}`"),
            });
        }
    }
    if let Some(expected) = spec.expected_bytes
        && expected > spec.max_bytes
    {
        return Err(FileDownloadError::InvalidSpec {
            message: format!(
                "expected_bytes ({expected}) exceeds max_bytes ({})",
                spec.max_bytes
            ),
        });
    }
    parse_url(&spec.url)?;
    if !(spec.path.starts_with("vfs://")
        || spec.path.starts_with("file://")
        || std::path::Path::new(&spec.path).is_absolute())
    {
        return Err(FileDownloadError::InvalidSpec {
            message: format!(
                "path must be a `vfs://` URI or an absolute path, got `{}`",
                spec.path
            ),
        });
    }
    Ok(())
}

fn data_file_ref(path: &str, size: u64, sha256: &str) -> FileRef {
    let format = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    FileRef {
        path: path.to_string(),
        format,
        fingerprint: Some(FileFingerprint {
            size,
            mtime_ns: 0,
            content_hash: Some(format!("sha256:{sha256}")),
            // The output lands in engine-local mutable storage (the VFS is a
            // local mount, not an immutable object store), and the source URL
            // is not guaranteed immutable either — so the fingerprint must
            // NOT claim `immutable_remote`. With the recorded sha256 the
            // freshness check recomputes the digest and a rewritten file is
            // correctly judged changed instead of being served from cache.
            immutable_remote: false,
        }),
    }
}

impl FileDownloadNode {
    pub fn new(spec: FileDownloadSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
        }
    }

    /// Execute against an explicit allowlist instead of the environment
    /// variable (test seam; `None` behaves like an unset variable).
    async fn execute_with_allowlist(
        &mut self,
        node_ctx: &NodeCtx,
        allowlist: Option<Vec<String>>,
    ) -> Result<PortOutputs, FileDownloadError> {
        validate_spec(&self.spec)?;
        let entries = match allowlist {
            Some(entries) => entries,
            None => load_allowlist_from_env()?,
        };
        let url = parse_url(&self.spec.url)?;
        ensure_host_allowed(&url, &entries)?;

        let normalized = crate::file_to_dataframe::normalize_path(&self.spec.path);
        let path = crate::file_to_dataframe::source_path(node_ctx, &normalized);

        let sink = if path.starts_with("vfs://") {
            let storage = node_ctx
                .opendal
                .as_ref()
                .ok_or_else(|| FileDownloadError::Write {
                    path: path.clone(),
                    reason: "vfs:// output requires mounted object storage".into(),
                })?;
            let virtual_path = path.strip_prefix("vfs://").unwrap_or(&path).to_string();
            let op = storage.resolve(&virtual_path);
            let final_key = storage.resolve_path(&virtual_path);
            let staging_key = format!("{final_key}.part");
            Sink::Vfs {
                op,
                final_key,
                staging_key,
            }
        } else {
            let final_path = PathBuf::from(path.strip_prefix("file://").unwrap_or(path.as_str()));
            let mut staging = final_path.clone().into_os_string();
            staging.push(".part");
            Sink::Local {
                final_path,
                staging_path: PathBuf::from(staging),
            }
        };

        if !self.spec.overwrite && sink.output_exists().await? {
            return Err(FileDownloadError::AlreadyExists { path });
        }

        let idle_timeout = Duration::from_secs(self.spec.idle_timeout_secs);
        let client = reqwest::Client::builder()
            .redirect(redirect_policy(entries))
            .build()
            .map_err(|source| describe_request_error(&self.spec.url, source))?;

        let started_unix = unix_now();
        let lock_owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: started_unix,
        };
        // One downloader per target: concurrent runs fail fast instead of
        // racing staging files and clobbering each other's publish. The
        // guard owns the claim — normal exits release it explicitly, and a
        // cancelled future drops it (removing the claim) instead of leaving
        // a live pid holding the target.
        let guard = sink.acquire_lock(&lock_owner).await?;
        let result = self
            .download_locked(
                node_ctx,
                &client,
                &sink,
                &url,
                &path,
                idle_timeout,
                started_unix,
            )
            .await;
        guard.release().await;
        result
    }

    /// Download phases after the target lock is held: attempts, verification,
    /// ledger write, publish. The existence check repeats here because the
    /// pre-lock check raced with any previous lock holder's publish.
    #[allow(clippy::too_many_arguments)]
    async fn download_locked(
        &mut self,
        node_ctx: &NodeCtx,
        client: &reqwest::Client,
        sink: &Sink,
        url: &reqwest::Url,
        path: &str,
        idle_timeout: Duration,
        started_unix: u64,
    ) -> Result<PortOutputs, FileDownloadError> {
        if !self.spec.overwrite && sink.output_exists().await? {
            return Err(FileDownloadError::AlreadyExists { path: path.into() });
        }

        let mut attempts = 0u32;
        let max_attempts = 1 + self.spec.retries;
        let mut last_error: Option<String> = None;
        let completed = loop {
            attempts += 1;
            match self.attempt(&client, &sink, &url, idle_timeout).await {
                AttemptOutcome::Done(completed) => break completed,
                AttemptOutcome::Fatal(error) => return Err(error),
                AttemptOutcome::Retry(reason) => {
                    last_error.get_or_insert(reason);
                    if attempts >= max_attempts {
                        return Err(FileDownloadError::AttemptsExhausted {
                            url: self.spec.url.clone(),
                            attempts,
                            last: last_error.unwrap_or_else(|| "unknown".into()),
                        });
                    }
                    let backoff = MAX_BACKOFF_MS.min(500 * (1u64 << (attempts - 1)));
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
            }
        };

        // Verification gate: nothing publishes until every check passes.
        if let Some(expected) = self.spec.expected_bytes
            && expected != completed.bytes
        {
            sink.delete_staging().await;
            return Err(FileDownloadError::SizeMismatch {
                url: self.spec.url.clone(),
                path: path.to_string(),
                expected,
                actual: completed.bytes,
            });
        }
        if let Some(expected) = self
            .spec
            .expected_sha256
            .as_deref()
            .map(|value| value.trim().to_ascii_lowercase())
        {
            if expected != completed.sha256 {
                sink.delete_staging().await;
                return Err(FileDownloadError::ChecksumMismatch {
                    url: self.spec.url.clone(),
                    path: path.to_string(),
                    expected,
                    actual: completed.sha256,
                });
            }
        }

        // Paired publish. The manifest is staged beside the data first; the
        // data publish is the commit point of the pair; the staged manifest
        // replaces the old one only after the data is ours. Every failure
        // path leaves either a complete pair (old or new) or nothing at
        // all — never old data described by a rewritten manifest, and never
        // our data published when the ledger entry for it could not be
        // written:
        //   * staged-manifest write fails  -> nothing published;
        //   * data publish refused (EEXIST)-> staged manifest dropped, the
        //     pre-existing data+manifest pair untouched;
        //   * manifest finalize fails (no
        //     overwrite)                    -> our just-linked data is
        //     unlinked again, so a retry is never blocked;
        //   * manifest finalize fails (overwrite) -> the old data is already
        //     replaced, so the manifest is written directly as a last resort.
        let manifest_path = format!("{path}.download.json");
        let staged_manifest = format!("{manifest_path}.part");
        let manifest = json!({
            "kind": FILE_DOWNLOAD_KIND,
            "path": path,
            "url": self.spec.url,
            "final_url": completed.final_url,
            "http_status": completed.http_status,
            "etag": completed.etag,
            "last_modified": completed.last_modified,
            "date": completed.date,
            "accession": self.spec.provenance.accession,
            "release": self.spec.provenance.release,
            "source": self.spec.provenance.source,
            "bytes": completed.bytes,
            "sha256": format!("sha256:{}", completed.sha256),
            "resumed_from": if completed.resumed_from > 0 {
                json!(completed.resumed_from)
            } else {
                serde_json::Value::Null
            },
            "attempts": attempts,
            "started_unix_s": started_unix,
            "completed_unix_s": unix_now(),
            "expected_sha256": self.spec.expected_sha256,
            "expected_bytes": self.spec.expected_bytes,
        });
        write_manifest(node_ctx, &staged_manifest, &manifest).await?;

        if let Err(error) = sink.publish(self.spec.overwrite).await {
            delete_manifest(node_ctx, &staged_manifest).await;
            return Err(error);
        }
        if let Err(error) = finalize_manifest(node_ctx, &staged_manifest, &manifest_path).await {
            if !(self.spec.overwrite
                && write_manifest(node_ctx, &manifest_path, &manifest)
                    .await
                    .is_ok())
            {
                if !self.spec.overwrite {
                    // Undo our own no-clobber publish: without its ledger
                    // entry the data must not stay at the final path.
                    sink.remove_final().await;
                }
                delete_manifest(node_ctx, &staged_manifest).await;
                return Err(error);
            }
        }

        let data_ref = data_file_ref(path, completed.bytes, &completed.sha256);
        let manifest_ref = FileRef {
            path: manifest_path,
            format: Some("json".into()),
            fingerprint: None,
        };
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, data_ref);
        outputs.insert_file(1, manifest_ref);
        Ok(outputs)
    }

    /// One request/response cycle against the target, with safe resume.
    async fn attempt(
        &self,
        client: &reqwest::Client,
        sink: &Sink,
        url: &reqwest::Url,
        idle_timeout: Duration,
    ) -> AttemptOutcome {
        // Safe resume needs a staging prefix PLUS a strong validator recorded
        // for this exact URL (RFC 9110 §13.1.5/§8.8.2.2: a weak `W/"…"` ETag
        // is never strong and forbids date fallback; a Last-Modified date is
        // strong only when the response's own Date placed it ≥60s in the
        // past — never merely because local time has moved on).
        // A prefix without one is unusable — the server could have served a
        // different revision — so it is dropped and the download restarts
        // from zero rather than append to an unverifiable prefix.
        let meta = if self.spec.resume {
            sink.read_staging_meta().await
        } else {
            None
        };
        let staged_len = sink.staging_len().await.ok().flatten().unwrap_or(0);
        let mut resumed_from = 0u64;
        let mut range_validator: Option<String> = None;
        if staged_len > 0 {
            match meta.as_ref() {
                Some(meta) if meta.url == self.spec.url => {
                    if let Some(validator) = strong_validator(
                        meta.etag.as_deref(),
                        meta.last_modified.as_deref(),
                        meta.date.as_deref(),
                    ) {
                        resumed_from = staged_len;
                        range_validator = Some(validator);
                    } else {
                        sink.delete_staging().await;
                    }
                }
                // No meta at all, or meta recorded for a different URL.
                _ => sink.delete_staging().await,
            }
        }

        let mut request = client.get(url.clone());
        if let Some(validator) = range_validator.as_ref() {
            request = request
                .header(reqwest::header::IF_RANGE, validator)
                .header(reqwest::header::RANGE, format!("bytes={resumed_from}-"));
        }

        // The idle timeout guards the header wait too: a server that accepts
        // and never answers is the same stall as one that stops mid-body.
        let response = match tokio::time::timeout(idle_timeout, request.send()).await {
            Err(_) => {
                return AttemptOutcome::Retry(format!(
                    "no response headers within {}s (idle timeout); staging kept for resume",
                    idle_timeout.as_secs()
                ));
            }
            Ok(Ok(response)) => response,
            Ok(Err(source)) => {
                let described = describe_request_error(&self.spec.url, source);
                return match described {
                    // A policy refusal is terminal, not a network hiccup.
                    redirect_refused @ FileDownloadError::RedirectRefused { .. } => {
                        AttemptOutcome::Fatal(redirect_refused)
                    }
                    other => AttemptOutcome::Retry(other.to_string()),
                };
            }
        };
        let status = response.status();
        if !status.is_success() {
            let error = FileDownloadError::HttpStatus {
                url: self.spec.url.clone(),
                status,
            };
            let retryable = status.is_server_error()
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE;
            if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                // The staged prefix no longer matches the resource.
                sink.delete_staging().await;
            }
            return if retryable {
                AttemptOutcome::Retry(error.to_string())
            } else {
                AttemptOutcome::Fatal(error)
            };
        }

        let etag = header_string(&response, reqwest::header::ETAG);
        let last_modified = header_string(&response, reqwest::header::LAST_MODIFIED);
        let response_date = header_string(&response, reqwest::header::DATE);
        let final_url = response.url().clone();
        let content_range = header_string(&response, reqwest::header::CONTENT_RANGE);

        let mut content_total: Option<u64> = response.content_length();
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            // 206: the Content-Range offset must equal the staged prefix,
            // otherwise this body is not a continuation of it.
            match content_range.as_deref().and_then(parse_content_range_start) {
                Some(start) if start == resumed_from => {
                    content_total = content_range.as_deref().and_then(parse_content_range_total);
                }
                _ => {
                    sink.delete_staging().await;
                    return AttemptOutcome::Retry(format!(
                        "resume refused: server Content-Range does not start at the staged \
                         offset {resumed_from}"
                    ));
                }
            }
        } else if resumed_from > 0 {
            // 200 despite a Range request: the validator changed (or the
            // server ignores ranges) — restart from zero, never append.
            sink.delete_staging().await;
            resumed_from = 0;
        }

        if let Some(total) = content_total {
            if total > self.spec.max_bytes {
                sink.delete_staging().await;
                return AttemptOutcome::Fatal(FileDownloadError::TooLarge {
                    url: self.spec.url.clone(),
                    max_bytes: self.spec.max_bytes,
                });
            }
            if let Some(expected) = self.spec.expected_bytes
                && total != expected
            {
                sink.delete_staging().await;
                return AttemptOutcome::Fatal(FileDownloadError::SizeMismatch {
                    url: self.spec.url.clone(),
                    path: self.spec.path.clone(),
                    expected,
                    actual: total,
                });
            }
            // Disk budget pre-flight: known requirement, probeable free space.
            if let Some(available) = sink.available_bytes() {
                let needed = total.saturating_sub(resumed_from);
                if available < needed {
                    return AttemptOutcome::Fatal(FileDownloadError::DiskBudget {
                        path: sink.describe(),
                        required: needed,
                        available,
                    });
                }
            }
        }

        // Record the validator before any body byte streams, so a cancelled
        // attempt leaves a resumable staging state.
        let _ = sink
            .write_staging_meta(&StagingMeta {
                url: self.spec.url.clone(),
                etag: etag.clone(),
                last_modified: last_modified.clone(),
                date: response_date.clone(),
            })
            .await;

        match sink
            .stream_body(
                response,
                resumed_from,
                self.spec.max_bytes,
                idle_timeout,
                &self.spec.url,
            )
            .await
        {
            Ok((bytes, sha256)) => {
                if let Some(total) = content_total
                    && bytes != total
                {
                    return AttemptOutcome::Retry(format!(
                        "truncated body: received {bytes} of {total} bytes (staging kept \
                         for resume)"
                    ));
                }
                AttemptOutcome::Done(Box::new(CompletedBody {
                    bytes,
                    sha256,
                    final_url: final_url.to_string(),
                    http_status: status.as_u16(),
                    etag,
                    last_modified,
                    date: response_date,
                    resumed_from,
                }))
            }
            Err(FileDownloadError::TooLarge { url, max_bytes }) => {
                sink.delete_staging().await;
                AttemptOutcome::Fatal(FileDownloadError::TooLarge { url, max_bytes })
            }
            Err(
                error @ (FileDownloadError::IdleTimeout { .. } | FileDownloadError::Request { .. }),
            ) => {
                AttemptOutcome::Retry(format!("attempt failed: {error} (staging kept for resume)"))
            }
            Err(error) => AttemptOutcome::Fatal(error),
        }
    }
}

fn header_string(
    response: &reqwest::Response,
    header: reqwest::header::HeaderName,
) -> Option<String> {
    response
        .headers()
        .get(header)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Parse the start offset from `Content-Range: bytes S-E/T`.
fn parse_content_range_start(header: &str) -> Option<u64> {
    let after_bytes = header.trim().strip_prefix("bytes ")?;
    after_bytes.split('-').next()?.trim().parse().ok()
}

/// Parse the total size from `Content-Range: bytes S-E/T` (`*` -> None).
fn parse_content_range_total(header: &str) -> Option<u64> {
    let total = header
        .trim()
        .strip_prefix("bytes ")?
        .split('/')
        .nth(1)?
        .trim();
    if total == "*" {
        return None;
    }
    total.parse().ok()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Write the manifest sidecar next to the published artifact.
async fn write_manifest(
    node_ctx: &NodeCtx,
    path: &str,
    manifest: &serde_json::Value,
) -> Result<(), FileDownloadError> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| FileDownloadError::Write {
        path: path.to_string(),
        reason: error.to_string(),
    })?;
    if let Some(virtual_path) = path.strip_prefix("vfs://") {
        let storage = node_ctx
            .opendal
            .as_ref()
            .ok_or_else(|| FileDownloadError::Write {
                path: path.to_string(),
                reason: "vfs:// manifest requires mounted object storage".into(),
            })?;
        let op = storage.resolve(virtual_path);
        op.write(&storage.resolve_path(virtual_path), bytes)
            .await
            .map_err(|error| FileDownloadError::Write {
                path: path.to_string(),
                reason: error.to_string(),
            })?;
        return Ok(());
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| FileDownloadError::Write {
                path: path.to_string(),
                reason: format!("cannot create `{}`: {error}", parent.display()),
            })?;
    }
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| FileDownloadError::Write {
            path: path.to_string(),
            reason: error.to_string(),
        })
}

/// Best-effort removal of a manifest path (staged or final) on either
/// storage flavor.
async fn delete_manifest(node_ctx: &NodeCtx, path: &str) {
    if let Some(virtual_path) = path.strip_prefix("vfs://") {
        if let Some(storage) = node_ctx.opendal.as_ref() {
            let op = storage.resolve(virtual_path);
            let _ = op.delete(&storage.resolve_path(virtual_path)).await;
        }
        return;
    }
    let _ = tokio::fs::remove_file(path).await;
}

/// Atomically move the staged manifest onto its final path. Both paths
/// derive from the same output target, so they share a storage flavor.
async fn finalize_manifest(
    node_ctx: &NodeCtx,
    staged: &str,
    final_path: &str,
) -> Result<(), FileDownloadError> {
    if let Some(virtual_staged) = staged.strip_prefix("vfs://") {
        let storage = node_ctx
            .opendal
            .as_ref()
            .ok_or_else(|| FileDownloadError::Write {
                path: staged.to_string(),
                reason: "vfs:// manifest requires mounted object storage".into(),
            })?;
        let virtual_final = final_path.strip_prefix("vfs://").unwrap_or(final_path);
        storage
            .resolve(virtual_staged)
            .rename(
                &storage.resolve_path(virtual_staged),
                &storage.resolve_path(virtual_final),
            )
            .await
            .map_err(|error| FileDownloadError::Write {
                path: final_path.to_string(),
                reason: format!("manifest finalize failed: {error}"),
            })?;
        return Ok(());
    }
    tokio::fs::rename(staged, final_path)
        .await
        .map_err(|error| FileDownloadError::Write {
            path: final_path.to_string(),
            reason: format!("manifest finalize failed: {error}"),
        })
}

#[async_trait]
impl DagNode for FileDownloadNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        FILE_DOWNLOAD_KIND
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        self.execute_with_allowlist(node_ctx, None)
            .await
            .map_err(Into::into)
    }
}

impl NodeFactory for FileDownloadNodeFactory {
    fn kind(&self) -> &'static str {
        FILE_DOWNLOAD_KIND
    }

    fn desc(&self) -> &'static str {
        "Streams an allowlisted http(s) URL to disk with resume, checksum \
        verification, and an atomic publish."
    }

    fn doc(&self) -> &'static str {
        "The large-file sibling of `http_fetch`: the same deny-by-default network \
        allowlist gates the initial URL and every redirect hop, but the body is \
        never buffered in memory — bytes stream to a staging object in chunks, \
        SHA256 is computed while streaming, and the file is published only after \
        the full body arrived, the byte count matched `expected_bytes` (when \
        given), and the SHA256 matched `expected_sha256` (when given). \
        Concurrency: a `{path}.lock` claim serializes downloads of one target — \
        an exclusive create on filesystem-backed sinks, a claim election on \
        object stores — so a second run fails fast with a TargetLocked error \
        instead of clobbering (a claim left by a dead same-host process is \
        reclaimed, and a cancelled run drops its claim rather than leaving its \
        live pid holding the target). The `overwrite=false` publish is a \
        no-clobber link, so a target that appeared mid-run is never replaced. \
        Ordering: the `<path>.download.json` sidecar manifest (original and \
        final URL, status, validator headers, accession/release, bytes, SHA256, \
        resume offset, attempts, timestamps) is staged beside the data, the \
        data publish is the pair's commit point, and the staged manifest \
        replaces the old one only after the data is ours — a refused publish \
        leaves the pre-existing data+manifest pair untouched, and a \
        manifest-finalize failure unwinds our own publish so a retry is never \
        blocked. Resume: a cancelled attempt leaves a `.part` staging file \
        that the next run continues only when the recorded validator is \
        strong per RFC 9110 §13.1.5 — a non-weak ETag, or (only when the \
        response carried no ETag at all) a Last-Modified at least 60s before \
        the response's own Date — sent as If-Range; anything else restarts \
        from zero. Outputs: port 0 is the downloaded File (fingerprint carries \
        the content hash and deliberately does NOT claim immutable_remote: \
        the target sits in mutable local storage and a rewritten file must \
        invalidate the cache), port 1 `manifest`. Network errors, 5xx, 429, \
        408 and mid-stream resets retry (`retries`, default 3) with backoff; \
        a stalled stream trips `idle_timeout_secs` (default 120) — there is \
        no total-time timeout. Disk budget is checked pre-flight when the \
        total size is known and the target sits on a probeable local \
        filesystem."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FileDownloadSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: FileDownloadSpec = serde_json::from_value(spec)?;
        Ok(Box::new(FileDownloadNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(url: &str, path: &str) -> FileDownloadSpec {
        FileDownloadSpec {
            url: url.to_string(),
            path: path.to_string(),
            expected_sha256: None,
            expected_bytes: None,
            max_bytes: 1024 * 1024,
            retries: 3,
            idle_timeout_secs: 10,
            resume: true,
            overwrite: false,
            provenance: DownloadProvenance::default(),
        }
    }

    fn ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    /// Serve `responses` in order, one per accepted connection. Returns the
    /// port and a handle yielding the raw request each connection sent (so
    /// tests can assert on Range/If-Range headers).
    async fn scripted_server(
        responses: Vec<Vec<u8>>,
    ) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 8192];
                let _ = socket.read(&mut buffer).await;
                requests.push(String::from_utf8_lossy(&buffer).into_owned());
                let _ = socket.write_all(&response).await;
            }
            requests
        });
        (port, handle)
    }

    fn file_output_of(outputs: &PortOutputs, port: u8) -> &FileRef {
        match outputs.get(&port).unwrap() {
            dag_core::value::NodeValue::File(file) => file,
            other => panic!("expected a File output on port {port}, got {other:?}"),
        }
    }

    #[test]
    fn spec_validation_rejects_bad_urls_scales_and_paths() {
        assert!(validate_spec(&spec("ftp://ebi.ac.uk/x", "/out.bin")).is_err());
        assert!(validate_spec(&spec("file:///etc/passwd", "/out.bin")).is_err());
        assert!(validate_spec(&spec("https://ebi.ac.uk/f", "relative/out.bin")).is_err());
        assert!(validate_spec(&spec("https://ebi.ac.uk/f", "vfs:///dir/out.bin")).is_ok());
        assert!(validate_spec(&spec("https://ebi.ac.uk/f", "file:///tmp/out.bin")).is_ok());
        let mut bad_sha = spec("https://ebi.ac.uk/f", "/out.bin");
        bad_sha.expected_sha256 = Some("zz".into());
        assert!(validate_spec(&bad_sha).is_err());
        let mut many = spec("https://ebi.ac.uk/f", "/out.bin");
        many.retries = 9;
        assert!(validate_spec(&many).is_err());
        let mut idle = spec("https://ebi.ac.uk/f", "/out.bin");
        idle.idle_timeout_secs = 0;
        assert!(validate_spec(&idle).is_err());
        let mut zero = spec("https://ebi.ac.uk/f", "/out.bin");
        zero.max_bytes = 0;
        assert!(validate_spec(&zero).is_err());
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(parse_content_range_start("bytes 5-9/10"), Some(5));
        assert_eq!(parse_content_range_total("bytes 5-9/10"), Some(10));
        assert_eq!(parse_content_range_total("bytes 5-9/*"), None);
        assert_eq!(parse_content_range_start("octets 5-9/10"), None);
        assert_eq!(parse_content_range_total("nope"), None);
    }

    #[tokio::test]
    async fn download_fails_without_an_allowlist() {
        let out = tempfile::tempdir().unwrap().keep().join("f.bin");
        let mut node = FileDownloadNode::new(spec("https://ebi.ac.uk/x", &out.to_string_lossy()));
        let error = node.execute_with_allowlist(&ctx(), None).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no HTTP fetch allowlist is configured"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn download_refuses_hosts_off_the_allowlist() {
        let out = tempfile::tempdir().unwrap().keep().join("f.bin");
        let mut node =
            FileDownloadNode::new(spec("https://example.org/d.tsv", &out.to_string_lossy()));
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["ebi.ac.uk".into()]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("example.org"), "{error}");
        assert!(!out.exists());
    }

    #[tokio::test]
    async fn success_publishes_file_fingerprint_and_manifest() {
        let body = b"0123456789";
        let sha = format!("{:x}", Sha256::digest(body));
        let (port, _server) = scripted_server(vec![format!(
            "HTTP/1.1 200 OK\r\ncontent-length: 10\r\netag: \"v1\"\r\nconnection: close\r\n\r\n{}",
            String::from_utf8_lossy(body),
        )
        .into_bytes()])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("payload.tsv");
        let mut download = spec(
            &format!("http://127.0.0.1:{port}/p.tsv"),
            &out.to_string_lossy(),
        );
        download.expected_sha256 = Some(sha.clone());
        download.expected_bytes = Some(10);
        download.provenance = DownloadProvenance {
            accession: Some("GCST90000061".into()),
            release: Some("v1".into()),
            source: Some("GWAS Catalog".into()),
        };
        let mut node = FileDownloadNode::new(download);
        let outputs = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();

        assert_eq!(std::fs::read(&out).unwrap(), body);
        // Staging leftovers must be gone after the atomic publish.
        assert!(!dir.join("payload.tsv.part").exists());
        assert!(!dir.join("payload.tsv.part.meta.json").exists());
        assert!(!dir.join("payload.tsv.download.json.part").exists());

        let file = file_output_of(&outputs, 0);
        assert_eq!(file.path, out.to_string_lossy());
        assert_eq!(file.format.as_deref(), Some("tsv"));
        let fingerprint = file.fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, 10);
        assert_eq!(
            fingerprint.content_hash.as_deref(),
            Some(format!("sha256:{sha}").as_str())
        );
        // Mutable local storage: the fingerprint must not short-circuit
        // freshness checks.
        assert!(!fingerprint.immutable_remote);

        let manifest = file_output_of(&outputs, 1);
        let manifest_path = dir.join("payload.tsv.download.json");
        assert_eq!(manifest.path, manifest_path.to_string_lossy());
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["bytes"], serde_json::json!(10));
        assert_eq!(
            manifest["sha256"],
            serde_json::json!(format!("sha256:{sha}"))
        );
        assert_eq!(manifest["http_status"], serde_json::json!(200));
        assert_eq!(manifest["etag"], serde_json::json!("\"v1\""));
        assert_eq!(manifest["accession"], serde_json::json!("GCST90000061"));
        assert_eq!(manifest["release"], serde_json::json!("v1"));
        assert_eq!(manifest["source"], serde_json::json!("GWAS Catalog"));
        assert_eq!(manifest["attempts"], serde_json::json!(1));
        assert_eq!(
            manifest["url"],
            serde_json::json!(format!("http://127.0.0.1:{port}/p.tsv"))
        );
        assert!(manifest["resumed_from"].is_null());
    }

    #[tokio::test]
    async fn checksum_mismatch_publishes_nothing() {
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("bad.bin");
        let mut download = spec(
            &format!("http://127.0.0.1:{port}/b.bin"),
            &out.to_string_lossy(),
        );
        download.expected_sha256 = Some("0".repeat(64));
        let mut node = FileDownloadNode::new(download);
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
        assert!(!out.exists(), "a mismatched file must never be published");
        assert!(
            !dir.join("bad.bin.part").exists(),
            "staging is dropped on mismatch"
        );
        assert!(!dir.join("bad.bin.download.json").exists());
    }

    #[tokio::test]
    async fn redirects_must_stay_on_the_allowlist() {
        let (port, _server) = scripted_server(vec![b"HTTP/1.1 302 Found\r\ncontent-length: 0\r\nlocation: http://localhost:9/x\r\nconnection: close\r\n\r\n".to_vec()])
            .await;
        let out = tempfile::tempdir().unwrap().keep().join("jump.bin");
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/jump"),
            &out.to_string_lossy(),
        ));
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(
            matches!(error, FileDownloadError::RedirectRefused { .. }),
            "{error}"
        );
        assert!(!out.exists());
    }

    #[tokio::test]
    async fn truncated_attempt_resumes_with_range_and_if_range() {
        // Connection 1: declared 10 bytes, sent 5, then the socket dies —
        // a mid-stream truncation that must NOT be published.
        // Connection 2: 206 continuing at offset 5 for the same ETag.
        let body: &[u8] = b"0123456789";
        let sha = format!("{:x}", Sha256::digest(body));
        let (port, server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\netag: \"v1\"\r\n\r\n01234".to_vec(),
            b"HTTP/1.1 206 Partial Content\r\ncontent-length: 5\r\ncontent-range: bytes 5-9/10\r\netag: \"v1\"\r\nconnection: close\r\n\r\n56789".to_vec(),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("resume.bin");
        let mut download = spec(
            &format!("http://127.0.0.1:{port}/r.bin"),
            &out.to_string_lossy(),
        );
        download.expected_sha256 = Some(sha.clone());
        let mut node = FileDownloadNode::new(download);
        let outputs = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();

        assert_eq!(std::fs::read(&out).unwrap(), body);
        assert!(!dir.join("resume.bin.part").exists());

        let requests = server.await.unwrap();
        assert!(requests.len() >= 2, "{requests:?}");
        let resume_request = requests[1].to_ascii_lowercase();
        assert!(
            resume_request.contains("range: bytes=5-"),
            "resume must send a Range header for the staged prefix: {resume_request}"
        );
        assert!(
            resume_request.contains("if-range:"),
            "resume must guard the prefix with If-Range: {resume_request}"
        );

        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("resume.bin.download.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["resumed_from"], serde_json::json!(5));
        assert_eq!(manifest["bytes"], serde_json::json!(10));
        assert_eq!(
            manifest["sha256"],
            serde_json::json!(format!("sha256:{sha}"))
        );
        let _ = outputs;
    }

    #[tokio::test]
    async fn server_error_then_success_retries_to_completion() {
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .to_vec(),
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("retry.bin");
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/retry"),
            &out.to_string_lossy(),
        ));
        node.execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("retry.bin.download.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["attempts"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn oversized_body_is_refused_and_publishes_nothing() {
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\nconnection: close\r\n\r\n0123456789"
                .to_vec(),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("big.bin");
        let mut download = spec(
            &format!("http://127.0.0.1:{port}/big"),
            &out.to_string_lossy(),
        );
        download.max_bytes = 50;
        let mut node = FileDownloadNode::new(download);
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("max_bytes (50)"), "{error}");
        assert!(!out.exists());
        assert!(!dir.join("big.bin.part").exists());
    }

    #[tokio::test]
    async fn existing_output_requires_overwrite() {
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("exists.bin");
        std::fs::write(&out, b"stale").unwrap();
        let mut node =
            FileDownloadNode::new(spec("http://127.0.0.1:9/none", &out.to_string_lossy()));
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(
            matches!(error, FileDownloadError::AlreadyExists { .. }),
            "{error}"
        );
        assert_eq!(std::fs::read(&out).unwrap(), b"stale");

        // With overwrite=true the same target is replaced.
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let mut download = spec(
            &format!("http://127.0.0.1:{port}/fresh"),
            &out.to_string_lossy(),
        );
        download.overwrite = true;
        let mut node = FileDownloadNode::new(download);
        node.execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
    }

    #[tokio::test]
    async fn stalled_stream_trips_the_idle_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 4096];
            let _ = socket.read(&mut buffer).await;
            // Promise 10 bytes, deliver the headers, then stall mid-body.
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\n01234")
                .await;
            tokio::time::sleep(Duration::from_secs(3)).await;
        });
        let out = tempfile::tempdir().unwrap().keep().join("stall.bin");
        let mut download = spec(
            &format!("http://127.0.0.1:{port}/stall"),
            &out.to_string_lossy(),
        );
        download.idle_timeout_secs = 1;
        download.retries = 0;
        let mut node = FileDownloadNode::new(download);
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("stalled for 1s"), "{error}");
        assert!(error.to_string().contains("after 1 attempt"), "{error}");
        assert!(!out.exists());
    }

    #[tokio::test]
    async fn manifest_write_failure_publishes_no_data() {
        // The ledger entry is staged before the data publishes; if the staged
        // manifest cannot be written, nothing lands at the final path and a
        // plain retry is never blocked by a half-published output.
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("payload.bin");
        // An unwritable staged-manifest target: a directory where the file
        // goes.
        std::fs::create_dir(dir.join("payload.bin.download.json.part")).unwrap();
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/payload"),
            &out.to_string_lossy(),
        ));
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("download.json"), "{error}");
        assert!(
            !out.exists(),
            "a manifest write failure must not publish data to the final path"
        );
    }

    #[tokio::test]
    async fn rewritten_vfs_download_invalidates_cache() {
        // The fingerprint must not claim immutable_remote: the artifact sits
        // in mutable local storage, so a rewritten object has to be judged
        // changed by the recorded sha256, not served from cache.
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        let context = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage.clone()),
        );
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/payload"),
            "vfs:///payload.bin",
        ));
        let outputs = node
            .execute_with_allowlist(&context, Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        let file = file_output_of(&outputs, 0);
        assert!(!file.fingerprint.as_ref().unwrap().immutable_remote);
        storage
            .resolve("/payload.bin")
            .write(&storage.resolve_path("/payload.bin"), "other".to_string())
            .await
            .unwrap();
        let changed =
            dag_core::fingerprint::cached_file_changed(file, Some(storage.as_ref())).await;
        assert!(
            changed,
            "a rewritten downloaded VFS object must invalidate the cached output"
        );
    }

    #[tokio::test]
    async fn weak_etag_is_not_a_strong_validator() {
        // A W/"…" ETag must never be sent as If-Range — not directly, and not
        // by falling back to a Last-Modified that would on its own qualify:
        // the staged prefix is dropped and the download restarts from zero
        // instead of appending to an unvalidatable prefix.
        let (port, server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\netag: W/\"v1\"\r\nconnection: close\r\n\r\n0123456789"
                .to_vec(),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("resume.bin");
        let url = format!("http://127.0.0.1:{port}/r.bin");
        std::fs::write(dir.join("resume.bin.part"), b"01234").unwrap();
        // The recorded date pair ALONE would be a strong validator (143s
        // margin); the weak ETag's presence must veto the fallback.
        std::fs::write(
            dir.join("resume.bin.part.meta.json"),
            serde_json::to_vec(&StagingMeta {
                url: url.clone(),
                etag: Some("W/\"v1\"".into()),
                last_modified: Some("Sun, 06 Nov 1994 08:49:37 GMT".into()),
                date: Some("Sun, 06 Nov 1994 08:51:00 GMT".into()),
            })
            .unwrap(),
        )
        .unwrap();
        let mut node = FileDownloadNode::new(spec(&url, &out.to_string_lossy()));
        node.execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        let requests = server.await.unwrap();
        let first = requests[0].to_ascii_lowercase();
        assert!(
            !first.contains("range:"),
            "no Range may be sent without a strong validator: {first}"
        );
        assert!(
            !first.contains("if-range:"),
            "a weak ETag must never be sent as If-Range: {first}"
        );
        assert_eq!(std::fs::read(&out).unwrap(), b"0123456789");
    }

    #[test]
    fn strong_validator_classification() {
        let old = "Sun, 06 Nov 1994 08:49:37 GMT";
        let served = "Sun, 06 Nov 1994 08:51:00 GMT"; // 143s after `old`
        // Non-weak ETags are strong; W/ (either case) is not.
        assert_eq!(
            strong_validator(Some("\"v1\""), None, None),
            Some("\"v1\"".to_string())
        );
        assert_eq!(strong_validator(Some("W/\"v1\""), None, None), None);
        assert_eq!(strong_validator(Some("w/\"v1\""), None, None), None);
        // A weak ETag vetoes date fallback even when the date pair alone
        // would qualify: the server's revision identity cannot be upgraded.
        assert_eq!(
            strong_validator(Some("W/\"v1\""), Some(old), Some(served)),
            None
        );
        // No ETag at all: Last-Modified is strong only relative to the
        // response's own Date (>=60s before it), never the local clock.
        assert_eq!(
            strong_validator(None, Some(old), Some(served)),
            Some(old.to_string())
        );
        let thirty = "Sun, 06 Nov 1994 08:50:30 GMT"; // 30s before `served`
        assert_eq!(strong_validator(None, Some(thirty), Some(served)), None);
        // Elapsed wall time must not promote a date that was weak when it
        // was served: both timestamps decades old, margin still 30s.
        let now_served = chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let now_thirty = (chrono::Utc::now() - chrono::Duration::seconds(30))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        assert_eq!(
            strong_validator(None, Some(&now_thirty), Some(&now_served)),
            None
        );
        // Without the response Date the strength cannot be established.
        assert_eq!(strong_validator(None, Some(old), None), None);
        // Unparseable values are not validators.
        assert_eq!(
            strong_validator(None, Some("yesterday"), Some(served)),
            None
        );
        assert_eq!(strong_validator(None, Some(old), Some("now")), None);
    }

    #[tokio::test]
    async fn parallel_downloads_cannot_both_publish() {
        // Two concurrent overwrite=false runs on one target: the lock lets
        // exactly one finish; the other fails fast with TargetLocked and the
        // final bytes belong to the winner alone.
        let (port_a, server_a) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let (port_b, server_b) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nother".to_vec(),
        ])
        .await;
        let out = tempfile::tempdir().unwrap().keep().join("shared.bin");
        let mut a = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port_a}/a"),
            &out.to_string_lossy(),
        ));
        let mut b = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port_b}/b"),
            &out.to_string_lossy(),
        ));
        let context_a = ctx();
        let context_b = ctx();
        let (result_a, result_b) = tokio::join!(
            a.execute_with_allowlist(&context_a, Some(vec!["127.0.0.1".into()])),
            b.execute_with_allowlist(&context_b, Some(vec!["127.0.0.1".into()])),
        );
        let loser = match (&result_a, &result_b) {
            (Ok(_), Err(error)) => error,
            (Err(error), Ok(_)) => error,
            (Err(_), Err(_)) => panic!("one of the two downloads must succeed"),
            (Ok(_), Ok(_)) => panic!("both overwrite=false writers succeeded"),
        };
        assert!(
            matches!(loser, FileDownloadError::TargetLocked { .. }),
            "the losing run must fail on the target lock, got: {loser}"
        );
        let bytes = std::fs::read(&out).unwrap();
        assert!(
            bytes == b"hello" || bytes == b"other",
            "final bytes must belong to the winner alone, got {bytes:?}"
        );
        // The lock is released on exit: a later download of a fresh target
        // is not blocked by the leftover claim.
        let (port_c, server_c) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nthird".to_vec(),
        ])
        .await;
        let out2 = tempfile::tempdir().unwrap().keep().join("next.bin");
        let mut c = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port_c}/c"),
            &out2.to_string_lossy(),
        ));
        c.execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out2).unwrap(), b"third");
        let _ = (server_a, server_b, server_c);
    }

    #[tokio::test]
    async fn stale_lock_from_a_dead_process_is_reclaimed() {
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("stale.bin");
        // A lock whose owner is a dead process on this host is stale.
        std::fs::write(
            dir.join("stale.bin.lock"),
            serde_json::to_vec(&LockContent {
                hostname: hostname(),
                pid: 999_999,
                started_unix_s: 0,
            })
            .unwrap(),
        )
        .unwrap();
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/s"),
            &out.to_string_lossy(),
        ));
        node.execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        assert!(!dir.join("stale.bin.lock").exists(), "lock released");
    }

    #[tokio::test]
    async fn live_lock_blocks_the_download() {
        let out = tempfile::tempdir().unwrap().keep().join("held.bin");
        // Our own pid is definitionally alive: the lock must be honored.
        std::fs::write(
            out.with_file_name("held.bin.lock"),
            serde_json::to_vec(&LockContent {
                hostname: hostname(),
                pid: std::process::id(),
                started_unix_s: 0,
            })
            .unwrap(),
        )
        .unwrap();
        let mut node =
            FileDownloadNode::new(spec("http://127.0.0.1:9/none", &out.to_string_lossy()));
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(
            matches!(error, FileDownloadError::TargetLocked { .. }),
            "{error}"
        );
        assert!(!out.exists());
    }

    #[tokio::test]
    async fn vfs_parallel_downloads_cannot_both_publish() {
        // fs-backed VFS targets must take the same exclusive lock as local
        // paths: the pre-fix stat-then-write claim let concurrent runs both
        // "hold" the lock and clobber each other's publish.
        let (port_a, server_a) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let (port_b, server_b) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nother".to_vec(),
        ])
        .await;
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
        let context_a = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage.clone()),
        );
        let context_b = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage.clone()),
        );
        let mut a = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port_a}/a"),
            "vfs:///shared.bin",
        ));
        let mut b = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port_b}/b"),
            "vfs:///shared.bin",
        ));
        let (result_a, result_b) = tokio::join!(
            a.execute_with_allowlist(&context_a, Some(vec!["127.0.0.1".into()])),
            b.execute_with_allowlist(&context_b, Some(vec!["127.0.0.1".into()])),
        );
        let loser = match (&result_a, &result_b) {
            (Ok(_), Err(error)) | (Err(error), Ok(_)) => error,
            (Ok(_), Ok(_)) => panic!("both overwrite=false VFS writers succeeded"),
            (Err(a), Err(b)) => panic!("one of the two downloads must succeed: {a}; {b}"),
        };
        assert!(
            matches!(loser, FileDownloadError::TargetLocked { .. }),
            "the losing run must fail on the target lock, got: {loser}"
        );
        // The winner's bytes are readable through the VFS, whole and alone.
        let bytes = storage
            .resolve("/shared.bin")
            .read(&storage.resolve_path("/shared.bin"))
            .await
            .unwrap()
            .to_vec();
        assert!(
            bytes == b"hello" || bytes == b"other",
            "final VFS bytes must belong to the winner alone, got {bytes:?}"
        );
        // And no lock claim is left behind.
        let lock_stat = storage
            .resolve("/shared.bin.lock")
            .stat(&storage.resolve_path("/shared.bin.lock"))
            .await;
        assert!(lock_stat.is_err(), "the VFS lock must be released");
        let _ = (server_a, server_b);
    }

    #[tokio::test]
    async fn object_store_claim_election_excludes_concurrent_holders() {
        // Backends without host paths elect one holder by smallest claim key:
        // two concurrent acquires must yield exactly one holder, release must
        // make the target free again, a dead-owner claim is reclaimed, and a
        // live foreign pid still blocks.
        let op = opendal::Operator::new(opendal::services::Memory::default())
            .unwrap()
            .finish();
        let sink = Sink::Vfs {
            op: op.clone(),
            final_key: "/x.bin".into(),
            staging_key: "/x.bin.part".into(),
        };
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
        };
        let (first, second) = tokio::join!(sink.acquire_lock(&owner), sink.acquire_lock(&owner));
        let winner = match (first, second) {
            (Ok(guard), Err(error)) | (Err(error), Ok(guard)) => {
                assert!(
                    matches!(error, FileDownloadError::TargetLocked { .. }),
                    "{error}"
                );
                guard
            }
            (Ok(_), Ok(_)) => panic!("claim election let two callers hold the lock"),
            (Err(a), Err(b)) => panic!("election failed both callers: {a}; {b}"),
        };
        winner.release().await;
        // Free after release: a plain re-acquire succeeds.
        sink.acquire_lock(&owner).await.unwrap().release().await;

        // A claim from a dead same-host pid sorts first and is reclaimed...
        let dead = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: 999_999,
            started_unix_s: 1,
        })
        .unwrap();
        op.write("/x.bin.lock/00000000000000000001-dead-999999-0", dead)
            .await
            .unwrap();
        sink.acquire_lock(&owner).await.unwrap().release().await;

        // ...but a live pid holding the earliest claim blocks us.
        let live = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 2,
        })
        .unwrap();
        op.write("/x.bin.lock/00000000000000000002-live-1-0", live)
            .await
            .unwrap();
        let error = match sink.acquire_lock(&owner).await {
            Err(error) => error,
            Ok(guard) => {
                guard.release().await;
                panic!("a live foreign pid must block the claim election");
            }
        };
        assert!(
            matches!(error, FileDownloadError::TargetLocked { .. }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn cancelled_download_releases_the_target_lock() {
        // Dropping the download future mid-flight (cancellation) must drop
        // the target lock too — a live pid left holding it would block every
        // retry with TargetLocked.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 4096];
            let _ = socket.read(&mut buffer).await;
            // Accept the request and never answer: the node sits waiting for
            // headers while the test cancels it.
            tokio::time::sleep(Duration::from_secs(120)).await;
        });
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("cancel.bin");
        let lock = dir.join("cancel.bin.lock");
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/c"),
            &out.to_string_lossy(),
        ));
        {
            // The pinned future (and its lock guard) must live inside this
            // scope: leaving it cancels the download and runs the guard's
            // Drop, which removes the host lock synchronously.
            let context = ctx();
            let mut download = std::pin::pin!(
                node.execute_with_allowlist(&context, Some(vec!["127.0.0.1".into()]))
            );
            // Drive the future until the lock claim appears (finishing early
            // would defeat the test).
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !lock.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "lock never appeared; download state stuck before claiming"
                );
                match tokio::time::timeout(Duration::from_millis(50), download.as_mut()).await {
                    Ok(_) => panic!("download completed before it could be cancelled"),
                    Err(_) => {}
                }
            }
            assert!(lock.exists());
            assert!(!out.exists());
        }
        assert!(
            !lock.exists(),
            "a cancelled run must not leave its live pid holding the lock"
        );

        // A fresh download of the same target is not blocked.
        let (port2, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let mut retry = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port2}/c"),
            &out.to_string_lossy(),
        ));
        retry
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
    }

    #[tokio::test]
    async fn download_into_a_new_directory_creates_it() {
        // The lock file is created before any staging writer runs, so a
        // target inside a not-yet-existing directory used to fail with
        // ENOENT on the lock itself. Parent directories must be created
        // first, preserving the auto-create behaviour of plain writes.
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let base = tempfile::tempdir().unwrap().keep();
        let out = base.join("new/subdir/f.bin");
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/n"),
            &out.to_string_lossy(),
        ));
        node.execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        assert!(base.join("new/subdir/f.bin.download.json").exists());
    }

    #[tokio::test]
    async fn publish_rejection_leaves_existing_pair_intact() {
        // A foreign writer publishes the target while our download is in
        // flight: our publish is refused with AlreadyExists and the
        // pre-existing data+manifest pair must survive byte-for-byte — the
        // manifest must NOT be rewritten to describe our unpublished bytes.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("shared.bin");
        let manifest = dir.join("shared.bin.download.json");
        let foreign_target = out.clone();
        let foreign_manifest = manifest.clone();
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 4096];
            let _ = socket.read(&mut buffer).await;
            // The foreign writer wins the target while our request is in
            // flight, and records its own ledger entry.
            std::fs::write(&foreign_target, b"FOREIGN").unwrap();
            std::fs::write(&foreign_manifest, b"{\"sha256\": \"foreign\"}\n").unwrap();
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
                )
                .await;
        });
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/s"),
            &out.to_string_lossy(),
        ));
        let error = node
            .execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(
            matches!(error, FileDownloadError::AlreadyExists { .. }),
            "{error}"
        );
        assert_eq!(
            std::fs::read(&out).unwrap(),
            b"FOREIGN",
            "the foreign publish must survive untouched"
        );
        assert_eq!(
            std::fs::read(&manifest).unwrap(),
            b"{\"sha256\": \"foreign\"}\n",
            "a refused publish must not rewrite the existing manifest"
        );
        assert!(!dir.join("shared.bin.download.json.part").exists());
    }
}
