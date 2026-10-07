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
//! the next run when the server offers a strong validator (ETag or
//! Last-Modified, sent as If-Range) — otherwise the download restarts from
//! zero rather than append to an unverifiable prefix.
//!
//! A sidecar manifest (`<path>.download.json`) records provenance for the
//! run ledger: accession/release when the caller knows them, original and
//! final URL, HTTP status, validator headers, bytes, SHA256, resume offset,
//! attempt count, and start/completion times.

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
    /// strong validator (ETag / Last-Modified); otherwise restart from zero
    /// (default true).
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
    resumed_from: u64,
}

/// Validator memory kept beside the staging file so a later attempt (or a
/// later run) can resume safely. Written when the response headers arrive,
/// before any body bytes stream.
#[derive(Serialize, Deserialize)]
struct StagingMeta {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
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

    /// Atomic publish: staging -> final, then drop the staging meta.
    async fn publish(&self) -> Result<(), FileDownloadError> {
        match self {
            Sink::Vfs {
                op,
                final_key,
                staging_key,
            } => {
                op.rename(staging_key, final_key).await.map_err(|error| {
                    FileDownloadError::Write {
                        path: final_key.clone(),
                        reason: format!("rename staging into place failed: {error}"),
                    }
                })?;
                let _ = op.delete(&format!("{staging_key}.meta.json")).await;
                Ok(())
            }
            Sink::Local {
                final_path,
                staging_path,
            } => {
                if let Some(parent) = final_path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|error| {
                        FileDownloadError::Write {
                            path: final_path.display().to_string(),
                            reason: format!("cannot create `{}`: {error}", parent.display()),
                        }
                    })?;
                }
                tokio::fs::rename(staging_path, final_path)
                    .await
                    .map_err(|error| FileDownloadError::Write {
                        path: final_path.display().to_string(),
                        reason: format!("rename staging into place failed: {error}"),
                    })?;
                let _ = tokio::fs::remove_file(staging_meta_path(staging_path)).await;
                Ok(())
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

fn data_file_ref(path: &str, size: u64, sha256: &str, immutable: bool) -> FileRef {
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
            immutable_remote: immutable,
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
                path: path.clone(),
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
                    path: path.clone(),
                    expected,
                    actual: completed.sha256,
                });
            }
        }

        sink.publish().await?;

        let manifest_path = format!("{path}.download.json");
        let manifest = json!({
            "kind": FILE_DOWNLOAD_KIND,
            "path": path,
            "url": self.spec.url,
            "final_url": completed.final_url,
            "http_status": completed.http_status,
            "etag": completed.etag,
            "last_modified": completed.last_modified,
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
        write_manifest(node_ctx, &manifest_path, &manifest).await?;

        let immutable = matches!(sink, Sink::Vfs { .. });
        let data_ref = data_file_ref(&path, completed.bytes, &completed.sha256, immutable);
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
        // for this exact URL; anything less restarts from zero (the writer
        // opens in truncate mode, so a stale prefix cannot leak into the body).
        let meta = if self.spec.resume {
            sink.read_staging_meta().await
        } else {
            None
        };
        let staged_len = sink.staging_len().await.ok().flatten().unwrap_or(0);
        let mut resumed_from = 0u64;
        let mut have_validator = false;
        if let Some(meta) = meta.as_ref() {
            let same_url = meta.url == self.spec.url;
            let strong = meta.etag.is_some() || meta.last_modified.is_some();
            if same_url && strong && staged_len > 0 {
                resumed_from = staged_len;
                have_validator = true;
            }
        }

        let mut request = client.get(url.clone());
        if have_validator {
            let meta = meta.as_ref().expect("checked above");
            let validator = meta
                .etag
                .clone()
                .or_else(|| meta.last_modified.clone())
                .unwrap_or_default();
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
        SHA256 is computed while streaming, and the file is published by an atomic \
        rename only after the full body arrived, the byte count matched \
        `expected_bytes` (when given), and the SHA256 matched `expected_sha256` \
        (when given). A cancelled or failed attempt leaves a `.part` staging file \
        that the next run resumes when the server offers a strong validator \
        (ETag / Last-Modified, sent as If-Range); without one the download \
        restarts from zero rather than risk appending to a stale prefix. A \
        `<path>.download.json` sidecar manifest records the original and final \
        URL, HTTP status, validator headers, optional accession/release, bytes, \
        SHA256, resume offset, attempts, and start/completion times for the run \
        ledger. Outputs: port 0 is the downloaded File (its fingerprint carries \
        the content hash), port 1 `manifest` is the sidecar. Network errors, \
        5xx, 429, 408 and mid-stream resets are retried (`retries`, default 3) \
        with backoff; a stalled stream trips `idle_timeout_secs` (default 120) — \
        there is no total-time timeout. Disk budget is checked pre-flight when \
        the total size is known and the target sits on a probeable local \
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

        let file = file_output_of(&outputs, 0);
        assert_eq!(file.path, out.to_string_lossy());
        assert_eq!(file.format.as_deref(), Some("tsv"));
        let fingerprint = file.fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, 10);
        assert_eq!(
            fingerprint.content_hash.as_deref(),
            Some(format!("sha256:{sha}").as_str())
        );

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
}
