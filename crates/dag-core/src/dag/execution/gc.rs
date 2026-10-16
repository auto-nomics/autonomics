//! Retention for `LocalTaskExecutor` task workspaces.
//!
//! Every dispatched task leaves a receipt directory under the dag-tasks
//! root: `task.json`, `status.json`, `attempt.json`, captured logs, and
//! materialized `outputs/port-N.{arrow,json}` evidence. The materialized
//! files are never read back as data (DataFrames travel in memory), so the
//! tree is bounded only by this sweep — unlike container work dirs it holds
//! no canonical outputs, so age-based deletion is always safe.
//!
//! Receipt directories carry no lock files; a directory owned by a live
//! process (pid from the directory name) is retained regardless of age.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Environment override for the dag-tasks root.
const DAG_TASKS_ROOT_ENV: &str = "AUTONOMICS_DAG_TASKS_ROOT";
/// Minimum age before a dead-owner receipt directory is removed (seconds).
const DAG_TASKS_GC_AGE_ENV: &str = "AUTONOMICS_DAG_TASKS_GC_AGE_SECS";
/// Delay between sweeps (seconds); `0` sweeps once at startup and stops.
const DAG_TASKS_GC_INTERVAL_ENV: &str = "AUTONOMICS_DAG_TASKS_GC_INTERVAL_SECS";
/// Total-size ceiling for the dag-tasks tree (bytes); `0` disables the
/// watermark sweep.
const DAG_TASKS_GC_MAX_BYTES_ENV: &str = "AUTONOMICS_DAG_TASKS_GC_MAX_BYTES";

const DEFAULT_GC_AGE_SECS: u64 = 7 * 24 * 3600;
const DEFAULT_GC_INTERVAL_SECS: u64 = 6 * 3600;
const DEFAULT_GC_MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;
/// Headroom left below the ceiling once the watermark sweep has run, so one
/// sweep buys the next interval its budget back.
const WATERMARK_HEADROOM_DIVISOR: u64 = 4;
/// Live-owner receipts become watermark candidates only after this age: a
/// long-lived daemon owns every receipt it ever wrote, so pid liveness alone
/// cannot exempt them forever, but an in-flight task's workspace must not be
/// pulled out from under it either.
const WATERMARK_LIVE_OWNER_GRACE: Duration = Duration::from_secs(6 * 3600);

/// Prefix for directories renamed by a sweep that died mid-delete.
const GC_PENDING_PREFIX: &str = ".gc-";

/// Persistent root for DAG task receipts.
///
/// `$AUTONOMICS_DAG_TASKS_ROOT` when set, else `$HOME/.autonomics/state/
/// dag-tasks`, else a temp-dir fallback for headless environments without
/// a home directory.
pub fn dag_tasks_root() -> PathBuf {
    if let Some(root) = std::env::var_os(DAG_TASKS_ROOT_ENV) {
        return PathBuf::from(root);
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Path::new(&home)
            .join(".autonomics")
            .join("state")
            .join("dag-tasks");
    }
    std::env::temp_dir().join("autonomics").join("dag-tasks")
}

/// Policy for one sweep of the dag-tasks tree.
#[derive(Debug, Clone)]
pub struct DagTaskGcPolicy {
    /// Receipt directories younger than this are retained.
    pub min_age: Duration,
    /// Report what would happen without deleting anything.
    pub dry_run: bool,
}

/// Outcome of one dag-tasks sweep.
#[derive(Debug, Default, Clone, Serialize)]
pub struct DagTaskGcReport {
    /// Receipt directories considered.
    pub scanned: usize,
    /// Directories removed (or that would be, under `dry_run`).
    pub removed: usize,
    /// Bytes reclaimed.
    pub bytes_freed: u64,
    /// Younger than `min_age`.
    pub retained_recent: usize,
    /// Owned by a process that is still alive.
    pub retained_live_owner: usize,
    /// Entries that do not match the receipt naming scheme — never touched.
    pub foreign_entries: usize,
    /// Best-effort failures encountered during the sweep.
    pub errors: Vec<String>,
}

