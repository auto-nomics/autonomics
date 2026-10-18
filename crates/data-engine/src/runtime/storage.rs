//! Central lifecycle services for the per-session DataEngine actors.
//!
//! [`DataEngineManager`](super::DataEngineManager) owns one instance of each
//! service. They provide the process-wide policies that individual session
//! engines cannot safely decide alone: idle eviction, output-cache expiry,
//! cross-session file-result reuse, active references, run-aware local
//! cleanup, and a managed-root byte budget.
//!
//! # Storage governance semantics (conservative)
//!
//! The [`StorageCoordinator`] keeps two reference indexes:
//!
//! - **Local managed roots** (`AUTONOMICS_DAG_WORK_ROOT`,
//!   `AUTONOMICS_DAG_RECEIPTS_ROOT`): workdirs and receipt roots, counted per
//!   run / live session / result-cache owner. These are the only things the
//!   run-aware budget cleaner may delete.
//! - **VFS artifacts** (`vfs://` URIs): counted the same way and reported
//!   through [`StorageCoordinator::artifact_snapshot`] and
//!   [`StorageStatus`], but **never deleted** by local budget cleanup —
//!   immutable object-store artifacts are treated as durable provenance.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use crate::dag::RunReport;
use crate::dag::history::DagHistory;
use crate::value::FileRef;

/// A single reusable file output.
#[derive(Debug, Clone)]
pub struct CachedFileOutput {
    pub node_id: String,
    pub fingerprint: String,
    pub port: u8,
    pub file: FileRef,
}

#[derive(Debug, Clone)]
struct CachedResult {
    fingerprint: String,
    outputs: Vec<CachedFileOutput>,
    last_access: Instant,
}

/// Fingerprint-addressed file-result cache shared by all sessions.
///
/// Only file outputs participate. DataFrame and Channel values remain
/// session-local because they are not durable values and cannot be restored
/// safely from a report.
#[derive(Debug, Default)]
pub struct ResultCache {
    max_entries: usize,
    ttl: Option<Duration>,
    entries: RwLock<HashMap<String, CachedResult>>,
}

