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
//! claim is staged under an exclusive name and published by an atomic
//! no-clobber link, so the lock path never holds a partial claim and two
//! concurrent `overwrite=false` runs can never both publish over each other:
//! the second fails fast with [`FileDownloadError::TargetLocked`], and the
//! publish itself is a no-clobber link (a plain rename when `overwrite=true`).
//! The guard exists the instant the link lands — no await separates them — so
//! a cancelled acquisition cannot leak its own live-pid lock: aborts either
//! orphan only the staging name (which gates nothing) or land in the guarded
//! region where `Drop` removes both names.
//! Backends without host paths lock through the backend's atomic conditional
//! create of a single `{key}.lock` object (`if_not_exists` / `if_none_match`),
//! where arrival order is irrelevant; backends that offer no such primitive
//! are refused explicitly. A claim is identified by its full owner identity —
//! hostname, pid, and a per-acquisition nonce drawn from a randomly seeded
//! per-process counter — and guards only ever delete a lock whose bytes still
//! name exactly their own acquisition, so per-process nonce collisions and
//! unrecognized (torn or foreign-format) lock bytes are never mistaken for
//! one's own claim. Stale claims (naming a dead process, or our own pid with
//! a nonce this process is not executing) are reclaimed only through
//! atomically verified protocols: on filesystems the claim's exclusive
//! `flock` is the fence — a holder (any task, any process) holds `LOCK_EX`
//! for its claim's whole life, and a reclaimer must acquire that flock
//! non-blockingly, re-verify the exact stale bytes, and only then move the
//! claim aside and delete it, so the lock path is only ever emptied by the
//! one reclaimer that proved the claim holderless, and displacement of a
//! live holder is impossible by construction; on object stores, etag
//! compare-and-swap where the etag is observed BEFORE the bytes are read,
//! so the swap can only replace the very version that was judged stale.
//! Backends lacking the atomic primitive report the claim for manual
//! removal instead of racing. A fencing check before the publish aborts a
//! download whose claim was displaced mid-flight by a foreign writer.
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

use std::os::unix::io::AsRawFd;
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
/// The full identity — hostname, pid, and a per-acquisition `nonce` — is what
/// a holder compares to recognize *its own* claim, so a displaced holder
/// neither deletes a successor's lock on release nor publishes unprotected.
/// Comparing only the nonce would be wrong: the counter is per-process, so
/// two real processes each mint nonce 0 and a failed contender's guard would
/// delete the incumbent's live claim (round-4 finding).
#[derive(Clone, Serialize, Deserialize)]
struct LockContent {
    hostname: String,
    pid: u32,
    started_unix_s: u64,
    #[serde(default)]
    nonce: u64,
}

impl LockContent {
    fn describe(&self) -> String {
        format!(
            "pid {} on {} since unix {} (claim {})",
            self.pid, self.hostname, self.started_unix_s, self.nonce
        )
    }

    /// The identity comparison behind every "is this lock still mine" check.
    fn names(&self, other: &LockContent) -> bool {
        self.hostname == other.hostname && self.pid == other.pid && self.nonce == other.nonce
    }
}

/// Monotonic per-process acquisition counter — the nonce source. Seeded from
/// a random base on first use so distinct processes (and, notably, a later
/// process that happens to reuse our pid after a crash) never mint
/// overlapping nonces: a dead predecessor's leftover claim can then never
/// collide with a live acquisition's identity.
static LOCK_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_nonce() -> u64 {
    use std::sync::atomic::Ordering;
    // 0 marks "not seeded yet": publish a random nonzero base once. A lost
    // race just means the winner's base sticks — either way every nonce
    // handed out is nonzero and strictly increasing within this process.
    if LOCK_NONCE.load(Ordering::Relaxed) == 0 {
        let base = random_nonce_base();
        let _ = LOCK_NONCE.compare_exchange(0, base, Ordering::Relaxed, Ordering::Relaxed);
    }
    LOCK_NONCE.fetch_add(1, Ordering::Relaxed)
}

/// A random nonzero base for the nonce counter: a splitmix64 finisher over
/// the pid, wall-clock nanoseconds, and an address. No `rand` dependency —
/// any single live bit of entropy suffices, because the hard guarantee is
/// the full-identity compare in [`LockContent::names`]; the random base only
/// closes the pid-reuse corner where a leftover would otherwise be
/// misread as an executing acquisition.
fn random_nonce_base() -> u64 {
    let pid = std::process::id() as u64;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    let address = &LOCK_NONCE as *const _ as u64;
    let mut x = pid ^ nanos.rotate_left(20) ^ address.rotate_left(41) ^ 0x9E37_79B9_7F4A_7C15;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (x ^ (x >> 31)) | 1 // odd => nonzero
}

/// Nonces of acquisitions currently executing in THIS process. A lock naming
/// our own pid whose nonce is absent here is a claim leaked by a cancelled
/// acquisition of ours — and since no other process can share our pid,
/// removing it is race-free. Registered for the lifetime of the lock guard.
static ACTIVE_DOWNLOADS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u64>>> =
    std::sync::OnceLock::new();

fn active_downloads() -> &'static std::sync::Mutex<std::collections::HashSet<u64>> {
    ACTIVE_DOWNLOADS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Registers an acquisition nonce until dropped (owned by the lock guard).
struct ActiveDownload(u64);

impl ActiveDownload {
    fn register(nonce: u64) -> Self {
        active_downloads().lock().unwrap().insert(nonce);
        ActiveDownload(nonce)
    }
}