/// Outcome of one watermark sweep.
#[derive(Debug, Default, Clone, Serialize)]
pub struct DagTaskWatermarkReport {
    /// Receipt directories considered.
    pub scanned: usize,
    /// Directories removed.
    pub removed: usize,
    /// Total bytes under the root before the sweep.
    pub total_bytes_before: u64,
    /// Total bytes under the root after the sweep.
    pub total_bytes_after: u64,
    /// Ceiling the sweep targets; `None` when the sweep is disabled.
    pub max_bytes: Option<u64>,
    /// Live-owner directories skipped because they are younger than the
    /// watermark grace window.
    pub retained_live_recent: usize,
    /// Best-effort failures encountered during the sweep.
    pub errors: Vec<String>,
}

/// Enforce a total-size ceiling on the dag-tasks tree.
///
/// Runs after the age sweep in the same loop. When the tree exceeds
/// `max_bytes`, receipts are deleted oldest-first — dead-owner directories
/// first, then live-owner ones past the grace window (a long-lived daemon
/// owns every receipt it wrote, so pid liveness cannot exempt them forever) —
/// until the tree is back at 75% of the ceiling. Pending `.gc-` leftovers
/// count toward the total and are cleared first.
pub async fn sweep_dag_tasks_watermark(
    root: &Path,
    max_bytes: Option<u64>,
) -> DagTaskWatermarkReport {
    let (root, max_bytes) = (root.to_path_buf(), max_bytes);
    tokio::task::spawn_blocking(move || sweep_dag_tasks_watermark_blocking(&root, max_bytes))
        .await
        .unwrap_or_else(|join_error| {
            let mut report = DagTaskWatermarkReport::default();
            report.errors.push(format!(
                "dag task watermark sweep task failed: {join_error}"
            ));
            report
        })
}

fn sweep_dag_tasks_watermark_blocking(
    root: &Path,
    max_bytes: Option<u64>,
) -> DagTaskWatermarkReport {
    let mut report = DagTaskWatermarkReport {
        max_bytes,
        ..Default::default()
    };
    let Some(max_bytes) = max_bytes else {
        return report;
    };
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return report,
        Err(error) => {
            report
                .errors
                .push(format!("cannot read `{}`: {error}", root.display()));
            return report;
        }
    };

    let now = SystemTime::now();
    let mut total = 0u64;
    let mut candidates = Vec::<(bool, SystemTime, PathBuf, u64)>::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name.starts_with(GC_PENDING_PREFIX) {
            // Never added to the running total; just reclaim it.
            if let Err(error) = fs::remove_dir_all(&path) {
                report.errors.push(format!(
                    "cannot clear pending sweep target `{}`: {error}",
                    path.display()
                ));
            }
            continue;
        }
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let Some(pid) = parse_receipt_pid(&name) else {
            continue;
        };
        report.scanned += 1;
        let live = owner_is_live(pid);
        let modified = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok();
        let age = modified
            .and_then(|modified| now.duration_since(modified).ok())
            .unwrap_or_default();
        let bytes = dir_size(&path);
        total = total.saturating_add(bytes);
        // Dead owners are always candidates; live owners only once past the
        // grace window, so an in-flight workspace is never pulled mid-run.
        let eligible = !live || age >= WATERMARK_LIVE_OWNER_GRACE;
        if eligible {
            if let Some(modified) = modified {
                candidates.push((live, modified, path, bytes));
            }
        } else {
            report.retained_live_recent += 1;
        }
    }

    let target = max_bytes - max_bytes / WATERMARK_HEADROOM_DIVISOR;
    if total <= max_bytes {
        report.total_bytes_before = total;
        report.total_bytes_after = total;
        return report;
    }

    // Dead-owner first, then oldest-first within each class.
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    report.total_bytes_before = total;
    for (_, _, path, _) in candidates {
        if total <= target {
            break;
        }
        match remove_dir_atomically(root, &path) {
            Ok(bytes_removed) => {
                report.removed += 1;
                total = total.saturating_sub(bytes_removed);
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("cannot remove `{}`: {error}", path.display()));
            }
        }
    }
    report.total_bytes_after = total;
    report
}

