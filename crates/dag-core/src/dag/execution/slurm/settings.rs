//! Slurm executor settings: the coordinator-side view of a target.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::contract::{TaskAttemptReceipt, TaskDispatch, TaskExecution};
use super::super::process::{
    ProcessTaskSpec, parse_process_task, process_task_environment, render_process_script,
    sanitize_path_component,
};
use super::super::traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
use super::ssh::SshTarget;
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::{DomainId, FileFingerprint, FileRef};

// ── settings ─────────────────────────────────────────────────────────────
/// Operator-owned settings for the Slurm backend. Built from
/// `RuntimeConfig::slurm` at daemon start; never agent-settable.
#[derive(Debug, Clone)]
pub struct SlurmSettings {
    /// Operator-owned digest-to-SIF map; absent means container tasks fail closed.
    pub apptainer: Option<super::super::apptainer::ApptainerSettings>,
    /// ssh destination for the cluster login node.
    pub ssh: SshTarget,
    /// Remote scratch root for task workspaces; a leading `~/` expands to
    /// the remote home directory. Must not contain whitespace once expanded
    /// (sbatch directive values cannot be quoted).
    pub scratch_dir: String,
    /// Local directory downloaded artifacts land in.
    pub local_artifact_root: PathBuf,
    /// Default sbatch partition (e.g. `64c512g`).
    pub partition: String,
    /// Explicit allocation account; no implicit scheduler default when configured.
    pub account: Option<String>,
    /// Scheduler-owned GPU allocation. None preserves CPU-only submission.
    pub gpus: Option<u32>,
    /// Optional operator QOS (e.g. debug for a debug GPU partition).
    pub qos: Option<String>,
    /// Memory per allocated CPU when a task requests no explicit total.
    pub mem_per_cpu: String,
    /// Wall-clock limit when a task requests no explicit duration.
    pub default_time: String,
    /// Terminal-state polling cadence.
    pub poll_interval: Duration,
}