impl Drop for ActiveDownload {
    fn drop(&mut self) {
        active_downloads().lock().unwrap().remove(&self.0);
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

/// What a held lock's content says about its holder.
enum HeldClaim {
    /// An executing download holds the target: a foreign pid that is alive,
    /// any pid on another host, or our own pid with a nonce this process is
    /// currently executing.
    Live,
    /// Names a dead process on this host: stale, reclaimable — atomically
    /// where the backend allows it.
    DeadOwner,
    /// Names OUR pid with a nonce this process is not executing: a claim
    /// leaked by a cancelled acquisition of ours. Stale like `DeadOwner` —
    /// and reclaimed through the same atomically verified protocols, since
    /// another task of this process may already have re-acquired the target
    /// between our read and any unconditional removal.
    OwnCancelled,
    /// Bytes that are not an interpretable claim (torn or foreign format).
    Foreign,
}

fn classify_held(held: &[u8], owner: &LockContent) -> HeldClaim {
    let Ok(content) = serde_json::from_slice::<LockContent>(held) else {
        return HeldClaim::Foreign;
    };
    if content.hostname == owner.hostname && content.pid == owner.pid {
        return if active_downloads().lock().unwrap().contains(&content.nonce) {
            HeldClaim::Live
        } else {
            HeldClaim::OwnCancelled
        };
    }
    if content.hostname == owner.hostname && !pid_alive(content.pid) {
        return HeldClaim::DeadOwner;
    }
    HeldClaim::Live
}

/// `true` when the lock bytes still name OUR acquisition — the full identity
/// (hostname, pid, nonce) matches the claim we minted. Unparseable bytes are
/// never ours: our own claims are always published atomically (staged,
/// flushed, then linked or conditionally created), so they always parse;
/// bytes that do not parse are torn or foreign-format and are left for a
/// human — a guard must never delete a lock it cannot positively recognize
/// as its own (round-4 finding: nonce-only comparison let one process
/// delete another's live claim, and parse-failure-as-ours let a failed
/// contender delete a foreign lock outright).
fn content_is_ours(held: &[u8], claim: &LockContent) -> bool {
    match serde_json::from_slice::<LockContent>(held) {
        Ok(content) => content.names(claim),
        Err(_) => false,
    }
}

/// Atomically verified reclaim of a stale host lock — used for BOTH stale
/// classes (a claim naming a dead process, and a claim naming our own pid
/// with a nonce this process is not executing). The incumbent's exclusive
/// `flock` is the fence: every holder — any task in any process — holds
/// `LOCK_EX` on the claim's inode for its whole lifetime (from staging,
/// through the link, until release), so a non-blocking acquire of that flock
/// atomically proves no holder is executing RIGHT NOW, and holding it
/// excludes every other protocol participant from renaming or unlinking the
/// claim while we act. Only under the fence does the reclaimer re-verify
/// that the bytes are still the exact claim it judged stale, then move the
/// claim aside and delete it there: the lock path is only ever emptied by
/// the one reclaimer that proved the claim holderless, a successor can link
/// only after the fence is released, and displacement of a live holder is
/// impossible by construction — no restore path is needed. The previous
/// bare `remove_file` for our own cancelled claims was a read-then-delete
/// race (round-4 controlled-scheduling finding), and the earlier
/// move-aside-and-verify protocol still held a rename→restore window that
/// could orphan a just-acquired successor's claim under three-way
/// contention (caught by the 2048-round three-reclaimer regression).
async fn reclaim_stale_host_lock(
    lock_host: &std::path::Path,
    judged_stale: &[u8],
    nonce: u64,
) -> Result<(), FileDownloadError> {
    let incumbent = match std::fs::OpenOptions::new().read(true).open(lock_host) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The claim vanished between the caller's read and now; the
            // caller retries.
            return Ok(());
        }
        Err(error) => {
            return Err(FileDownloadError::Write {
                path: lock_host.display().to_string(),
                reason: format!("cannot open the stale claim to verify it: {error}"),
            });
        }
    };
    // Non-blocking: contend only with LIVE holders, never wait for them.
    let fenced = unsafe { libc::flock(incumbent.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if fenced != 0 {
        let error = std::io::Error::last_os_error();
        let current = tokio::fs::read(lock_host).await.unwrap_or_else(|_| judged_stale.to_vec());
        return match error.raw_os_error() {
            Some(libc::EWOULDBLOCK) => Err(FileDownloadError::TargetLocked {
                path: lock_host.display().to_string(),
                // Flock semantics: an open file description holds the lock
                // until its last descriptor closes — so contention here
                // means a holder (task or process) is executing NOW and the
                // earlier stale judgment is obsolete.
                holder: lock_holder(&current),
            }),
            Some(libc::ENOLCK | libc::EOPNOTSUPP) => Err(FileDownloadError::TargetLocked {
                path: lock_host.display().to_string(),
                holder: format!(
                    "{}; the filesystem offers no lock verification — remove `{}` manually",
                    lock_holder(&current),
                    lock_host.display()
                ),
            }),
            _ => Err(FileDownloadError::Write {
                path: lock_host.display().to_string(),
                reason: format!("cannot verify the stale claim holderlessly: {error}"),
            }),
        };
    }
    // We hold the fence on the incumbent's inode: no participant can move
    // it and no successor can appear at the path until we release. Verify
    // the bytes one last time — a foreign (non-participant) writer may have
    // rewritten the path between the caller's read and the fence; if so,
    // refuse to act and let the caller re-judge.
    let held_now = match tokio::fs::read(lock_host).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FileDownloadError::Write {
                path: lock_host.display().to_string(),
                reason: error.to_string(),
            });
        }
    };
    if held_now != judged_stale {
        return Ok(());
    }
    // Verified holderless and unchanged: move the stale claim aside and
    // delete it THERE. The path empties only under our fence; a successor's
    // link can only land afterwards and is never touched by us.
    let mut aside_os = lock_host.as_os_str().to_os_string();
    aside_os.push(format!(".stale-{nonce}"));
    let aside = PathBuf::from(aside_os);
    match tokio::fs::rename(lock_host, &aside).await {
        Ok(()) => {
            let _ = tokio::fs::remove_file(&aside).await;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(FileDownloadError::Write {
            path: lock_host.display().to_string(),
            reason: format!("cannot move the stale lock aside under the fence: {error}"),
        }),
    }
    // `incumbent` drops here: the fence releases only after the aside name
    // is gone, so the caller's next-round link races no one.
}

/// Ownership of a successfully claimed target lock. The explicit
/// [`LockGuard::release`] consumes the guard; if the download future is
/// dropped (cancelled) before that, `Drop` removes the claim anyway — a
/// cancelled run must not leave its own live pid holding the target and
/// blocking every retry with `TargetLocked`. Both paths first verify the
/// claim still names this exact acquisition (full identity match), so
/// releasing after a displacement never deletes the successor's lock — not
/// even under a per-process nonce collision with another process (round-4
/// finding). The guard also carries the registration of its nonce in the
/// process-wide active-download set.
struct LockGuard(Option<GuardInner>);

enum GuardInner {
    /// A `{final}.lock` host file (local filesystem or fs-backed VFS), with
    /// the staging name it was published from until that name is unlinked.
    /// `_flock` holds the exclusive fence on the claim's inode (taken BEFORE
    /// the publish link, released when the guard drops) — `None` only for
    /// guards assembled directly in tests, which exercise release identity
    /// rather than the fence.
    HostFile {
        path: std::path::PathBuf,
        temp: Option<std::path::PathBuf>,
        claim: LockContent,
        _flock: Option<std::fs::File>,
        _active: ActiveDownload,
    },
    /// The `{final_key}.lock` object on a backend without host paths
    /// (object stores), created through the backend's atomic
    /// exclusive-create.
    Claim {
        op: opendal::Operator,
        key: String,
        claim: LockContent,
        _active: ActiveDownload,
    },
}

impl LockGuard {
    /// The staging name was unlinked after the claim was linked into place;
    /// stop tracking it so release/Drop do not retry the removal.
    fn staging_consumed(&mut self) {
        if let Some(GuardInner::HostFile { temp, .. }) = self.0.as_mut() {
            *temp = None;
        }
    }