/// Retention window for dead-owner receipt directories.
pub fn dag_task_gc_age() -> Duration {
    parse_env_secs(
        std::env::var_os(DAG_TASKS_GC_AGE_ENV).as_deref(),
        DEFAULT_GC_AGE_SECS,
    )
}

/// Delay between sweeps; `None` sweeps once and stops. `0` disables the
/// periodic loop (startup sweep only); invalid values fall back to the
/// default interval.
pub fn dag_task_gc_interval() -> Option<Duration> {
    let secs = std::env::var_os(DAG_TASKS_GC_INTERVAL_ENV)
        .and_then(|value| value.to_string_lossy().trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_GC_INTERVAL_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Total-size ceiling for the dag-tasks tree; `None` disables the watermark
/// sweep. Invalid values fall back to the default ceiling.
pub fn dag_task_gc_max_bytes() -> Option<u64> {
    std::env::var_os(DAG_TASKS_GC_MAX_BYTES_ENV)
        .and_then(|value| value.to_string_lossy().trim().parse::<u64>().ok())
        .map_or(Some(DEFAULT_GC_MAX_BYTES), |bytes| {
            (bytes > 0).then_some(bytes)
        })
}

fn parse_env_secs(raw: Option<&std::ffi::OsStr>, default: u64) -> Duration {
    raw.and_then(|value| value.to_string_lossy().trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map_or_else(|| Duration::from_secs(default), Duration::from_secs)
}

/// Sweep receipt directories under `root` by age.
///
/// Only `{pid}-{nanos}-{seq}` directories (the `LocalTaskWorkspace` naming
/// scheme) are candidates; anything else — including the `script-inputs/`
/// staging area — is counted as foreign and never touched. A directory
/// whose pid is still alive is always retained; deletions rename to
/// `.gc-{nanos}-{pid}` inside the root first, so they stay on one
/// filesystem and a crashed sweep leaves an unambiguous leftover.
pub async fn sweep_dag_tasks(root: &Path, policy: &DagTaskGcPolicy) -> DagTaskGcReport {
    let root = root.to_path_buf();
    let policy = policy.clone();
    tokio::task::spawn_blocking(move || sweep_dag_tasks_blocking(&root, &policy))
        .await
        .unwrap_or_else(|join_error| {
            let mut report = DagTaskGcReport::default();
            report
                .errors
                .push(format!("dag task sweep task failed: {join_error}"));
            report
        })
}

fn sweep_dag_tasks_blocking(root: &Path, policy: &DagTaskGcPolicy) -> DagTaskGcReport {
    let mut report = DagTaskGcReport::default();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return report,
        Err(error) => {
            report
                .errors
                .push(format!("cannot read `{}`: {error}", root.display()));
            return report;
        }
    };

    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(GC_PENDING_PREFIX) {
            // Judged dead by an earlier sweep that died mid-delete.
            if policy.dry_run {
                report.foreign_entries += 1;
            } else {
                match fs::remove_dir_all(entry.path()) {
                    Ok(()) => report.removed += 1,
                    Err(error) => report.errors.push(format!(
                        "cannot clear pending sweep target `{}`: {error}",
                        entry.path().display()
                    )),
                }
            }
            continue;
        }
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            report.foreign_entries += 1;
            continue;
        }
        let Some(pid) = parse_receipt_pid(&name) else {
            report.foreign_entries += 1;
            continue;
        };
        report.scanned += 1;
        let path = entry.path();
        if owner_is_live(pid) {
            report.retained_live_owner += 1;
            continue;
        }
        let age = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .unwrap_or_default();
        if age < policy.min_age {
            report.retained_recent += 1;
            continue;
        }
        if policy.dry_run {
            report.removed += 1;
            report.bytes_freed += dir_size(&path);
            continue;
        }
        match remove_dir_atomically(root, &path) {
            Ok(bytes) => {
                report.removed += 1;
                report.bytes_freed += bytes;
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("cannot remove `{}`: {error}", path.display()));
            }
        }
    }
    report
}

