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

const DEFAULT_GC_AGE_SECS: u64 = 7 * 24 * 3600;
const DEFAULT_GC_INTERVAL_SECS: u64 = 6 * 3600;

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