impl ResultCache {
    pub fn new(max_entries: usize, ttl: Option<Duration>) -> Self {
        Self {
            max_entries,
            ttl,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Record successful file outputs from a completed run.
    ///
    /// A fingerprint includes node kind/spec, input identities, engine version
    /// and plugin identity, so an equal fingerprint is the cross-session reuse
    /// key. Missing local files are rejected; immutable VFS files are accepted.
    pub fn record_report(&self, session_id: &str, report: &RunReport) -> usize {
        self.record_report_collected(session_id, report).0.len()
    }

    /// [`Self::record_report`] returning the outputs it cached and the
    /// outputs it displaced, so the caller can maintain cache-owner storage
    /// references in lockstep.
    pub fn record_report_collected(
        &self,
        _session_id: &str,
        report: &RunReport,
    ) -> (Vec<CachedFileOutput>, Vec<CachedFileOutput>) {
        let mut cached = Vec::new();
        let mut replaced = Vec::new();
        for node in &report.nodes {
            let Some(fingerprint) = &node.fingerprint else {
                continue;
            };
            let outputs = node
                .port_assignments
                .iter()
                .map(|(port, file)| CachedFileOutput {
                    node_id: node.id.clone(),
                    fingerprint: fingerprint.clone(),
                    port: *port,
                    file: file.clone(),
                })
                .collect::<Vec<_>>();
            if outputs.is_empty() || !outputs.iter().all(|entry| output_available(&entry.file)) {
                continue;
            }
            let mut entries = self.entries.write();
            if let Some(previous) = entries.insert(
                fingerprint.clone(),
                CachedResult {
                    fingerprint: fingerprint.clone(),
                    outputs: outputs.clone(),
                    last_access: Instant::now(),
                },
            ) {
                replaced.extend(previous.outputs);
            }
            cached.extend(outputs);
        }
        // Enforce TTL/LRU immediately; retired entries lose their cache-owner
        // reference together with the ones displaced above.
        let retired = self.prune_locked();
        replaced.extend(retired);
        (cached, replaced)
    }

    /// Take a cache snapshot before a run.
    ///
    /// Entries whose local files have disappeared since recording are dropped
    /// here as well: there is no point seeding a run with outputs that cannot
    /// be read. Full fingerprint freshness is verified by
    /// `DataEngine::restore_cached_file_outputs` at restore time.
    pub fn snapshot(&self) -> Vec<CachedFileOutput> {
        let now = Instant::now();
        let selected: Vec<_> = self
            .entries
            .read()
            .values()
            .filter(|entry| match self.ttl {
                Some(ttl) => now.duration_since(entry.last_access) < ttl,
                None => true,
            })
            .filter(|entry| entry.outputs.iter().all(|out| output_available(&out.file)))
            .cloned()
            .collect();
        let keys = selected
            .iter()
            .map(|entry| entry.fingerprint.clone())
            .collect::<Vec<_>>();
        let mut write = self.entries.write();
        for key in keys {
            if let Some(entry) = write.get_mut(&key) {
                entry.last_access = now;
            }
        }
        drop(write);
        selected
            .into_iter()
            .flat_map(|entry| entry.outputs)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    pub fn prune(&self) -> usize {
        self.prune_locked().len()
    }

    /// Expire TTL-stale entries and enforce the LRU bound, returning every
    /// evicted output so the caller can drop the corresponding cache-owner
    /// storage references.
    pub fn prune_retired(&self) -> Vec<CachedFileOutput> {
        self.prune_locked()
    }

    fn prune_locked(&self) -> Vec<CachedFileOutput> {
        let mut entries = self.entries.write();
        let mut retired = Vec::new();
        if let Some(ttl) = self.ttl {
            let now = Instant::now();
            let expired: Vec<String> = entries
                .iter()
                .filter(|(_, entry)| now.duration_since(entry.last_access) >= ttl)
                .map(|(key, _)| key.clone())
                .collect();
            for key in expired {
                if let Some(entry) = entries.remove(&key) {
                    retired.extend(entry.outputs);
                }
            }
        }
        // Recency eviction. `record_report` moved each selected key, so sorting
        // by last access gives deterministic LRU semantics.
        if self.max_entries > 0 && entries.len() > self.max_entries {
            let mut keys: Vec<_> = entries
                .iter()
                .map(|(key, value)| (key.clone(), value.last_access))
                .collect();
            keys.sort_by_key(|(_, access)| *access);
            let excess = entries.len().saturating_sub(self.max_entries);
            for (key, _) in keys.into_iter().take(excess) {
                if let Some(entry) = entries.remove(&key) {
                    retired.extend(entry.outputs);
                }
            }
        }
        retired
    }
}

fn output_available(file: &FileRef) -> bool {
    if file.path.starts_with("vfs://") {
        return true;
    }
    Path::new(&file.path).exists()
}

/// One active reference to a managed local directory.
///
/// `sessions` counts live session owners, `cache` counts result-cache owners;
/// either protects the path from run-aware cleanup. `runs` records which
/// historical runs mention the path (informational, for status reporting).
#[derive(Debug, Clone, Default)]
struct StorageReferences {
    runs: HashSet<String>,
    sessions: HashSet<String>,
    cache: usize,
}

impl StorageReferences {
    fn protected(&self) -> bool {
        !self.sessions.is_empty() || self.cache > 0
    }

    fn empty(&self) -> bool {
        self.runs.is_empty() && self.sessions.is_empty() && self.cache == 0
    }
}

/// Reference counts for one tracked VFS artifact.
#[derive(Debug, Clone, Default)]
struct ArtifactUsage {
    runs: HashSet<String>,
    sessions: HashSet<String>,
    bytes: u64,
}

/// One row of [`StorageCoordinator::artifact_snapshot`].
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactEntry {
    pub uri: String,
    pub runs: usize,
    pub sessions: usize,
    pub logical_bytes: u64,
}

/// One row of [`StorageStatus::references`].
#[derive(Debug, Clone, Serialize)]
pub struct ReferenceEntry {
    pub path: PathBuf,
    pub runs: usize,
    pub sessions: usize,
    pub cache_owners: usize,
}

/// Point-in-time storage view served by
/// [`DataEngineManager::storage_status`](super::DataEngineManager::storage_status).
#[derive(Debug, Default, Clone, Serialize)]
pub struct StorageStatus {
    pub session_count: usize,
    pub result_cache_entries: usize,
    /// Bytes under the local managed roots (workdirs + receipts).
    pub local_bytes: u64,
    /// Tracked `vfs://` artifacts. Indexed and reported only — the local
    /// budget cleaner never deletes them.
    pub artifact_count: usize,
    pub artifact_bytes: u64,
    pub references: Vec<ReferenceEntry>,
    pub artifacts: Vec<ArtifactEntry>,
}

/// Tracks local workdirs/receipts and VFS artifacts currently retained by
/// live sessions, the result cache, and recorded history.
#[derive(Debug, Default)]
pub struct StorageCoordinator {
    work_root: PathBuf,
    receipt_root: PathBuf,
    references: Mutex<HashMap<PathBuf, StorageReferences>>,
    artifacts: Mutex<HashMap<String, ArtifactUsage>>,
}

impl StorageCoordinator {
    pub fn new(work_root: impl Into<PathBuf>, receipt_root: impl Into<PathBuf>) -> Self {
        Self {
            work_root: canonical_or_raw(work_root.into()),
            receipt_root: canonical_or_raw(receipt_root.into()),
            references: Mutex::new(HashMap::new()),
            artifacts: Mutex::new(HashMap::new()),
        }
    }

    pub fn work_root(&self) -> &Path {
        &self.work_root
    }

    pub fn receipt_root(&self) -> &Path {
        &self.receipt_root
    }

    /// Retain every managed local path and VFS artifact mentioned by a run
    /// report for its producing session. This is the live-session half of
    /// reference counting; history-based retention is recomputed by run-aware
    /// cleanup.
    pub fn retain_report(&self, session_id: &str, report: &RunReport) -> usize {
        let run_id = report
            .run_id
            .clone()
            .unwrap_or_else(|| format!("session:{session_id}"));
        let mut retained = 0;
        let value = match serde_json::to_value(report) {
            Ok(value) => value,
            Err(_) => return 0,
        };
        {
            let mut refs = self.references.lock();
            for path in report_local_roots_from_value(&value, &self.work_root, &self.receipt_root) {
                let entry = refs.entry(path).or_default();
                entry.runs.insert(run_id.clone());
                entry.sessions.insert(session_id.to_string());
                retained += 1;
            }
        }
        {
            let mut artifacts = self.artifacts.lock();
            for (uri, bytes) in vfs_artifacts_from_value(&value) {
                let entry = artifacts.entry(uri).or_default();
                entry.runs.insert(run_id.clone());
                entry.sessions.insert(session_id.to_string());
                if entry.bytes == 0 {
                    entry.bytes = bytes;
                }
                retained += 1;
            }
        }
        retained
    }

    /// Add a result-cache owner reference for every managed local path behind
    /// `outputs`, so run-aware cleanup cannot delete a file that a future
    /// cross-session restore may still consume.
    pub fn retain_cache_owner(&self, outputs: &[CachedFileOutput]) -> usize {
        let mut refs = self.references.lock();
        let mut retained = 0;
        for output in outputs {
            let Some(path) =
                managed_root_from_path(&output.file.path, &self.work_root, &self.receipt_root)
            else {
                continue;
            };
            refs.entry(path).or_default().cache += 1;
            retained += 1;
        }
        retained
    }

    /// Drop result-cache owner references for evicted cache entries.
    pub fn release_cache_owner(&self, outputs: &[CachedFileOutput]) -> usize {
        let mut refs = self.references.lock();
        let mut released = 0;
        for output in outputs {
            let Some(path) =
                managed_root_from_path(&output.file.path, &self.work_root, &self.receipt_root)
            else {
                continue;
            };
            if let Some(entry) = refs.get_mut(&path)
                && entry.cache > 0
            {
                entry.cache -= 1;
                released += 1;
                if entry.empty() {
                    refs.remove(&path);
                }
            }
        }
        released
    }

    pub fn release_session(&self, session_id: &str) -> usize {
        let mut released = 0;
        {
            let mut refs = self.references.lock();
            for entry in refs.values_mut() {
                if entry.sessions.remove(session_id) {
                    released += 1;
                }
            }
            refs.retain(|_, entry| !entry.empty());
        }
        let mut artifacts = self.artifacts.lock();
        for entry in artifacts.values_mut() {
            if entry.sessions.remove(session_id) {
                released += 1;
            }
        }
        artifacts.retain(|_, entry| !entry.runs.is_empty() || !entry.sessions.is_empty());
        released
    }

    /// Count managed local paths protected by at least one live session.
    pub fn active_reference_count(&self) -> usize {
        self.references
            .lock()
            .iter()
            .filter(|(_, entry)| !entry.sessions.is_empty())
            .count()
    }

    /// Bytes currently stored under the local managed roots.
    pub fn managed_bytes(&self) -> u64 {
        managed_bytes(&self.work_root, &self.receipt_root)
    }

    /// Number of tracked VFS artifacts.
    pub fn artifact_count(&self) -> usize {
        self.artifacts.lock().len()
    }

    /// Summed logical byte size of tracked VFS artifacts.
    pub fn artifact_bytes(&self) -> u64 {
        self.artifacts.lock().values().map(|e| e.bytes).sum()
    }

    fn protected_paths(&self) -> HashSet<PathBuf> {
        self.references
            .lock()
            .iter()
            .filter(|(_, entry)| entry.protected())
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn reference_snapshot(&self) -> Vec<ReferenceEntry> {
        self.references
            .lock()
            .iter()
            .map(|(path, entry)| ReferenceEntry {
                path: path.clone(),
                runs: entry.runs.len(),
                sessions: entry.sessions.len(),
                cache_owners: entry.cache,
            })
            .collect()
    }

    pub fn artifact_snapshot(&self) -> Vec<ArtifactEntry> {
        self.artifacts
            .lock()
            .iter()
            .map(|(uri, entry)| ArtifactEntry {
                uri: uri.clone(),
                runs: entry.runs.len(),
                sessions: entry.sessions.len(),
                logical_bytes: entry.bytes,
            })
            .collect()
    }

    /// Rebuild historical run references (local roots + VFS artifacts) from
    /// `DagHistory` into the index, so [`StorageStatus`] reflects every
    /// recorded run rather than only live sessions. Run-aware cleanup
    /// performs the same sync as part of [`Self::classify_candidates`].
    pub async fn sync_history_references(&self, history: &DagHistory) -> Result<(), String> {
        let runs = history
            .list_runs(usize::MAX, None)
            .await
            .map_err(|error| error.to_string())?;
        for run in &runs {
            let Some(report_json) = &run.run_report_json else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(report_json) else {
                continue;
            };
            let mut refs = self.references.lock();
            for path in report_local_roots_from_value(&value, &self.work_root, &self.receipt_root) {
                refs.entry(path).or_default().runs.insert(run.id.clone());
            }
            drop(refs);
            let mut artifacts = self.artifacts.lock();
            for (uri, bytes) in vfs_artifacts_from_value(&value) {
                let entry = artifacts.entry(uri).or_default();
                entry.runs.insert(run.id.clone());
                if entry.bytes == 0 {
                    entry.bytes = bytes;
                }
            }
        }
        Ok(())
    }

    /// Classify managed local directories for run-aware cleanup: rebuild the
    /// historical reference index from `history`, then collect directories
    /// that belong only to runs outside the newest `keep_runs` and are not
    /// protected by a live session or the result cache.
    async fn classify_candidates(
        &self,
        history: &DagHistory,
        keep_runs: usize,
    ) -> Result<(HashSet<PathBuf>, usize, usize), String> {
        let keep_runs = keep_runs.max(1);
        let runs = history
            .list_runs(keep_runs, None)
            .await
            .map_err(|error| error.to_string())?;
        let protected_runs: HashSet<_> = runs.iter().map(|run| run.id.as_str()).collect();

        // list_runs is newest-first. Re-list all runs, rebuild the historical
        // reference index, then classify old paths.
        let all_runs = history
            .list_runs(usize::MAX, None)
            .await
            .map_err(|error| error.to_string())?;
        let mut candidate_runs = Vec::new();
        for run in &all_runs {
            let Some(report_json) = &run.run_report_json else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(report_json) else {
                continue;
            };
            {
                let mut refs = self.references.lock();
                for root in
                    report_local_roots_from_value(&value, &self.work_root, &self.receipt_root)
                {
                    refs.entry(root).or_default().runs.insert(run.id.clone());
                }
            }
            {
                let mut artifacts = self.artifacts.lock();
                for (uri, bytes) in vfs_artifacts_from_value(&value) {
                    let entry = artifacts.entry(uri).or_default();
                    entry.runs.insert(run.id.clone());
                    if entry.bytes == 0 {
                        entry.bytes = bytes;
                    }
                }
            }
            if !protected_runs.contains(run.id.as_str()) {
                candidate_runs.push(value);
            }
        }

        let active = self.protected_paths();
        let mut candidates = HashSet::<PathBuf>::new();
        let mut protected_directories = 0usize;
        for report_json in &candidate_runs {
            for root in
                report_local_roots_from_value(report_json, &self.work_root, &self.receipt_root)
            {
                if active.contains(&root) {
                    protected_directories += 1;
                } else {
                    candidates.insert(root);
                }
            }
        }
        Ok((candidates, protected_directories, all_runs.len()))
    }

    fn base_report(&self, keep_runs: usize, scanned_runs: usize) -> RunAwareCleanReport {
        let artifacts = self.artifacts.lock();
        RunAwareCleanReport {
            keep_runs: keep_runs.max(1),
            scanned_runs,
            retained_artifacts: artifacts.len(),
            retained_artifact_bytes: artifacts.values().map(|entry| entry.bytes).sum(),
            ..Default::default()
        }
    }

    /// Remove `candidates` (oldest first) until usage drops to `target`.
    fn remove_candidates(
        &self,
        candidates: HashSet<PathBuf>,
        target: u64,
        dry_run: bool,
        report: &mut RunAwareCleanReport,
    ) {
        let mut current = report.bytes_before;
        let mut ordered = candidates.into_iter().collect::<Vec<_>>();
        ordered.sort_by_key(|path| modified(path));
        for path in ordered {
            if current <= target {
                break;
            }
            let bytes = dir_size(&path);
            if dry_run {
                report.removed += 1;
                current = current.saturating_sub(bytes);
                continue;
            }
            match remove_dir(&path) {
                Ok(removed_bytes) => {
                    current = current.saturating_sub(removed_bytes);
                    report.removed += 1;
                    report.bytes_freed += removed_bytes;
                    self.references.lock().remove(&path);
                }
                Err(error) => report
                    .errors
                    .push(format!("cannot remove `{}`: {error}", path.display())),
            }
        }
        if dry_run {
            report.bytes_freed = report.bytes_before.saturating_sub(current);
        }
        report.bytes_after = if dry_run {
            current
        } else {
            managed_bytes(&self.work_root, &self.receipt_root)
        };
        report.dry_run = dry_run;
    }

    /// Enforce the local byte budget: delete managed local directories of old
    /// runs (oldest first) until usage drops to 75% of `max_bytes`.
    ///
    /// Live-session and result-cache references are never deleted. Immutable
    /// VFS artifacts are indexed and reported, but never touched.
    pub async fn run_aware_clean(
        &self,
        history: &DagHistory,
        keep_runs: usize,
        max_bytes: u64,
        dry_run: bool,
    ) -> Result<RunAwareCleanReport, String> {
        let (candidates, protected_directories, scanned_runs) =
            self.classify_candidates(history, keep_runs).await?;
        let mut report = self.base_report(keep_runs, scanned_runs);
        report.candidate_directories = candidates.len();
        report.protected_directories = protected_directories;

        let total = managed_bytes(&self.work_root, &self.receipt_root);
        report.bytes_before = total;
        if max_bytes == 0 || total <= max_bytes {
            report.bytes_after = total;
            report.dry_run = dry_run;
            return Ok(report);
        }
        let target = max_bytes.saturating_sub(max_bytes / 4);
        self.remove_candidates(candidates, target, dry_run, &mut report);
        Ok(report)
    }

    /// Explicit run-aware clean: delete every unprotected candidate directory
    /// outside the newest `keep_runs`, regardless of the byte budget.
    pub async fn clean_history(
        &self,
        history: &DagHistory,
        keep_runs: usize,
        dry_run: bool,
    ) -> Result<RunAwareCleanReport, String> {
        let (candidates, protected_directories, scanned_runs) =
            self.classify_candidates(history, keep_runs).await?;
        let mut report = self.base_report(keep_runs, scanned_runs);
        report.candidate_directories = candidates.len();
        report.protected_directories = protected_directories;
        report.bytes_before = managed_bytes(&self.work_root, &self.receipt_root);
        self.remove_candidates(candidates, 0, dry_run, &mut report);
        Ok(report)
    }

    /// Convenience wrapper used by the central manager.
    pub async fn run_aware_clean_once(
        &self,
        keep_runs: usize,
        max_bytes: u64,
        dry_run: bool,
        history: &DagHistory,
    ) -> Result<RunAwareCleanReport, String> {
        self.run_aware_clean(history, keep_runs, max_bytes, dry_run)
            .await
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct RunAwareCleanReport {
    pub keep_runs: usize,
    pub scanned_runs: usize,
    pub candidate_directories: usize,
    /// Old-run directories kept because a live session or the result cache
    /// still references them.
    pub protected_directories: usize,
    pub removed: usize,
    pub bytes_freed: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub dry_run: bool,
    /// VFS artifacts tracked in the central index. Reported only — never
    /// deleted by this cleaner.
    pub retained_artifacts: usize,
    pub retained_artifact_bytes: u64,
    pub errors: Vec<String>,
}

/// Runtime lifecycle policy, resolved from the environment.
#[derive(Debug, Clone, Copy)]
pub struct MaintenancePolicy {
    pub session_idle: Duration,
    pub output_cache_idle: Duration,
    pub result_cache_ttl: Duration,
    pub result_cache_max_entries: usize,
    pub maintenance_interval: Option<Duration>,
    pub clean_keep_runs: usize,
    pub storage_max_bytes: u64,
}

impl Default for MaintenancePolicy {
    fn default() -> Self {
        Self::from_env()
    }
}

impl MaintenancePolicy {
    pub fn from_env() -> Self {
        let secs = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        Self {
            session_idle: Duration::from_secs(secs("AUTONOMICS_DAG_SESSION_IDLE_SECS", 24 * 3600)),
            output_cache_idle: Duration::from_secs(secs(
                "AUTONOMICS_DAG_OUTPUT_CACHE_IDLE_SECS",
                3600,
            )),
            result_cache_ttl: Duration::from_secs(secs(
                "AUTONOMICS_DAG_RESULT_CACHE_TTL_SECS",
                7 * 24 * 3600,
            )),
            result_cache_max_entries: secs("AUTONOMICS_DAG_RESULT_CACHE_MAX_ENTRIES", 4096)
                as usize,
            maintenance_interval: match std::env::var("AUTONOMICS_DAG_MAINTENANCE_INTERVAL_SECS") {
                Ok(raw) => raw
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .map(Duration::from_secs),
                Err(_) => Some(Duration::from_secs(900)),
            },
            clean_keep_runs: secs("AUTONOMICS_DAG_CLEAN_KEEP_RUNS", 100) as usize,
            storage_max_bytes: std::env::var("AUTONOMICS_DAG_STORAGE_MAX_BYTES")
                .ok()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .unwrap_or(0),
        }
    }
}

/// Tracks the most recent actor command per session.
#[derive(Debug, Default)]
pub struct SessionTracker {
    last_activity: Mutex<HashMap<String, Instant>>,
}

impl SessionTracker {
    pub fn touch(&self, session_id: &str) {
        self.last_activity
            .lock()
            .insert(session_id.to_string(), Instant::now());
    }

    pub fn forget(&self, session_id: &str) {
        self.last_activity.lock().remove(session_id);
    }

    pub fn is_idle(&self, session_id: &str, idle: Duration) -> bool {
        self.last_activity
            .lock()
            .get(session_id)
            .is_some_and(|last| last.elapsed() >= idle)
    }
}

fn report_local_roots_from_value(
    value: &serde_json::Value,
    work_root: &Path,
    receipt_root: &Path,
) -> Vec<PathBuf> {
    let mut strings = Vec::new();
    collect_strings(value, &mut strings);
    strings
        .into_iter()
        .filter_map(|raw| managed_root_from_path(&raw, work_root, receipt_root))
        .collect()
}

fn collect_strings(value: &serde_json::Value, output: &mut Vec<String>) {
    match value {
        serde_json::Value::String(value) => output.push(value.clone()),
        serde_json::Value::Array(values) => values
            .iter()
            .for_each(|value| collect_strings(value, output)),
        serde_json::Value::Object(values) => values
            .values()
            .for_each(|value| collect_strings(value, output)),
        _ => {}
    }
}

/// Collect every `vfs://` URI in a serialized run report together with the
/// logical byte size recorded in the enclosing `FileRef` fingerprint, if any.
fn vfs_artifacts_from_value(value: &serde_json::Value) -> Vec<(String, u64)> {
    let mut artifacts = Vec::new();
    walk_vfs(value, &mut artifacts);
    artifacts
}

fn walk_vfs(value: &serde_json::Value, output: &mut Vec<(String, u64)>) {
    match value {
        serde_json::Value::String(raw) => {
            if raw.starts_with("vfs://") {
                output.push((normalize_vfs_uri(raw), 0));
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(path)) = map.get("path")
                && path.starts_with("vfs://")
            {
                let bytes = map
                    .get("fingerprint")
                    .and_then(|fp| fp.get("size"))
                    .and_then(|size| size.as_u64())
                    .unwrap_or(0);
                output.push((normalize_vfs_uri(path), bytes));
                return;
            }
            map.values().for_each(|value| walk_vfs(value, output));
        }
        serde_json::Value::Array(values) => values.iter().for_each(|value| walk_vfs(value, output)),
        _ => {}
    }
}

/// Normalize a `vfs://` URI so the same artifact is always indexed under one
/// key: collapse duplicate slashes (after the scheme) and strip trailing ones.
fn normalize_vfs_uri(raw: &str) -> String {
    let trimmed = raw.trim_end_matches('/');
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_string();
    };
    let mut normalized = format!("{scheme}://");
    let mut previous_slash = false;
    for ch in rest.chars() {
        if ch == '/' {
            if previous_slash {
                continue;
            }
            previous_slash = true;
        } else {
            previous_slash = false;
        }
        normalized.push(ch);
    }
    normalized
}

fn managed_root_from_path(raw: &str, work_root: &Path, receipt_root: &Path) -> Option<PathBuf> {
    let path = Path::new(raw);
    let canonical = path
        .canonicalize()
        .ok()
        .unwrap_or_else(|| path.to_path_buf());
    if let Ok(relative) = canonical.strip_prefix(work_root) {
        let mut components = relative.components();
        let shard = components.next()?.as_os_str().to_string_lossy().to_string();
        let hash = components.next()?.as_os_str().to_string_lossy().to_string();
        if shard.len() == 2 {
            return Some(work_root.join(shard).join(hash));
        }
        return Some(work_root.to_path_buf());
    }
    if let Ok(relative) = canonical.strip_prefix(receipt_root) {
        let first = relative.components().next()?.as_os_str();
        return Some(receipt_root.join(first));
    }
    None
}

fn canonical_or_raw(path: PathBuf) -> PathBuf {
    let _ = std::fs::create_dir_all(&path);
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn managed_bytes(work_root: &Path, receipt_root: &Path) -> u64 {
    dir_size(work_root).saturating_add(dir_size(receipt_root))
}

fn dir_size(path: &Path) -> u64 {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return 0,
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            }
            total += metadata.len();
        }
    }
    total
}

fn modified(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn remove_dir(path: &Path) -> std::io::Result<u64> {
    let bytes = dir_size(path);
    std::fs::remove_dir_all(path).map(|_| bytes)
}

/// Convenience alias used by [`crate::runtime`].
pub type SharedStorageCoordinator = Arc<StorageCoordinator>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::history::RunRecord;

    fn run_record(id: &str, started_at: &str, report_json: &str) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            ref_name: "main".into(),
            snapshot_id: None,
            manifest_hash: format!("hash-{id}"),
            trigger: None,
            started_at: started_at.into(),
            finished_at: started_at.into(),
            ok: true,
            cancelled: false,
            error: None,
            message: None,
            engine_version: "test".into(),
            source_revision: "test".into(),
            run_report_json: Some(report_json.into()),
        }
    }

    #[tokio::test]
    async fn run_aware_clean_protects_new_runs_and_active_references() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        let old = work.join("aa").join("b".repeat(62));
        let latest = work.join("cc").join("d".repeat(62));
        std::fs::create_dir_all(old.join("nested")).unwrap();
        std::fs::create_dir_all(latest.join("nested")).unwrap();
        std::fs::write(old.join("nested/out.txt"), "old").unwrap();
        std::fs::write(latest.join("nested/out.txt"), "latest").unwrap();

        let old_json = format!(r#"{{"path":"{}"}}"#, old.join("nested/out.txt").display());
        let latest_json = format!(
            r#"{{"path":"{}"}}"#,
            latest.join("nested/out.txt").display()
        );

        let history = DagHistory::open_in_memory().await.unwrap();
        history
            .record_run(&run_record("old", "2026-01-01T00:00:00Z", &old_json))
            .await
            .unwrap();
        history
            .record_run(&run_record("latest", "2026-01-02T00:00:00Z", &latest_json))
            .await
            .unwrap();

        let receipts = root.path().join("receipts");
        std::fs::create_dir_all(&receipts).unwrap();
        let coordinator = StorageCoordinator::new(&work, &receipts);
        let report = coordinator
            .run_aware_clean(&history, 1, 1, false)
            .await
            .unwrap();
        assert_eq!(report.removed, 1);
        assert!(!old.exists(), "only the old run's workdir is eligible");
        assert!(latest.exists(), "the newest run is protected");
    }

    #[tokio::test]
    async fn dry_run_reports_candidates_without_deleting() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        let old = work.join("aa").join("b".repeat(62));
        let latest = work.join("cc").join("d".repeat(62));
        std::fs::create_dir_all(old.join("nested")).unwrap();
        std::fs::create_dir_all(&latest).unwrap();
        std::fs::write(old.join("nested/out.txt"), "old").unwrap();
        let old_json = format!(r#"{{"path":"{}"}}"#, old.join("nested/out.txt").display());
        let latest_json = format!(r#"{{"path":"{}"}}"#, latest.display());

        let history = DagHistory::open_in_memory().await.unwrap();
        history
            .record_run(&run_record("old", "2026-01-01T00:00:00Z", &old_json))
            .await
            .unwrap();
        history
            .record_run(&run_record("latest", "2026-01-02T00:00:00Z", &latest_json))
            .await
            .unwrap();

        let coordinator = StorageCoordinator::new(&work, root.path().join("receipts"));
        let report = coordinator
            .run_aware_clean(&history, 1, 1, true)
            .await
            .unwrap();
        assert_eq!(report.dry_run, true);
        assert_eq!(report.candidate_directories, 1);
        assert_eq!(report.removed, 1);
        assert!(report.bytes_freed > 0);
        assert!(old.exists(), "dry-run must not touch the disk");
    }

    #[tokio::test]
    async fn vfs_artifacts_are_indexed_but_never_deleted() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        let old = work.join("aa").join("b".repeat(62));
        let latest = work.join("cc").join("d".repeat(62));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&latest).unwrap();
        std::fs::write(old.join("out.txt"), "old").unwrap();
        let report_json = format!(
            r#"{{"nodes":[{{"id":"sink","port_assignments":{{"0":{{"path":"vfs://bucket/out.parquet","fingerprint":{{"size":123}}}}}}}}],"path":"{}"}}"#,
            old.join("out.txt").display()
        );
        let history = DagHistory::open_in_memory().await.unwrap();
        history
            .record_run(&run_record("old", "2026-01-01T00:00:00Z", &report_json))
            .await
            .unwrap();
        history
            .record_run(&run_record(
                "latest",
                "2026-01-02T00:00:00Z",
                &format!(r#"{{"path":"{}"}}"#, latest.display()),
            ))
            .await
            .unwrap();

        let coordinator = StorageCoordinator::new(&work, root.path().join("receipts"));
        let report = coordinator
            .run_aware_clean(&history, 1, 1, false)
            .await
            .unwrap();
        assert_eq!(report.removed, 1, "the local workdir is still cleaned");
        assert!(!old.exists());
        assert_eq!(report.retained_artifacts, 1);
        assert_eq!(report.retained_artifact_bytes, 123);
        let artifacts = coordinator.artifact_snapshot();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].uri, "vfs://bucket/out.parquet");
        assert_eq!(artifacts[0].logical_bytes, 123);
        assert_eq!(artifacts[0].runs, 1);
    }

    #[tokio::test]
    async fn cache_owner_reference_protects_local_files() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        let cached = work.join("aa").join("b".repeat(62));
        let latest = work.join("cc").join("d".repeat(62));
        std::fs::create_dir_all(&cached).unwrap();
        std::fs::create_dir_all(&latest).unwrap();
        let file = cached.join("out.csv");
        std::fs::write(&file, "cached").unwrap();

        let old_json = format!(r#"{{"path":"{}"}}"#, file.display());
        let history = DagHistory::open_in_memory().await.unwrap();
        history
            .record_run(&run_record("old", "2026-01-01T00:00:00Z", &old_json))
            .await
            .unwrap();
        history
            .record_run(&run_record(
                "latest",
                "2026-01-02T00:00:00Z",
                &format!(r#"{{"path":"{}"}}"#, latest.display()),
            ))
            .await
            .unwrap();

        let coordinator = StorageCoordinator::new(&work, root.path().join("receipts"));
        coordinator.retain_cache_owner(&[CachedFileOutput {
            node_id: "sink".into(),
            fingerprint: "fp".into(),
            port: 0,
            file: FileRef::local(&file, Some("csv".into())).unwrap(),
        }]);

        let report = coordinator
            .run_aware_clean(&history, 1, 1, false)
            .await
            .unwrap();
        assert_eq!(report.removed, 0);
        assert_eq!(report.protected_directories, 1);
        assert!(file.exists(), "result-cache owner must protect the file");

        // Once the cache entry is evicted the reference is recyclable.
        coordinator.release_cache_owner(&[CachedFileOutput {
            node_id: "sink".into(),
            fingerprint: "fp".into(),
            port: 0,
            file: FileRef::local(&file, Some("csv".into())).unwrap(),
        }]);
        let report = coordinator
            .run_aware_clean(&history, 1, 1, false)
            .await
            .unwrap();
        assert_eq!(report.removed, 1);
        assert!(!file.exists());
    }

    #[test]
    fn vfs_uri_normalization_collapses_duplicates() {
        assert_eq!(normalize_vfs_uri("vfs://bucket//a//b/"), "vfs://bucket/a/b");
        assert_eq!(normalize_vfs_uri("vfs://bucket"), "vfs://bucket");
    }
}