/// `{pid}-{nanos}-{seq}` — the `LocalTaskWorkspace` directory naming
/// scheme. Returns the owning pid.
fn parse_receipt_pid(name: &str) -> Option<u32> {
    let mut parts = name.split('-');
    let pid = parts.next()?;
    let nanos = parts.next()?;
    let sequence = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let _ = nanos.parse::<u64>().ok()?;
    let _ = sequence.parse::<u64>().ok()?;
    pid.parse::<u32>().ok()
}

/// Whether the process still exists. Plain existence check: pid recycling
/// can keep a dead receipt alive for one extra cycle, which is acceptable
/// for run evidence.
fn owner_is_live(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(unix))]
    {
        // No portable liveness probe; retain by age only.
        let _ = pid;
        false
    }
}

/// `rename` the directory under a fresh `.gc-{nanos}-{pid}` name, then
/// delete it. Concurrent sweepers race harmlessly: `rename` is atomic, so
/// exactly one wins and the loser sees a missing source.
fn remove_dir_atomically(root: &Path, dir: &Path) -> Result<u64, std::io::Error> {
    let bytes = dir_size(dir);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let pending = root.join(format!("{GC_PENDING_PREFIX}{nanos}-{}", std::process::id()));
    match fs::rename(dir, &pending) {
        Ok(()) => fs::remove_dir_all(&pending).map(|_| bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(bytes),
        Err(error) => Err(error),
    }
}

/// Recursive directory size. Symlinks count as their link size, never
/// their target, so a pathological tree cannot loop or escape.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            match fs::symlink_metadata(entry.path()) {
                Ok(metadata) if metadata.is_dir() => {
                    stack.push(entry.path());
                    total += metadata.len();
                }
                Ok(metadata) => total += metadata.len(),
                Err(_) => continue,
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_receipt(root: &Path, pid: u32, age: Duration) -> PathBuf {
        let dir = root.join(format!("{pid}-1791400000000000000-0"));
        fs::create_dir_all(dir.join("outputs")).unwrap();
        fs::write(dir.join("task.json"), "{}").unwrap();
        fs::write(dir.join("outputs").join("port-0.arrow"), "arrow").unwrap();
        set_age(&dir, age);
        dir
    }

    fn set_age(dir: &Path, age: Duration) {
        let past = SystemTime::now()
            .checked_sub(age)
            .expect("age within system time range");
        let file = fs::File::options().read(true).open(dir).unwrap();
        file.set_modified(past).unwrap();
    }

    #[test]
    fn receipt_names_parse_pid_and_reject_foreign() {
        assert_eq!(parse_receipt_pid("123-1791400000000000000-7"), Some(123));
        assert_eq!(parse_receipt_pid("0-1-0"), Some(0));
        assert_eq!(parse_receipt_pid("script-inputs"), None);
        assert_eq!(parse_receipt_pid("abc-1-2"), None);
        assert_eq!(parse_receipt_pid("1-2"), None);
        assert_eq!(parse_receipt_pid("1-2-3-4"), None);
        assert_eq!(parse_receipt_pid("1-x-3"), None);
    }

    #[test]
    fn dag_tasks_root_prefers_env_then_home() {
        // Env override wins over the home default.
        let guard = EnvGuard::set(DAG_TASKS_ROOT_ENV, "/tmp/dag-tasks-env-override");
        assert_eq!(
            dag_tasks_root(),
            PathBuf::from("/tmp/dag-tasks-env-override")
        );
        drop(guard);
        // Default derives from HOME when present (it is, in test envs).
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                dag_tasks_root(),
                Path::new(&home)
                    .join(".autonomics")
                    .join("state")
                    .join("dag-tasks")
            );
        }
    }

    struct EnvGuard(&'static str);
    impl EnvGuard {
        fn set(name: &'static str, value: &str) -> Self {
            // SAFETY: process-global env mutation; tests using this guard
            // run within dag-core's serial test profile for env-sensitive
            // cases (single-threaded per process by cargo's default).
            unsafe { std::env::set_var(name, value) };
            Self(name)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: restoring process-global state at test end.
            unsafe { std::env::remove_var(self.0) };
        }
    }

    #[tokio::test]
    async fn sweep_removes_old_dead_and_retains_young_and_live() {
        let root = tempfile::tempdir().unwrap();
        let dead_old = make_receipt(root.path(), 4_000_000, Duration::from_secs(8 * 24 * 3600));
        let dead_young = make_receipt(root.path(), 4_000_001, Duration::from_secs(60));
        let live_old = make_receipt(
            root.path(),
            std::process::id(),
            Duration::from_secs(8 * 24 * 3600),
        );
        let foreign = root.path().join("script-inputs");
        fs::create_dir_all(&foreign).unwrap();

        let report = sweep_dag_tasks(
            root.path(),
            &DagTaskGcPolicy {
                min_age: Duration::from_secs(7 * 24 * 3600),
                dry_run: false,
            },
        )
        .await;

        assert_eq!(report.scanned, 3);
        assert_eq!(report.removed, 1, "only the old dead receipt is removed");
        assert!(!dead_old.exists());
        assert!(dead_young.exists());
        assert!(
            live_old.exists(),
            "live owner is retained regardless of age"
        );
        assert!(foreign.exists(), "foreign entries are never touched");
        assert_eq!(report.retained_recent, 1);
        assert_eq!(report.retained_live_owner, 1);
        assert_eq!(report.foreign_entries, 1);
        assert!(report.bytes_freed > 0);
    }

    #[tokio::test]
    async fn sweep_clears_pending_leftovers() {
        let root = tempfile::tempdir().unwrap();
        let leftover = root.path().join(".gc-1-2");
        fs::create_dir_all(&leftover).unwrap();
        fs::write(leftover.join("task.json"), "{}").unwrap();

        let report = sweep_dag_tasks(
            root.path(),
            &DagTaskGcPolicy {
                min_age: Duration::from_secs(1),
                dry_run: false,
            },
        )
        .await;
        assert_eq!(report.scanned, 0);
        assert!(
            !leftover.exists(),
            "pending target is cleared unconditionally"
        );
    }

    #[tokio::test]
    async fn sweep_without_root_reports_nothing() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("absent");
        let report = sweep_dag_tasks(
            &missing,
            &DagTaskGcPolicy {
                min_age: Duration::from_secs(1),
                dry_run: false,
            },
        )
        .await;
        assert_eq!(report.scanned, 0);
        assert!(report.errors.is_empty());
    }

    fn make_sized_receipt(root: &Path, pid: u32, age: Duration, bytes: usize) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SIZED_SEQ: AtomicU64 = AtomicU64::new(0);
        let sequence = SIZED_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("{pid}-1791400000000000000-{sequence}"));
        fs::create_dir_all(dir.join("outputs")).unwrap();
        fs::write(dir.join("task.json"), "{}").unwrap();
        fs::write(dir.join("outputs").join("port-0.arrow"), vec![b'x'; bytes]).unwrap();
        set_age(&dir, age);
        dir
    }

    #[tokio::test]
    async fn watermark_noop_below_ceiling() {
        let root = tempfile::tempdir().unwrap();
        make_sized_receipt(root.path(), 4_000_000, Duration::from_secs(3600), 1024);
        let report = sweep_dag_tasks_watermark(root.path(), Some(1 << 20)).await;
        assert_eq!(report.scanned, 1);
        assert_eq!(report.removed, 0);
        assert_eq!(report.total_bytes_before, report.total_bytes_after);
        assert!(report.errors.is_empty());
    }

    #[tokio::test]
    async fn watermark_disabled_is_a_noop() {
        let root = tempfile::tempdir().unwrap();
        make_sized_receipt(root.path(), 4_000_000, Duration::from_secs(3600), 1024);
        let report = sweep_dag_tasks_watermark(root.path(), None).await;
        assert_eq!(report.scanned, 0);
        assert_eq!(report.removed, 0);
    }

    #[tokio::test]
    async fn watermark_reclaims_oldest_dead_first_down_to_target() {
        let root = tempfile::tempdir().unwrap();
        let unit = 8192;
        let oldest = make_sized_receipt(root.path(), 4_000_000, Duration::from_secs(900), unit);
        let middle = make_sized_receipt(root.path(), 4_000_001, Duration::from_secs(600), unit);
        let newest = make_sized_receipt(root.path(), 4_000_002, Duration::from_secs(300), unit);

        // Ceiling of three units (target 2.25): the ~3.01 units on disk push
        // past it, so exactly the oldest receipt goes.
        let report = sweep_dag_tasks_watermark(root.path(), Some(unit as u64 * 3)).await;
        assert_eq!(report.scanned, 3);
        assert_eq!(
            report.removed, 1,
            "only enough receipts to reach the target"
        );
        assert!(!oldest.exists(), "the oldest receipt is reclaimed first");
        assert!(middle.exists());
        assert!(newest.exists());
        assert!(report.total_bytes_after < report.total_bytes_before);
    }

    #[tokio::test]
    async fn watermark_retains_young_live_owner_but_reclaims_old_live_owner() {
        let root = tempfile::tempdir().unwrap();
        let unit = 4096;
        let live = std::process::id();
        let live_young = make_sized_receipt(root.path(), live, Duration::from_secs(300), unit);
        // Older than the 6h grace: eligible even though its owner is alive.
        let live_old = make_sized_receipt(root.path(), live, Duration::from_secs(7 * 3600), unit);

        let report = sweep_dag_tasks_watermark(root.path(), Some(unit as u64)).await;
        assert_eq!(report.removed, 1);
        assert!(
            live_young.exists(),
            "in-flight workspaces are never touched"
        );
        assert!(!live_old.exists(), "grace-expired live-owner receipts go");
        assert_eq!(report.retained_live_recent, 1);
    }

    #[test]
    fn gc_env_parsing_defaults_and_overrides() {
        // Defaults.
        let _ = EnvGuard::remove(DAG_TASKS_GC_AGE_ENV);
        let _ = EnvGuard::remove(DAG_TASKS_GC_INTERVAL_ENV);
        assert_eq!(dag_task_gc_age(), Duration::from_secs(7 * 24 * 3600));
        assert_eq!(dag_task_gc_interval(), Some(Duration::from_secs(6 * 3600)));

        // Overrides and disabled interval.
        let _age = EnvGuard::set(DAG_TASKS_GC_AGE_ENV, "3600");
        assert_eq!(dag_task_gc_age(), Duration::from_secs(3600));
        drop(_age);
        let _interval = EnvGuard::set(DAG_TASKS_GC_INTERVAL_ENV, "0");
        assert_eq!(dag_task_gc_interval(), None);
        drop(_interval);
        let _invalid = EnvGuard::set(DAG_TASKS_GC_AGE_ENV, "not-a-number");
        assert_eq!(
            dag_task_gc_age(),
            Duration::from_secs(7 * 24 * 3600),
            "invalid values fall back to the default"
        );
    }

    impl EnvGuard {
        fn remove(name: &'static str) -> Self {
            // SAFETY: process-global env mutation, restored on drop.
            unsafe { std::env::remove_var(name) };
            Self(name)
        }
    }
}