    async fn release(mut self) {
        if let Some(inner) = self.0.take() {
            match inner {
                GuardInner::HostFile {
                    path, temp, claim, ..
                } => {
                    if let Ok(held) = tokio::fs::read(&path).await
                        && content_is_ours(&held, &claim)
                    {
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                    // Else: the claim at `path` belongs to a successor now —
                    // not ours to delete.
                    if let Some(temp) = temp {
                        let _ = tokio::fs::remove_file(&temp).await;
                    }
                    // The `flock` File drops at the end of this arm: the
                    // fence releases only after our own names are gone.
                }
                GuardInner::Claim { op, key, claim, .. } => {
                    if let Ok(held) = op.read(&key).await
                        && content_is_ours(&held.to_vec(), &claim)
                    {
                        let _ = op.delete(&key).await;
                    }
                }
            }
        }
    }

    /// Fencing check: does the lock still name OUR acquisition? Called at the
    /// commit boundary (before publish) — under the flock fence the protocol
    /// itself can never displace a live holder, but a foreign (non-participant)
    /// writer touching the path must still fail the download instead of
    /// racing the new holder.
    async fn still_holds(&self) -> bool {
        let Some(inner) = self.0.as_ref() else {
            return false;
        };
        let held = match inner {
            GuardInner::HostFile { path, .. } => match tokio::fs::read(path).await {
                Ok(bytes) => bytes,
                Err(_) => return false,
            },
            GuardInner::Claim { op, key, .. } => match op.read(key).await {
                Ok(buffer) => buffer.to_vec(),
                Err(_) => return false,
            },
        };
        content_is_ours(&held, inner.claim())
    }
}

impl GuardInner {
    fn claim(&self) -> &LockContent {
        match self {
            GuardInner::HostFile { claim, .. } | GuardInner::Claim { claim, .. } => claim,
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let Some(inner) = self.0.take() else {
            return;
        };
        match inner {
            // Sync removal works from any context, cancellation included —
            // but only after re-verifying the claim still names this exact
            // acquisition. The staging name is exclusively ours (pid+nonce)
            // and always goes. The `flock` File closes with the arm's end,
            // releasing the fence after the names are removed.
            GuardInner::HostFile {
                path, temp, claim, ..
            } => {
                if let Ok(held) = std::fs::read(&path)
                    && content_is_ours(&held, &claim)
                {
                    let _ = std::fs::remove_file(&path);
                }
                if let Some(temp) = temp {
                    let _ = std::fs::remove_file(&temp);
                }
            }
            GuardInner::Claim { op, key, claim, .. } => {
                // Async verify+delete needs a runtime: spawn best-effort when
                // one is ambient. Outside any runtime the claim leaks — it
                // records hostname/pid/nonce, so own-cancelled or dead-owner
                // reclaim (or manual removal) settles it on a later run.
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Ok(held) = op.read(&key).await
                            && content_is_ours(&held.to_vec(), &claim)
                        {
                            let _ = op.delete(&key).await;
                        }
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
    /// fs-backed VFS mounts, via [`Sink::host_paths`]) the claim is staged at
    /// an exclusive `{lock}.tmp-{pid}-{nonce}` name, fenced with an exclusive
    /// `flock` taken before publication, and published by an atomic
    /// no-clobber link — two concurrent runs can never both hold the lock
    /// path, a torn claim can never sit on it, and the guard exists the
    /// instant the link lands (no await between), so a cancelled acquisition
    /// cannot leak its own live-pid lock. Backends without host paths take
    /// the atomic conditional-create path (see
    /// [`Sink::acquire_claim_lock`]). A pre-existing stale claim — one naming
    /// a dead process, or our own pid with a nonce this process is not
    /// executing — is reclaimed only under the claim's flock fence (see
    /// [`reclaim_stale_host_lock`]), which a live successor holds by
    /// construction; anything else fails fast with
    /// [`FileDownloadError::TargetLocked`]. Cross-host staleness is not
    /// guessable and is reported for manual removal.
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
            let nonce = next_nonce();
            let mut owner = owner.clone();
            owner.nonce = nonce;
            let payload = serde_json::to_vec(&owner).map_err(|error| FileDownloadError::Write {
                path: "<lock>".into(),
                reason: error.to_string(),
            })?;
            // The claim is staged under an exclusive name (pid+nonce is unique
            // per acquisition) and published by an atomic NO-CLOBBER link:
            // the lock path never holds a partial claim — it appears exactly
            // once, complete, or not at all. A cancellation before the link
            // can only orphan the staging name, which gates nothing.
            let mut claim_stage = lock_host.clone().into_os_string();
            claim_stage.push(format!(".tmp-{}-{}", std::process::id(), nonce));
            let claim_stage = PathBuf::from(claim_stage);
            for _round in 0..4 {
                match tokio::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&claim_stage)
                    .await
                {
                    Ok(mut file) => {
                        // Abort before the link can only orphan the staging
                        // name; a write failure cleans it before returning.
                        // The flush is load-bearing: tokio::fs::File buffers
                        // small writes in memory, and the link must never
                        // publish a claim whose bytes have not reached the
                        // filesystem (a zero-length claim at the lock path
                        // reads as unparseable — and would be treated as a
                        // torn remnant by every concurrent observer).
                        if let Err(error) = file.write_all(&payload).await {
                            let _ = tokio::fs::remove_file(&claim_stage).await;
                            return Err(FileDownloadError::Write {
                                path: claim_stage.display().to_string(),
                                reason: error.to_string(),
                            });
                        }
                        if let Err(error) = file.flush().await {
                            let _ = tokio::fs::remove_file(&claim_stage).await;
                            return Err(FileDownloadError::Write {
                                path: claim_stage.display().to_string(),
                                reason: error.to_string(),
                            });
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        // A remnant of an earlier acquisition that reused this
                        // pid (pid wraparound) — it gates nothing; drop it and
                        // restage in the next round.
                        let _ = tokio::fs::remove_file(&claim_stage).await;
                        continue;
                    }
                    Err(error) => {
                        return Err(FileDownloadError::Write {
                            path: claim_stage.display().to_string(),
                            reason: error.to_string(),
                        });
                    }
                }
                // The fence is taken on the claim's inode BEFORE the publish
                // link: for the claim's whole life (staging → path →
                // release) an exclusive non-blocking flock marks the
                // executing holder, so any contender — in this process or
                // another — can atomically test liveness by trying to
                // acquire it (see `reclaim_stale_host_lock`).
                let fence = match std::fs::OpenOptions::new().read(true).open(&claim_stage) {
                    Ok(file) => file,
                    Err(error) => {
                        let _ = tokio::fs::remove_file(&claim_stage).await;
                        return Err(FileDownloadError::Write {
                            path: claim_stage.display().to_string(),
                            reason: format!("cannot open the claim to fence it: {error}"),
                        });
                    }
                };
                let locked =
                    unsafe { libc::flock(fence.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if locked != 0 {
                    let error = std::io::Error::last_os_error();
                    let _ = tokio::fs::remove_file(&claim_stage).await;
                    return Err(FileDownloadError::Write {
                        path: claim_stage.display().to_string(),
                        reason: format!("cannot fence the claim: {error}"),
                    });
                }
                // The guard exists BEFORE the link can execute. An abort
                // parked inside the link await — after the syscall landed but
                // before the future resumes — still finds the guard alive:
                // its Drop removes whichever names materialized (the lock
                // only after re-verifying it is ours, the staging name
                // unconditionally) and closes the fence. There is no point
                // at which the lock path exists unowned.
                let mut guard = LockGuard(Some(GuardInner::HostFile {
                    path: lock_host.clone(),
                    temp: Some(claim_stage.clone()),
                    claim: owner.clone(),
                    _flock: Some(fence),
                    _active: ActiveDownload::register(nonce),
                }));
                match tokio::fs::hard_link(&claim_stage, &lock_host).await {
                    Ok(()) => {
                        let _ = tokio::fs::remove_file(&claim_stage).await;
                        guard.staging_consumed();
                        return Ok(guard);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        // Held: drop our staging name (the next round
                        // recreates it; the round guard's Drop would too) and
                        // inspect the incumbent claim.
                        let _ = tokio::fs::remove_file(&claim_stage).await;
                        let held = tokio::fs::read(&lock_host).await.map_err(|error| {
                            FileDownloadError::Write {
                                path: lock_host.display().to_string(),
                                reason: error.to_string(),
                            }
                        })?;
                        match classify_held(&held, &owner) {
                            HeldClaim::Live | HeldClaim::Foreign => {
                                return Err(FileDownloadError::TargetLocked {
                                    path: self.describe(),
                                    holder: lock_holder(&held),
                                });
                            }
                            // Both stale classes — dead owner and our own
                            // cancelled claim — reclaim through the same
                            // fence-verified protocol: the old bare
                            // `remove_file` for our own cancelled claims was
                            // a read-then-delete race (round-4
                            // controlled-scheduling finding).
                            HeldClaim::OwnCancelled | HeldClaim::DeadOwner => {
                                reclaim_stale_host_lock(&lock_host, &held, nonce).await?;
                            }
                        }
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
                reason: "lock did not settle within 4 rounds".into(),
            });
        }
        let Sink::Vfs { op, final_key, .. } = self else {
            unreachable!("Local sinks always have host paths");
        };
        self.acquire_claim_lock(op, final_key, owner).await
    }

    /// Atomic conditional lock for backends without host paths (object
    /// stores). The claim is a single `{final_key}.lock` object created
    /// through the backend's exclusive conditional write (`if_not_exists`,
    /// or `if_none_match("*")` where that is the advertised primitive):
    /// whoever's create lands first holds the lock, and a claim that reaches
    /// the backend late can never displace an active holder — there is no
    /// ordering to get wrong. Stale claims — dead owners and our own
    /// cancelled acquisitions alike — are taken over by compare-and-swap
    /// (`if_match` on an etag observed BEFORE the claim bytes were read, so
    /// the swap is bound to the very version that was judged stale) where
    /// the backend supports it; a backend without atomic reclaim reports
    /// the claim for manual removal instead of racing. A backend with no
    /// exclusive-create primitive at all is refused explicitly — a
    /// stat-then-write "lock" serializes nothing.
    async fn acquire_claim_lock(
        &self,
        op: &opendal::Operator,
        final_key: &str,
        owner: &LockContent,
    ) -> Result<LockGuard, FileDownloadError> {
        let capability = op.info().full_capability();
        let exclusive_create = capability.write_with_if_not_exists;
        let conditional_create = capability.write_with_if_none_match;
        let compare_and_swap = capability.write_with_if_match;
        if !exclusive_create && !conditional_create {
            return Err(FileDownloadError::Write {
                path: self.describe(),
                reason: format!(
                    "backend `{}` has no atomic exclusive-create primitive for target locks; \
                     refusing to download — concurrent runs could clobber each other",
                    op.info().scheme()
                ),
            });
        }
        let lock_key = format!("{final_key}.lock");
        let nonce = next_nonce();
        let mut owner = owner.clone();
        owner.nonce = nonce;
        let payload = serde_json::to_vec(&owner).map_err(|error| FileDownloadError::Write {
            path: "<lock>".into(),
            reason: error.to_string(),
        })?;
        let guard = |op: &opendal::Operator| {
            LockGuard(Some(GuardInner::Claim {
                op: op.clone(),
                key: lock_key.clone(),
                claim: owner.clone(),
                _active: ActiveDownload::register(nonce),
            }))
        };
        for _round in 0..4 {
            let create = if exclusive_create {
                op.write_with(&lock_key, payload.clone())
                    .if_not_exists(true)
                    .await
            } else {
                op.write_with(&lock_key, payload.clone())
                    .if_none_match("*")
                    .await
            };
            match create {
                Ok(_) => return Ok(guard(op)),
                Err(error)
                    if error.kind() == opendal::ErrorKind::ConditionNotMatch
                        || error.kind() == opendal::ErrorKind::AlreadyExists =>
                {
                    // Held: inspect below.
                }
                Err(error) if error.kind() == opendal::ErrorKind::Unsupported => {
                    return Err(FileDownloadError::Write {
                        path: self.describe(),
                        reason: format!(
                            "backend `{}` refused the conditional lock create: {error}",
                            op.info().scheme()
                        ),
                    });
                }
                Err(error) => {
                    return Err(FileDownloadError::Write {
                        path: lock_key.clone(),
                        reason: error.to_string(),
                    });
                }
            }
            // Observe the etag BEFORE the bytes: the conditional swap below
            // is then bound to a version that is exactly the one the
            // following read serves, or older — reading the bytes first and
            // stat'ing afterwards (the round-4 static finding) could pair a
            // successor's etag with a stale judgment and let the swap
            // overwrite a new live holder. With etag-first, any replacement
            // after the stat either shows up in the bytes (classified live,
            // no swap) or fails the `if_match` atomically.
            let etag = match op.stat(&lock_key).await {
                Ok(meta) => meta.etag().map(str::to_string),
                Err(error) if error.kind() == opendal::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(FileDownloadError::Write {
                        path: lock_key.clone(),
                        reason: error.to_string(),
                    });
                }
            };
            let held = match op.read(&lock_key).await {
                Ok(buffer) => buffer.to_vec(),
                Err(error) if error.kind() == opendal::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(FileDownloadError::Write {
                        path: lock_key.clone(),
                        reason: error.to_string(),
                    });
                }
            };
            match classify_held(&held, &owner) {
                HeldClaim::Live | HeldClaim::Foreign => {
                    return Err(FileDownloadError::TargetLocked {
                        path: self.describe(),
                        holder: lock_holder(&held),
                    });
                }
                // Both stale classes — dead owner and our own cancelled
                // claim — reclaim through the same etag compare-and-swap:
                // a bare delete raced identically for both (a concurrent
                // acquirer can replace the object between our read and the
                // delete, and we would remove a live successor's claim).
                HeldClaim::OwnCancelled | HeldClaim::DeadOwner => {
                    if !compare_and_swap {
                        return Err(FileDownloadError::TargetLocked {
                            path: self.describe(),
                            holder: format!(
                                "{}; the claim is stale but backend `{}` has no atomic \
                                 reclaim — remove `{}` manually",
                                lock_holder(&held),
                                op.info().scheme(),
                                lock_key
                            ),
                        });
                    }
                    let Some(etag) = etag else {
                        return Err(FileDownloadError::TargetLocked {
                            path: self.describe(),
                            holder: format!(
                                "{}; no observable etag to reclaim atomically — remove `{}` \
                                 manually",
                                lock_holder(&held),
                                lock_key
                            ),
                        });
                    };
                    match op
                        .write_with(&lock_key, payload.clone())
                        .if_match(&etag)
                        .await
                    {
                        // The object is still the exact stale claim we
                        // judged: the takeover is ours.
                        Ok(_) => return Ok(guard(op)),
                        Err(error) if error.kind() == opendal::ErrorKind::ConditionNotMatch => {
                            // Another reclaimer took it over first; retry.
                        }
                        Err(error) => {
                            return Err(FileDownloadError::Write {
                                path: lock_key.clone(),
                                reason: error.to_string(),
                            });
                        }
                    }
                }
            }
        }
        Err(FileDownloadError::Write {
            path: self.describe(),
            reason: "lock did not settle within 4 rounds".into(),
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
            // Overwritten per acquisition with the fresh nonce.
            nonce: 0,
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
                &guard,
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
        guard: &LockGuard,
    ) -> Result<PortOutputs, FileDownloadError> {
        if !self.spec.overwrite && sink.output_exists().await? {
            return Err(FileDownloadError::AlreadyExists { path: path.into() });
        }

        let mut attempts = 0u32;
        let max_attempts = 1 + self.spec.retries;
        let mut last_error: Option<String> = None;
        let completed = loop {
            attempts += 1;
            match self.attempt(client, sink, url, idle_timeout).await {
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

        // Fencing: the whole download ran under the target lock; under the
        // flock fence the reclaim protocol can no longer displace a live
        // holder, but a foreign (non-participant) writer touching the path
        // must still not let us publish over a lock that stopped naming this
        // acquisition. Fail instead — a retry is safe.
        if !guard.still_holds().await {
            return Err(FileDownloadError::TargetLocked {
                path: path.into(),
                holder: "lock displaced during the download (fencing check)".into(),
            });
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
        staged at an exclusive temp name and published by an atomic \
        no-clobber link on filesystem-backed sinks (the guard exists the \
        instant the link lands, so a cancelled acquisition leaks nothing), \
        the backend's atomic conditional create on object \
        stores (backends without that primitive are refused explicitly) — so \
        a second run fails fast with a TargetLocked error instead of \
        clobbering. A claim carries its full owner identity (hostname, pid, \
        and a randomly-seeded per-acquisition nonce) and a guard only ever \
        deletes a lock whose bytes still name its own exact acquisition — \
        per-process nonce collisions across processes and unrecognized \
        (torn or foreign-format) lock bytes are never mistaken for one's \
        own claim. Stale claims — dead owners and the canceller's own \
        leaked claims alike — are reclaimed safely: filesystem reclaimers \
        must first acquire the claim's exclusive flock — proving no holder \
        is executing, in this process or any other — and only then delete \
        the re-verified stale bytes, so a new live holder is untouchable by \
        construction; \
        object-store reclaim uses etag compare-and-swap — with the etag \
        observed before the bytes, so the swap is bound to the judged \
        version — where available and reports for manual removal otherwise. \
        A fencing check before the publish \
        aborts a download whose claim was displaced mid-flight. The \
        `overwrite=false` publish is a \
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
                nonce: 0,
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
        // Our own pid with a nonce this process IS executing: a concurrent
        // download of ours — the lock must be honored. (An unregistered
        // own-pid nonce would be our own cancelled claim, reclaimable by
        // design; the registration is what separates the two.)
        let _executing = ActiveDownload::register(4242);
        std::fs::write(
            out.with_file_name("held.bin.lock"),
            serde_json::to_vec(&LockContent {
                hostname: hostname(),
                pid: std::process::id(),
                started_unix_s: 0,
                nonce: 4242,
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_stale_reclaim_never_removes_a_new_live_lock() {
        // Round-3 review probe: two concurrent reclaimers of one stale lock
        // must never delete each other's newly acquired live lock. The
        // fence-verified reclaim keeps exactly one holder per round
        // across all 256 rounds; a naive read-then-unlink reclaimed ~1-3%.
        let dir = tempfile::tempdir().unwrap().keep();
        let final_path = dir.join("reclaim.bin");
        let lock = dir.join("reclaim.bin.lock");
        let sink = Sink::Local {
            final_path: final_path.clone(),
            staging_path: dir.join("reclaim.bin.part"),
        };
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        let dead = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: 999_999,
            started_unix_s: 1,
            nonce: 0,
        })
        .unwrap();
        let mut doubles = 0;
        for _ in 0..256 {
            std::fs::write(&lock, dead.clone()).unwrap();
            let (a, b) = tokio::join!(sink.acquire_lock(&owner), sink.acquire_lock(&owner));
            if a.is_ok() && b.is_ok() {
                doubles += 1;
            }
            // While the winner holds it, the lock file must still exist and
            // name the winner — the loser's failed reclaim must not have
            // removed it.
            let (winner, loser) = match (a, b) {
                (Ok(winner), Err(loser)) | (Err(loser), Ok(winner)) => (winner, loser),
                (Ok(_), Ok(_)) => panic!("double-held in the 256-round reclaim probe"),
                (Err(a), Err(b)) => panic!("both reclaimers failed: {a}; {b}"),
            };
            let _ = loser;
            let held = std::fs::read(&lock).unwrap();
            let content: LockContent = serde_json::from_slice(&held).unwrap();
            assert!(
                active_downloads().lock().unwrap().contains(&content.nonce),
                "the live winner's claim must be intact, got {held:?}"
            );
            winner.release().await;
            let _ = std::fs::remove_file(&lock);
        }
        assert_eq!(doubles, 0, "two stale reclaimers must never both acquire");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_lock_acquisition_leaves_no_lock() {
        // Round-3 review probe: aborting a task mid-acquisition must never
        // leave the lock file behind. The lock path only materializes
        // complete via the atomic link, and the guard exists the instant the
        // link returns — before the next await — so every abort point
        // (staging write, the link itself, post-acquire parking) either
        // orphans nothing but the staging name or drops a guard that removes
        // both names. The naive create-then-write-then-guard order leaked
        // 60/64.
        let dir = tempfile::tempdir().unwrap().keep();
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        for i in 0..64 {
            let final_path = dir.join(format!("acquire-{i}.bin"));
            let lock = dir.join(format!("acquire-{i}.bin.lock"));
            let sink = Sink::Local {
                final_path: final_path.clone(),
                staging_path: dir.join(format!("acquire-{i}.bin.part")),
            };
            let round_owner = owner.clone();
            let task = tokio::spawn(async move {
                let _guard = sink.acquire_lock(&round_owner).await.unwrap();
                std::future::pending::<()>().await;
            });
            tokio::time::timeout(Duration::from_secs(3), async {
                while !lock.exists() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            tokio::time::sleep(Duration::from_millis(2)).await;
            assert!(
                !lock.exists(),
                "aborting mid-acquisition must not leave a live-pid lock (round {i})"
            );
        }
    }

    #[tokio::test]
    async fn release_never_deletes_a_displaced_claim() {
        // If the claim at the lock path no longer names our nonce (a
        // successor acquired through the bounded displacement window),
        // releasing must leave the successor's lock strictly alone.
        let dir = tempfile::tempdir().unwrap().keep();
        let sink = Sink::Local {
            final_path: dir.join("d.bin"),
            staging_path: dir.join("d.bin.part"),
        };
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        let guard = sink.acquire_lock(&owner).await.unwrap();
        // Simulate the displacement: a successor's claim replaces ours.
        std::fs::write(
            dir.join("d.bin.lock"),
            serde_json::to_vec(&LockContent {
                hostname: hostname(),
                pid: std::process::id(),
                started_unix_s: 43,
                nonce: 987_654,
            })
            .unwrap(),
        )
        .unwrap();
        guard.release().await;
        let held = std::fs::read(dir.join("d.bin.lock")).unwrap();
        let content: LockContent = serde_json::from_slice(&held).unwrap();
        assert_eq!(content.nonce, 987_654, "the successor's claim survives");
    }

    #[tokio::test]
    async fn own_cancelled_host_lock_is_reclaimed_by_the_next_run() {
        // A host lock naming OUR pid with a nonce this process is not
        // executing is the remnant of a cancelled acquisition of ours (the
        // empty-guard window before the round-3 fix, or a torn write): no
        // other process can hold our pid, so the next run reclaims it.
        let (port, _server) = scripted_server(vec![
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap().keep();
        let out = dir.join("own.bin");
        std::fs::write(
            dir.join("own.bin.lock"),
            serde_json::to_vec(&LockContent {
                hostname: hostname(),
                pid: std::process::id(),
                started_unix_s: 0,
                nonce: 555_001,
            })
            .unwrap(),
        )
        .unwrap();
        let mut node = FileDownloadNode::new(spec(
            &format!("http://127.0.0.1:{port}/o"),
            &out.to_string_lossy(),
        ));
        node.execute_with_allowlist(&ctx(), Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
        assert!(!dir.join("own.bin.lock").exists(), "reclaimed and released");
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
    async fn object_store_conditional_create_is_exclusive() {
        // Backends without host paths lock through the backend's atomic
        // conditional create of a single `{key}.lock` object: two concurrent
        // acquires yield exactly one holder, release frees the target again,
        // and stale claims — dead owners and our own cancelled acquisitions
        // alike — are reported for manual removal when the backend has no
        // compare-and-swap to reclaim them atomically (the memory backend
        // is exactly such a backend).
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
            nonce: 0,
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
            (Ok(_), Ok(_)) => panic!("conditional create let two callers hold the lock"),
            (Err(a), Err(b)) => panic!("lock acquisition failed both callers: {a}; {b}"),
        };
        winner.release().await;
        // Free after release: a plain re-acquire succeeds.
        sink.acquire_lock(&owner).await.unwrap().release().await;

        // A claim from a dead same-host pid: the memory backend advertises no
        // compare-and-swap, so the acquisition must refuse explicitly and
        // name the object for manual removal instead of racing.
        op.write(
            "/x.bin.lock",
            serde_json::to_vec(&LockContent {
                hostname: hostname(),
                pid: 999_999,
                started_unix_s: 1,
                nonce: 0,
            })
            .unwrap(),
        )
        .await
        .unwrap();
        let error = match sink.acquire_lock(&owner).await {
            Err(error) => error,
            Ok(guard) => {
                guard.release().await;
                panic!("a dead-owner claim must not be reclaimed unsafely");
            }
        };
        assert!(
            matches!(error, FileDownloadError::TargetLocked { .. }),
            "{error}"
        );
        assert!(
            error.to_string().contains("remove `/x.bin.lock` manually"),
            "the refusal must name the object for manual removal: {error}"
        );
        let _ = op.delete("/x.bin.lock").await;

        // A claim of our own pid whose nonce this process is NOT executing
        // is a cancelled acquisition of ours — but it is reclaimed through
        // the same atomic protocol as a dead-owner claim now, and the memory
        // backend offers no compare-and-swap: the acquisition must refuse
        // and name the object for manual removal instead of racing (the old
        // bare delete could remove a live successor's claim mid-read).
        op.write(
            "/x.bin.lock",
            serde_json::to_vec(&LockContent {
                hostname: hostname(),
                pid: std::process::id(),
                started_unix_s: 2,
                nonce: 777_777,
            })
            .unwrap(),
        )
        .await
        .unwrap();
        let error = match sink.acquire_lock(&owner).await {
            Err(error) => error,
            Ok(guard) => {
                guard.release().await;
                panic!("an own-cancelled claim must not be reclaimed unsafely either");
            }
        };
        assert!(
            matches!(error, FileDownloadError::TargetLocked { .. }),
            "{error}"
        );
        assert!(
            error.to_string().contains("remove `/x.bin.lock` manually"),
            "the refusal must name the object for manual removal: {error}"
        );
    }

    #[tokio::test]
    async fn late_earlier_claim_cannot_displace_a_live_holder() {
        // Round-3 review probe: an earlier-started contender reaching the
        // object store AFTER a newer holder must not displace it. The lock is
        // a single conditional-create object — arrival order cannot matter.
        let op = opendal::Operator::new(opendal::services::Memory::default())
            .unwrap()
            .finish();
        let sink = Sink::Vfs {
            op: op.clone(),
            final_key: "/review.bin".into(),
            staging_key: "/review.bin.part".into(),
        };
        let newer = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        let delayed_earlier = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 41,
            nonce: 0,
        };
        let first_guard = sink.acquire_lock(&newer).await.unwrap();
        let second = sink.acquire_lock(&delayed_earlier).await;
        let displaced = matches!(second, Err(_));
        if let Ok(guard) = second {
            guard.release().await;
        }
        first_guard.release().await;
        assert!(
            displaced,
            "an earlier claim arriving late must not displace an active holder"
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

    #[test]
    fn claim_identity_requires_hostname_pid_and_nonce() {
        // Round-4 finding 1 essence: ownership is the FULL identity, so a
        // same-nonce claim from another process is not ours, and neither is
        // a same-pid claim with another nonce, nor another host, nor bytes
        // that cannot be parsed at all.
        let claim = LockContent {
            hostname: "h".into(),
            pid: 10,
            started_unix_s: 1,
            nonce: 77,
        };
        let bytes = serde_json::to_vec(&claim).unwrap();
        assert!(content_is_ours(&bytes, &claim));
        // The cross-process per-process-counter collision: same nonce,
        // different pid — the case that deleted a live foreign lock.
        let mut foreign_pid = claim.clone();
        foreign_pid.pid = 11;
        assert!(!content_is_ours(&bytes, &foreign_pid));
        // A successor's claim after displacement: same pid, fresh nonce.
        let mut successor = claim.clone();
        successor.nonce = 78;
        assert!(!content_is_ours(&bytes, &successor));
        let mut foreign_host = claim.clone();
        foreign_host.hostname = "elsewhere".into();
        assert!(!content_is_ours(&bytes, &foreign_host));
        // Round-4 finding 2: unparseable bytes are never ours.
        assert!(!content_is_ours(b"FOREIGN-LOCK-FORMAT", &claim));
        assert!(!content_is_ours(b"", &claim));
        // `started_unix_s` is descriptive, not identity: claims agreeing on
        // hostname+pid+nonce are the same acquisition.
        let mut reserialized = claim.clone();
        reserialized.started_unix_s = 999;
        assert!(content_is_ours(&bytes, &reserialized));
    }

    #[test]
    fn guard_never_deletes_a_foreign_process_lock_even_on_a_full_nonce_collision() {
        // Deterministic core of round-4 finding 1: a guard whose claim
        // shares the incumbent's nonce but names a different pid (the
        // cross-process collision shape) must not delete the incumbent on
        // drop. The old nonce-only `content_is_ours` removed it.
        let dir = tempfile::tempdir().unwrap().keep();
        let lock = dir.join("held.bin.lock");
        let incumbent = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: 424_242,
            started_unix_s: 1,
            nonce: 777,
        })
        .unwrap();
        std::fs::write(&lock, &incumbent).unwrap();
        let guard = LockGuard(Some(GuardInner::HostFile {
            path: lock.clone(),
            temp: None,
            claim: LockContent {
                hostname: hostname(),
                pid: std::process::id(),
                started_unix_s: 2,
                nonce: 777,
            },
            _flock: None,
            _active: ActiveDownload::register(777),
        }));
        drop(guard);
        assert_eq!(
            std::fs::read(&lock).unwrap(),
            incumbent,
            "a nonce collision must not let a guard delete a foreign process's live lock"
        );
    }

    #[tokio::test]
    async fn unparseable_foreign_lock_survives_a_failed_acquire() {
        // Round-4 finding 2 (reviewer probe): bytes that are not an
        // interpretable claim used to be treated as ours, so the failed
        // contender's guard Drop deleted them. Now the failed acquire
        // reports the holder as-is and the bytes survive untouched.
        let dir = tempfile::tempdir().unwrap().keep();
        let lock = dir.join("unknown.bin.lock");
        std::fs::write(&lock, b"FOREIGN-LOCK-FORMAT").unwrap();
        let sink = Sink::Local {
            final_path: dir.join("unknown.bin"),
            staging_path: dir.join("unknown.bin.part"),
        };
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        let acquired = sink.acquire_lock(&owner).await;
        assert!(
            matches!(acquired, Err(FileDownloadError::TargetLocked { .. })),
            "an unrecognized lock must block, not be adopted"
        );
        assert_eq!(
            std::fs::read(&lock).unwrap(),
            b"FOREIGN-LOCK-FORMAT",
            "a failed contender must never delete bytes it cannot recognize"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn own_cancelled_reclaim_is_exclusive() {
        // Round-4 finding 3 stress (reviewer probe, 1024 rounds): two tasks
        // racing to reclaim the same own-cancelled claim must never both
        // hold the lock. The fence-verified reclaim replaced the old bare
        // read-then-unlink, whose interleaving admitted a second holder
        // while the first was still live.
        let dir = tempfile::tempdir().unwrap().keep();
        let lock = dir.join("cancelled.bin.lock");
        let sink = Sink::Local {
            final_path: dir.join("cancelled.bin"),
            staging_path: dir.join("cancelled.bin.part"),
        };
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        let orphan = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 1,
            nonce: u64::MAX,
        })
        .unwrap();
        let mut doubles = 0;
        for _ in 0..1024 {
            std::fs::write(&lock, &orphan).unwrap();
            let (a, b) = tokio::join!(sink.acquire_lock(&owner), sink.acquire_lock(&owner));
            if a.is_ok() && b.is_ok() {
                doubles += 1;
            }
            if let Ok(guard) = a {
                guard.release().await;
            }
            if let Ok(guard) = b {
                guard.release().await;
            }
            let _ = std::fs::remove_file(&lock);
        }
        assert_eq!(
            doubles, 0,
            "own-cancelled reclaimers must never both acquire"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn three_dead_owner_reclaimers_never_double_hold() {
        // Reviewer probe (2048 rounds, three reclaimers): the reclaim
        // protocol must keep at most one holder even with a third contender
        // constantly racing the other two. The earlier move-aside variant
        // FAILED this probe intermittently (~1 double-holder per few runs):
        // its rename→restore window admitted a fresh link while a delayed
        // reclaimer still held the moved claim aside, and the restore's
        // already-exists branch then destroyed the displaced claim's bytes.
        // The flock fence closes the window by construction.
        let dir = tempfile::tempdir().unwrap().keep();
        let lock = dir.join("dead.bin.lock");
        let sink = Sink::Local {
            final_path: dir.join("dead.bin"),
            staging_path: dir.join("dead.bin.part"),
        };
        assert!(!pid_alive(999_999));
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        let orphan = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: 999_999,
            started_unix_s: 1,
            nonce: 0,
        })
        .unwrap();
        let mut doubles = 0;
        for _ in 0..2048 {
            std::fs::write(&lock, &orphan).unwrap();
            let (a, b, c) = tokio::join!(
                sink.acquire_lock(&owner),
                sink.acquire_lock(&owner),
                sink.acquire_lock(&owner)
            );
            let holders = a.is_ok() as usize + b.is_ok() as usize + c.is_ok() as usize;
            if holders > 1 {
                doubles += 1;
            }
            if let Ok(guard) = a {
                guard.release().await;
            }
            if let Ok(guard) = b {
                guard.release().await;
            }
            if let Ok(guard) = c {
                guard.release().await;
            }
            let _ = std::fs::remove_file(&lock);
        }
        assert_eq!(
            doubles, 0,
            "reclaiming an incumbent lock must not admit a third holder"
        );
    }

    #[tokio::test]
    async fn delayed_stale_reclaimer_cannot_touch_a_new_live_holder() {
        // Round-4 finding 3, deterministic form of the reviewer's
        // controlled-scheduling probe: a reclaimer that classified the
        // incumbent as stale BEFORE a successor acquired must fail against
        // the successor when it finally acts — the claim's flock is held by
        // the live successor, so the stale judgment can never rename or
        // delete it. The old bare unlink deleted it
        // (`first_owned_after=false, incumbent_preserved=false`); the
        // move-aside variant needed a byte-for-byte restore dance instead.
        let dir = tempfile::tempdir().unwrap().keep();
        let lock = dir.join("delayed.bin.lock");
        let sink = Sink::Local {
            final_path: dir.join("delayed.bin"),
            staging_path: dir.join("delayed.bin.part"),
        };
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        let guard = sink.acquire_lock(&owner).await.unwrap();
        let held_now = std::fs::read(&lock).unwrap();
        // The delayed reclaimer had read the ORPHAN bytes before the
        // successor's link landed; its reclaim now hits the successor's
        // live claim instead — and must be refused by the fence.
        let orphan = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 1,
            nonce: u64::MAX,
        })
        .unwrap();
        let result = reclaim_stale_host_lock(&lock, &orphan, 123_456).await;
        assert!(
            matches!(result, Err(FileDownloadError::TargetLocked { .. })),
            "a delayed stale reclaimer must not touch the new live holder's claim, got {result:?}"
        );
        assert_eq!(
            std::fs::read(&lock).unwrap(),
            held_now,
            "the successor's live claim must be untouched"
        );
        assert!(guard.still_holds().await, "the successor still fences");
        guard.release().await;
        assert!(!lock.exists(), "released cleanly after the refusal");
    }

    // Child half of the cross-process lock test: holds the target lock
    // until the parent writes `stop`, then releases cleanly.
    #[tokio::test]
    async fn child_process_lock_holder() {
        let Some(dir) = std::env::var_os("FILE_DOWNLOAD_CHILD_LOCK_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let sink = Sink::Local {
            final_path: dir.join("foreign.bin"),
            staging_path: dir.join("foreign.bin.part"),
        };
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 1,
            nonce: 0,
        };
        let guard = sink.acquire_lock(&owner).await.unwrap();
        std::fs::write(dir.join("ready"), b"ready").unwrap();
        while !dir.join("stop").exists() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        guard.release().await;
    }

    #[tokio::test]
    async fn failed_acquire_preserves_a_foreign_live_lock_even_when_nonces_collide() {
        // Round-4 finding 1 (reviewer probe, real child process): a live
        // foreign process holds the lock; the contender fails with
        // TargetLocked and the incumbent's bytes must survive. Historically
        // both processes minted nonce 0 and the contender's guard deleted
        // the holder's lock; identity now covers hostname+pid+nonce. With
        // the randomly seeded nonce base the literal collision is no longer
        // constructible across processes — the full-collision guard
        // behavior is covered deterministically in
        // `guard_never_deletes_a_foreign_process_lock_even_on_a_full_nonce_collision`.
        let dir = tempfile::tempdir().unwrap().keep();
        let lock = dir.join("foreign.bin.lock");
        let sink = Sink::Local {
            final_path: dir.join("foreign.bin"),
            staging_path: dir.join("foreign.bin.part"),
        };
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "child_process_lock_holder",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("FILE_DOWNLOAD_CHILD_LOCK_DIR", &dir)
            .spawn()
            .unwrap();
        let ready = tokio::time::timeout(Duration::from_secs(30), async {
            while !dir.join("ready").exists() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        if ready.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child process did not acquire the lock");
        }
        let original = std::fs::read(&lock).unwrap();
        let incumbent: LockContent = serde_json::from_slice(&original).unwrap();
        assert_eq!(incumbent.pid, child.id(), "the live holder is the child");
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 2,
            nonce: 0,
        };
        let acquired = sink.acquire_lock(&owner).await;
        let blocked = matches!(acquired, Err(FileDownloadError::TargetLocked { .. }));
        if let Ok(guard) = acquired {
            guard.release().await;
        }
        let preserved = std::fs::read(&lock).ok().as_deref() == Some(original.as_slice());
        std::fs::write(dir.join("stop"), b"stop").unwrap();
        let status = child.wait().unwrap();
        assert!(blocked, "a live foreign process must block acquisition");
        assert!(
            preserved,
            "a failed acquisition must preserve the incumbent's live lock"
        );
        assert!(
            status.success(),
            "the child holder must exit cleanly: {status:?}"
        );
    }

    /// A versioned in-memory object backend for tests: every write bumps a
    /// per-key version and publishes it as the etag (`"v{n}"`), and writer
    /// `close` evaluates `if_not_exists` / `if_match` / `if_none_match`
    /// against the version live at close — the atomic fence a real object
    /// store provides. `interfere_after_read`, once armed for a key,
    /// replaces the content and bumps the version right AFTER serving the
    /// next read of that key — simulating a successor's takeover landing
    /// between the caller's read and its subsequent conditional write.
    #[derive(Debug, Clone)]
    struct VersionedMemory {
        state: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, (Vec<u8>, u64)>>>,
        interfere_after_read: std::sync::Arc<std::sync::Mutex<Option<(String, Vec<u8>)>>>,
        info: std::sync::Arc<opendal::raw::AccessorInfo>,
    }

    impl VersionedMemory {
        fn new() -> Self {
            let info = opendal::raw::AccessorInfo::default();
            info.set_scheme("versioned-memory");
            info.set_root("/");
            info.set_native_capability(opendal::Capability {
                stat: true,
                read: true,
                write: true,
                write_can_empty: true,
                write_with_if_not_exists: true,
                write_with_if_match: true,
                ..Default::default()
            });
            Self {
                state: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
                interfere_after_read: std::sync::Arc::new(std::sync::Mutex::new(None)),
                info: std::sync::Arc::new(info),
            }
        }

        fn arm_interference(&self, key: &str, replacement: Vec<u8>) {
            *self.interfere_after_read.lock().unwrap() =
                Some((normalize_key(key).to_string(), replacement));
        }
    }

    /// Keys are stored compared-with and looked-up-by this canonical form:
    /// the operator layer and the caller may spell the same key with or
    /// without a leading `/`, and the backend must not care.
    fn normalize_key(key: &str) -> &str {
        key.strip_prefix('/').unwrap_or(key)
    }

    impl opendal::raw::Access for VersionedMemory {
        type Reader = opendal::Buffer;
        type Writer = VersionedWriter;
        type Lister = ();
        type Deleter = ();
        type Copier = ();

        fn info(&self) -> std::sync::Arc<opendal::raw::AccessorInfo> {
            self.info.clone()
        }

        async fn stat(
            &self,
            path: &str,
            _args: opendal::raw::OpStat,
        ) -> opendal::Result<opendal::raw::RpStat> {
            let state = self.state.lock().unwrap();
            match state.get(normalize_key(path)) {
                Some((bytes, version)) => Ok(opendal::raw::RpStat::new(
                    opendal::Metadata::new(opendal::EntryMode::FILE)
                        .with_content_length(bytes.len() as u64)
                        .with_etag(format!("v{version}")),
                )),
                None => Err(opendal::Error::new(
                    opendal::ErrorKind::NotFound,
                    "no such key",
                )),
            }
        }

        async fn read(
            &self,
            path: &str,
            _args: opendal::raw::OpRead,
        ) -> opendal::Result<(opendal::raw::RpRead, Self::Reader)> {
            let mut interference = None;
            {
                let mut armed = self.interfere_after_read.lock().unwrap();
                if let Some((key, _)) = armed.as_ref()
                    && key == normalize_key(path)
                {
                    interference = armed.take();
                }
            }
            let bytes = {
                let state = self.state.lock().unwrap();
                match state.get(normalize_key(path)) {
                    Some((bytes, _)) => bytes.clone(),
                    None => {
                        return Err(opendal::Error::new(
                            opendal::ErrorKind::NotFound,
                            "no such key",
                        ));
                    }
                }
            };
            if let Some((key, replacement)) = interference {
                let mut state = self.state.lock().unwrap();
                let next = state.get(&key).map(|(_, v)| v + 1).unwrap_or(1);
                state.insert(key, (replacement, next));
            }
            let len = bytes.len() as u64;
            Ok((
                opendal::raw::RpRead::new(
                    opendal::Metadata::new(opendal::EntryMode::FILE).with_content_length(len),
                ),
                opendal::Buffer::from(bytes),
            ))
        }

        async fn write(
            &self,
            path: &str,
            args: opendal::raw::OpWrite,
        ) -> opendal::Result<(opendal::raw::RpWrite, Self::Writer)> {
            Ok((
                opendal::raw::RpWrite::new(),
                VersionedWriter {
                    state: self.state.clone(),
                    key: normalize_key(path).to_string(),
                    buffer: Vec::new(),
                    if_match: args.if_match().map(str::to_string),
                    if_none_match: args.if_none_match().map(str::to_string),
                    if_not_exists: args.if_not_exists(),
                },
            ))
        }
    }

    struct VersionedWriter {
        state: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, (Vec<u8>, u64)>>>,
        key: String,
        buffer: Vec<u8>,
        if_match: Option<String>,
        if_none_match: Option<String>,
        if_not_exists: bool,
    }

    impl opendal::raw::oio::Write for VersionedWriter {
        async fn write(&mut self, bs: opendal::Buffer) -> opendal::Result<()> {
            self.buffer.extend(bs.to_vec());
            Ok(())
        }

        async fn close(&mut self) -> opendal::Result<opendal::Metadata> {
            let mut state = self.state.lock().unwrap();
            let current = state.get(&self.key);
            let mismatch =
                || opendal::Error::new(opendal::ErrorKind::ConditionNotMatch, "version moved on");
            if self.if_not_exists && current.is_some() {
                return Err(mismatch());
            }
            if let Some(expected) = &self.if_match {
                match current {
                    Some((_, version)) if expected == &format!("v{version}") => {}
                    _ => return Err(mismatch()),
                }
            }
            if let Some(expected) = &self.if_none_match {
                if let Some((_, version)) = current {
                    if expected == "*" || expected == &format!("v{version}") {
                        return Err(mismatch());
                    }
                }
            }
            let next = current.map(|(_, v)| v + 1).unwrap_or(1);
            let content = std::mem::take(&mut self.buffer);
            let len = content.len() as u64;
            state.insert(self.key.clone(), (content, next));
            Ok(opendal::Metadata::new(opendal::EntryMode::FILE)
                .with_content_length(len)
                .with_etag(format!("v{next}")))
        }

        async fn abort(&mut self) -> opendal::Result<()> {
            self.buffer = Vec::new();
            Ok(())
        }
    }

    #[tokio::test]
    async fn object_store_takeover_cas_is_bound_to_the_judged_version() {
        // Round-4 finding 4 (static finding, now runtime): the object-store
        // reclaim must observe the etag BEFORE reading the claim bytes, so
        // the conditional write can only replace the very version that was
        // judged stale. The interference hook replaces the claim right
        // after serving our read — a successor's takeover in that window.
        // A stat-AFTER-read ordering picks up the successor's NEW etag and
        // the conditional write overwrites the live successor; with
        // etag-first the write fails the match and the successor survives.
        let backend = VersionedMemory::new();
        let op = opendal::OperatorBuilder::new(backend.clone()).finish();
        let sink = Sink::Vfs {
            op: op.clone(),
            final_key: "/t.bin".into(),
            staging_key: "/t.bin.part".into(),
        };
        let owner = LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 42,
            nonce: 0,
        };
        // The stale claim names a dead process on this host.
        let dead = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: 999_999,
            started_unix_s: 1,
            nonce: 7,
        })
        .unwrap();
        op.write("/t.bin.lock", dead).await.unwrap();
        // The successor that lands right after our read: our own pid with a
        // nonce this process IS executing — a live claim by construction.
        let executing = ActiveDownload::register(424_242);
        let successor = serde_json::to_vec(&LockContent {
            hostname: hostname(),
            pid: std::process::id(),
            started_unix_s: 2,
            nonce: 424_242,
        })
        .unwrap();
        backend.arm_interference("/t.bin.lock", successor.clone());
        match sink.acquire_lock(&owner).await {
            Ok(guard) => {
                guard.release().await;
                panic!("a takeover racing a stale reclaim must not hand out the lock");
            }
            Err(error) => {
                assert!(
                    matches!(error, FileDownloadError::TargetLocked { .. }),
                    "{error}"
                );
            }
        }
        assert_eq!(
            op.read("/t.bin.lock").await.unwrap().to_vec(),
            successor,
            "the successor's live claim must survive the failed reclaim byte-for-byte"
        );
        drop(executing);
    }
}
