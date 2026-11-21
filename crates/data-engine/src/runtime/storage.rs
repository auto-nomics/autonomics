//! Central lifecycle services for the per-agent DataEngine actors.
//!
//! [`DataEngineManager`](super::DataEngineManager) owns one instance of each
//! service. They provide the process-wide policies that individual actor
//! engines cannot safely decide alone: idle eviction, run-aware local
//! cleanup, and a managed-root byte budget.
//!
//! # Storage governance semantics (conservative)
//!
//! The [`StorageCoordinator`] counts **VFS artifacts** (`vfs://` URIs) from
//! recorded run reports and reports them through
//! [`StorageCoordinator::artifact_snapshot`] and [`RunAwareCleanReport`] —
//! but **never deletes them**: immutable object-store artifacts are treated
//! as durable provenance. Only managed local roots (`AUTONOMICS_DAG_WORK_ROOT`,
//! `AUTONOMICS_DAG_RECEIPTS_ROOT`) are deletable, and only for runs outside
//! the newest `keep_runs`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use serde::Serialize;

use crate::dag::history::DagHistory;

/// Reference counts for one tracked VFS artifact.
#[derive(Debug, Clone, Default)]
struct ArtifactUsage {
    runs: HashSet<String>,
    bytes: u64,
}

/// One row of [`StorageCoordinator::artifact_snapshot`].
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactEntry {
    pub uri: String,
    pub runs: usize,
    pub logical_bytes: u64,
}

/// Tracks VFS artifacts mentioned by recorded history. Local workdirs and
/// receipt roots are never indexed here: they are classified directly from
/// the run reports during run-aware cleanup.
#[derive(Debug, Default)]
pub struct StorageCoordinator {
    work_root: PathBuf,
    receipt_root: PathBuf,
    artifacts: Mutex<HashMap<String, ArtifactUsage>>,
}

impl StorageCoordinator {
    pub fn new(work_root: impl Into<PathBuf>, receipt_root: impl Into<PathBuf>) -> Self {
        Self {
            work_root: canonical_or_raw(work_root.into()),
            receipt_root: canonical_or_raw(receipt_root.into()),
            artifacts: Mutex::new(HashMap::new()),
        }
    }

    pub fn work_root(&self) -> &Path {
        &self.work_root
    }

    pub fn receipt_root(&self) -> &Path {
        &self.receipt_root
    }

    /// Classify managed local directories for run-aware cleanup: collect
    /// directories that belong only to runs outside the newest `keep_runs`.
    async fn classify_candidates(
        &self,
        history: &DagHistory,
        keep_runs: usize,
    ) -> Result<(HashSet<PathBuf>, usize), String> {
        let keep_runs = keep_runs.max(1);
        let runs = history
            .list_runs(keep_runs, None)
            .await
            .map_err(|error| error.to_string())?;
        let protected_runs: HashSet<_> = runs.iter().map(|run| run.id.as_str()).collect();

        // list_runs is newest-first. Re-list all runs to classify old paths
        // and index the VFS artifacts every run mentions.
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

        let mut candidates = HashSet::<PathBuf>::new();
        for report_json in &candidate_runs {
            for root in
                report_local_roots_from_value(report_json, &self.work_root, &self.receipt_root)
            {
                candidates.insert(root);
            }
        }
        Ok((candidates, all_runs.len()))
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
    /// Immutable VFS artifacts are indexed and reported, but never touched.
    pub async fn run_aware_clean(
        &self,
        history: &DagHistory,
        keep_runs: usize,
        max_bytes: u64,
        dry_run: bool,
    ) -> Result<RunAwareCleanReport, String> {
        let (candidates, scanned_runs) = self.classify_candidates(history, keep_runs).await?;
        let mut report = self.base_report(keep_runs, scanned_runs);
        report.candidate_directories = candidates.len();

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

    /// Explicit run-aware clean: delete every candidate directory outside the
    /// newest `keep_runs`, regardless of the byte budget.
    pub async fn clean_history(
        &self,
        history: &DagHistory,
        keep_runs: usize,
        dry_run: bool,
    ) -> Result<RunAwareCleanReport, String> {
        let (candidates, scanned_runs) = self.classify_candidates(history, keep_runs).await?;
        let mut report = self.base_report(keep_runs, scanned_runs);
        report.candidate_directories = candidates.len();
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

    pub fn artifact_snapshot(&self) -> Vec<ArtifactEntry> {
        self.artifacts
            .lock()
            .iter()
            .map(|(uri, entry)| ArtifactEntry {
                uri: uri.clone(),
                runs: entry.runs.len(),
                logical_bytes: entry.bytes,
            })
            .collect()
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct RunAwareCleanReport {
    pub keep_runs: usize,
    pub scanned_runs: usize,
    pub candidate_directories: usize,
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

/// Tracks the most recent actor command per agent.
#[derive(Debug, Default)]
pub struct AgentActivityTracker {
    last_activity: Mutex<HashMap<String, Instant>>,
}

impl AgentActivityTracker {
    pub fn touch(&self, agent_id: &str) {
        self.last_activity
            .lock()
            .insert(agent_id.to_string(), Instant::now());
    }

    pub fn forget(&self, agent_id: &str) {
        self.last_activity.lock().remove(agent_id);
    }

    pub fn is_idle(&self, agent_id: &str, idle: Duration) -> bool {
        self.last_activity
            .lock()
            .get(agent_id)
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
            if vfs::uri::storage_path(raw).is_some() {
                output.push((normalize_vfs_uri(raw), 0));
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(path)) = map.get("path")
                && vfs::uri::storage_path(path).is_some()
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
    async fn run_aware_clean_protects_new_runs() {
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
        assert!(report.dry_run);
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

    #[test]
    fn vfs_uri_normalization_collapses_duplicates() {
        assert_eq!(normalize_vfs_uri("vfs://bucket//a//b/"), "vfs://bucket/a/b");
        assert_eq!(normalize_vfs_uri("vfs://bucket"), "vfs://bucket");
    }
}
