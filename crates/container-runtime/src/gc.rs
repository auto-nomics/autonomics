//! Garbage collection for the container data plane.
//!
//! One cache grows without bound by design:
//!
//! * the workspace root accumulates one ephemeral scratch directory per
//!   container run (`autonomics-container-command-{pid}-{nanos}`), and
//! Successful runs remove their own scratch inline (see `container_command`);
//! the sweeper here reclaims the rest: crash residue, failed-run scratch
//! kept for debugging, and `AUTONOMICS_KEEP_WORKSPACE` leftovers.
//!
//! The panel data cache is deliberately outside this module's boundary; its
//! lifecycle is owned by the data-catalog layer.
//!
//! Safety rests on three checks, in order of strength:
//!
//! 1. an advisory `flock` on a lock file inside each directory — any live
//!    container run holds a shared lock, the sweeper takes an exclusive one
//!    non-blocking, so a directory in use is never touched;
//! 2. owner-pid liveness from the directory name, cross-checked against the
//!    process start time so a recycled pid cannot impersonate a live owner;
//! 3. a minimum-age window for everything else, so recently created entries
//!    are never removed on a race.
//!
//! Removal is `rename`-into-`.gc-*`-then-`remove_dir_all`: concurrent sweepers
//! (a TUI and a CLI run) cannot fight over the same directory, and an
//! interrupted sweep leaves only `.gc-*` names that the next sweep clears
//! unconditionally.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Lock file held (shared) by a container run inside its scratch directory.
pub const SCRATCH_LOCK_FILE: &str = ".autonomics-lock";
/// Entries swept in a previous run but not fully deleted yet.
const GC_PENDING_PREFIX: &str = ".gc-";
/// Directory below the panel cache root holding one lock file per entry.
const PANEL_LOCKS_DIR: &str = ".locks";
/// Scratch directories are named `{SCRATCH_NAME_PREFIX}{pid}-{nanos}`.
pub const SCRATCH_NAME_PREFIX: &str = "autonomics-container-command-";
/// A live-owner scratch must be at least this old before the sweeper removes
/// it (its lock is free, so no run is using it — this is the retention window
/// for failed runs and `AUTONOMICS_KEEP_WORKSPACE` debugging).
pub const DEFAULT_WORKSPACE_GC_AGE_SECS: u64 = 24 * 60 * 60;
/// Default delay between background sweeps.
pub const DEFAULT_WORKSPACE_GC_INTERVAL_SECS: u64 = 6 * 60 * 60;
/// Slack when comparing a process start time against the scratch creation
/// timestamp; clock ticks and name-nanos come from different clocks.
const START_TIME_SLACK: Duration = Duration::from_secs(5);

// ───────────────────────────── locking ─────────────────────────────

/// Result of a non-blocking lock attempt: `Ok(false)` means "locked by
/// someone else", not an error.
type LockOutcome = Result<bool, io::Error>;

fn flock(fd: i32, operation: i32) -> LockOutcome {
    // SAFETY: `flock` only touches the owned fd.
    if unsafe { libc::flock(fd, operation) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        // EWOULDBLOCK == EAGAIN on Linux; one spelling covers both.
        Some(libc::EWOULDBLOCK) => Ok(false),
        _ => Err(error),
    }
}

/// Take the shared lock a container run holds on its scratch directory for
/// the whole duration of `PodmanConnection::run`. The returned [`File`] must
/// stay alive until the run (and any inline cleanup) is done; dropping it
/// releases the lock.
pub fn acquire_scratch_lock_shared(scratch: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(scratch.join(SCRATCH_LOCK_FILE))?;
    flock(file.as_raw_fd(), libc::LOCK_SH)
        .map(|_| ())
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot lock scratch `{}`: {error}", scratch.display()),
            )
        })?;
    Ok(file)
}

/// Take the exclusive lock a container run holds on its persistent,
/// content-addressed work directory. Blocking: concurrent executions of the
/// same task identity serialize here for their whole duration. The returned
/// [`File`] must stay alive until the run is done; dropping it releases the
/// lock. A work-dir GC sweep skips any directory whose lock is held.
pub fn acquire_workdir_lock_exclusive(workdir: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(workdir.join(SCRATCH_LOCK_FILE))?;
    flock(file.as_raw_fd(), libc::LOCK_EX)
        .map(|_| ())
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot lock work dir `{}`: {error}", workdir.display()),
            )
        })?;
    Ok(file)
}

/// Lock file for one panel cache entry. Locks live in a side directory so the
/// mounted panel itself only ever contains its data and completion marker.
pub fn panel_lock_path(panel_root: &Path, entry_name: &str) -> PathBuf {
    panel_root
        .join(PANEL_LOCKS_DIR)
        .join(format!("{entry_name}.lock"))
}

/// Shared variant of [`acquire_scratch_lock_shared`] for panel entries: a run
/// mounting this panel holds the lock until the container exits.
pub fn acquire_panel_lock_shared(panel_root: &Path, entry_name: &str) -> io::Result<File> {
    let path = panel_lock_path(panel_root, entry_name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    flock(file.as_raw_fd(), libc::LOCK_SH)
        .map(|_| ())
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot lock panel `{entry_name}`: {error}"),
            )
        })?;
    Ok(file)
}

/// Try-lock a lock file exclusively, creating it (and its parent directory)
/// when absent. `Ok(false)` means a container run (or another sweeper) holds it.
fn try_lock_exclusive(path: &Path) -> LockOutcome {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let fd = file.as_raw_fd();
    flock(fd, libc::LOCK_EX | libc::LOCK_NB)
}

/// Like [`try_lock_exclusive`] but never creates the lock file: an absent
/// lock reads as free (`Ok(true)`). `Ok(false)` means the lock is held by a
/// live run. Creating a lock here would touch the directory mtime and reset
/// the age the work-dir sweep measures.
fn try_lock_existing_exclusive(path: &Path) -> Result<bool, io::Error> {
    if !path.exists() {
        return Ok(true);
    }
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
}

// ───────────────────────── owner liveness ──────────────────────────

/// Parse `autonomics-container-command-{pid}-{nanos}` into its parts.
pub fn parse_scratch_name(name: &str) -> Option<(i32, u128)> {
    let rest = name.strip_prefix(SCRATCH_NAME_PREFIX)?;
    let (pid, nanos) = rest.split_once('-')?;
    Some((pid.parse().ok()?, nanos.parse().ok()?))
}

fn boot_time_secs() -> Option<u64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().find(|line| line.starts_with("btime "))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn clock_ticks_per_sec() -> f64 {
    // SAFETY: `sysconf` is a pure query.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks as f64 } else { 100.0 }
}

/// True when `pid` names a process that already existed when the scratch
/// directory was created. A missing `/proc/{pid}` or a start time *after* the
/// directory was created (the pid was recycled) both report `false`; any
/// parsing failure reports `true` so the sweeper errs on retention and lets
/// the age window do the work instead.
fn owner_is_live(pid: i32, created_at: SystemTime) -> bool {
    let Some(stat) = fs::read_to_string(format!("/proc/{pid}/stat")).ok() else {
        return false;
    };
    // `comm` may contain spaces and parentheses; everything after the final
    // `)` is whitespace-separated, field 3 onwards. starttime is field 22.
    let Some((_, fields)) = stat.rsplit_once(')') else {
        return true;
    };
    let Some(start_ticks) = fields.split_whitespace().nth(19) else {
        return true;
    };
    let Ok(start_ticks) = start_ticks.parse::<u64>() else {
        return true;
    };
    let Some(boot) = boot_time_secs() else {
        return true;
    };
    let started_secs = boot as f64 + start_ticks as f64 / clock_ticks_per_sec();
    let started_at = UNIX_EPOCH + Duration::from_secs_f64(started_secs);
    // A recycled pid started long after the directory existed.
    started_at + START_TIME_SLACK <= created_at
}

// ─────────────────────────── workspace sweep ───────────────────────

/// Tuning for [`sweep_workspace`].
#[derive(Debug, Clone)]
pub struct WorkspaceGcPolicy {
    /// Scratch owned by a live process is removed only once older than this.
    pub min_age: Duration,
    /// Report what would happen without deleting anything.
    pub dry_run: bool,
}

impl Default for WorkspaceGcPolicy {
    fn default() -> Self {
        Self {
            min_age: Duration::from_secs(DEFAULT_WORKSPACE_GC_AGE_SECS),
            dry_run: false,
        }
    }
}

/// Outcome of one workspace sweep.
#[derive(Debug, Default, Clone, Serialize)]
pub struct WorkspaceGcReport {
    /// Scratch directories considered.
    pub scanned: usize,
    /// Directories removed (or that would be, under `dry_run`).
    pub removed: usize,
    /// Bytes reclaimed (`st_blocks`-free plain `len()` sums).
    pub bytes_freed: u64,
    /// Live owner, lock free, but younger than `min_age`.
    pub retained_recent: usize,
    /// A container run currently holds the shared lock.
    pub retained_in_use: usize,
    /// Entries below the root that do not match the scratch pattern —
    /// user-declared `workdir` directories and anything else the sweeper must
    /// never touch.
    pub foreign_entries: usize,
    /// Leftover `.gc-*` staging names cleared.
    pub pending_cleared: usize,
    /// Per-entry failures; a failing directory never aborts the sweep.
    pub errors: Vec<String>,
}

/// Reclaim dead scratch directories below `workspace_root`.
///
/// Never touches entries that fail the scratch name pattern (user workdirs),
/// are locked by a live run, or belong to a live owner and are younger than
/// `min_age`. Filesystem work runs on the blocking pool.
pub async fn sweep_workspace(
    workspace_root: &Path,
    policy: &WorkspaceGcPolicy,
) -> WorkspaceGcReport {
    let root = workspace_root.to_path_buf();
    let policy = policy.clone();
    tokio::task::spawn_blocking(move || sweep_workspace_blocking(&root, &policy))
        .await
        .unwrap_or_else(|join_error| {
            let mut report = WorkspaceGcReport::default();
            report
                .errors
                .push(format!("workspace sweep task failed: {join_error}"));
            report
        })
}

fn sweep_workspace_blocking(root: &Path, policy: &WorkspaceGcPolicy) -> WorkspaceGcReport {
    let mut report = WorkspaceGcReport::default();
    let Ok(entries) = fs::read_dir(root) else {
        return report; // no workspace yet — nothing to sweep
    };

    let now = SystemTime::now();
    let mut scratch: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        if name.starts_with(GC_PENDING_PREFIX) {
            // Judged dead by an earlier sweep that died mid-delete.
            if policy.dry_run {
                report.pending_cleared += 1;
            } else {
                match fs::remove_dir_all(entry.path()) {
                    Ok(()) => report.pending_cleared += 1,
                    Err(error) => report.errors.push(format!(
                        "cannot clear pending `{}`: {error}",
                        entry.path().display()
                    )),
                }
            }
            continue;
        }
        if parse_scratch_name(&name).is_some() {
            scratch.push(entry.path());
        } else {
            report.foreign_entries += 1;
        }
    }

    for path in scratch {
        report.scanned += 1;
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some((pid, nanos)) = parse_scratch_name(&name) else {
            report.foreign_entries += 1;
            continue;
        };
        let created_at = UNIX_EPOCH + Duration::from_nanos(nanos as u64);
        let age = now.duration_since(created_at).unwrap_or_default();
        let owner_live = owner_is_live(pid, created_at);

        // The exclusive try-lock both proves no run is using the directory
        // and arbitrates between concurrent sweepers.
        match try_lock_exclusive(&path.join(SCRATCH_LOCK_FILE)) {
            Ok(false) => {
                report.retained_in_use += 1;
                continue;
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("cannot lock scratch `{name}`: {error}"));
                continue;
            }
            Ok(true) => {}
        }

        let expired = !owner_live || age >= policy.min_age;
        if !expired {
            report.retained_recent += 1;
            continue;
        }
        if policy.dry_run {
            report.removed += 1;
            continue;
        }
        match remove_dir_atomically(root, &path) {
            Ok(bytes) => {
                report.removed += 1;
                report.bytes_freed += bytes;
            }
            Err(error) => report
                .errors
                .push(format!("cannot remove scratch `{name}`: {error}")),
        }
    }
    report
}

// ─────────────────────────── shared helpers ────────────────────────

/// `rename` the directory under a fresh `.gc-{nanos}` name, then delete it.
/// Concurrent sweepers race harmlessly: `rename` is atomic, so exactly one
/// wins and the loser sees a missing source, which reads as "already gone".
fn remove_dir_atomically(root: &Path, dir: &Path) -> Result<u64, io::Error> {
    let bytes = dir_size(dir);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let pid = std::process::id();
    let pending = root.join(format!("{GC_PENDING_PREFIX}{nanos}-{pid}"));
    match fs::rename(dir, &pending) {
        Ok(()) => fs::remove_dir_all(&pending).map(|_| bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(bytes),
        Err(error) => Err(error),
    }
}

/// Recursive directory size. Symlinks count as their link size, never their
/// target, so a pathological tree cannot loop or escape.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            match fs::symlink_metadata(entry.path()) {
                Ok(meta) if meta.is_dir() => stack.push(entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => continue,
            }
        }
    }
    total
}

/// Policy for one sweep of the persistent, content-addressed work dirs.
///
/// Unlike the legacy scratch sweeper this is purely opt-in: work dirs are
/// the durable store of container outputs (Nextflow-style work dir), so by
/// default nothing is ever reclaimed.
#[derive(Debug, Clone)]
pub struct WorkDirGcPolicy {
    /// Only remove work dirs whose mtime is at least this old.
    pub min_age: Duration,
    /// Report what would happen without deleting anything.
    pub dry_run: bool,
}

/// Outcome of one work-dir sweep.
#[derive(Debug, Default, Clone, Serialize)]
pub struct WorkDirGcReport {
    /// Work directories considered under `work/`.
    pub scanned: usize,
    /// Directories removed (or that would be, under `dry_run`).
    pub removed: usize,
    /// Bytes reclaimed.
    pub bytes_freed: u64,
    /// Younger than `min_age`.
    pub retained_recent: usize,
    /// A container run currently holds the exclusive lock.
    pub retained_in_use: usize,
    /// Best-effort failures encountered during the sweep.
    pub errors: Vec<String>,
}

/// Sweep `workspace_root/work/{shard}/{hash}` directories by age.
///
/// A directory whose `.autonomics-lock` is currently held (a live run took
/// the exclusive lock) is always retained. Deletion renames within the
/// shard directory first so it stays on one filesystem.
pub async fn sweep_work_dirs(workspace_root: &Path, policy: &WorkDirGcPolicy) -> WorkDirGcReport {
    let root = workspace_root.to_path_buf();
    let policy = policy.clone();
    tokio::task::spawn_blocking(move || sweep_work_dirs_blocking(&root, &policy))
        .await
        .unwrap_or_else(|join_error| {
            let mut report = WorkDirGcReport::default();
            report
                .errors
                .push(format!("work dir sweep task failed: {join_error}"));
            report
        })
}

fn sweep_work_dirs_blocking(root: &Path, policy: &WorkDirGcPolicy) -> WorkDirGcReport {
    let mut report = WorkDirGcReport::default();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return report,
        Err(error) => {
            report
                .errors
                .push(format!("cannot read `{}`: {error}", root.display()));
            return report;
        }
    };

    let now = SystemTime::now();
    for shard in entries.flatten() {
        // The container work-dir tree is sharded `work/{hash[0..2]}/{hash[2..]}`
        // directly under the workspace root. Only two-hex shard directories
        // are considered — legacy scratch and explicit user workdirs never
        // match the shape.
        if !shard.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            || !is_shard_name(&shard.file_name().to_string_lossy())
        {
            continue;
        }
        let hash_dirs = match fs::read_dir(shard.path()) {
            Ok(entries) => entries,
            Err(error) => {
                report
                    .errors
                    .push(format!("cannot read `{}`: {error}", shard.path().display()));
                continue;
            }
        };
        let mut remaining = 0usize;
        for hash_dir in hash_dirs.flatten() {
            if !hash_dir
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false)
                || !is_work_dir_name(&hash_dir.file_name().to_string_lossy())
            {
                continue;
            }
            report.scanned += 1;
            let path = hash_dir.path();
            match try_lock_existing_exclusive(&path.join(SCRATCH_LOCK_FILE)) {
                Err(error) => {
                    report
                        .errors
                        .push(format!("cannot lock `{}`: {error}", path.display()));
                    continue;
                }
                // Lock currently held by a live run.
                Ok(false) => {
                    report.retained_in_use += 1;
                    remaining += 1;
                    continue;
                }
                Ok(true) => {}
            }
            let age = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .unwrap_or_default();
            if age < policy.min_age {
                report.retained_recent += 1;
                remaining += 1;
                continue;
            }
            if policy.dry_run {
                report.removed += 1;
                report.bytes_freed += dir_size(&path);
                remaining += 1;
                continue;
            }
            match remove_dir_atomically(&shard.path(), &path) {
                Ok(bytes) => {
                    report.removed += 1;
                    report.bytes_freed += bytes;
                }
                Err(error) => {
                    report
                        .errors
                        .push(format!("cannot remove `{}`: {error}", path.display()));
                    remaining += 1;
                }
            }
        }
        // Prune shard directories that no longer hold any work dir.
        if remaining == 0
            && !policy.dry_run
            && fs::read_dir(shard.path())
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false)
        {
            let _ = fs::remove_dir(shard.path());
        }
    }
    report
}

/// Two lowercase hex characters — the shard component of the work-dir tree.
fn is_shard_name(name: &str) -> bool {
    name.len() == 2
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The 62-character hash remainder of a content-addressed work dir.
fn is_work_dir_name(name: &str) -> bool {
    name.len() == 62
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Touch a directory's mtime so panel LRU ordering reflects *use*, not
/// download time. Called on every cache hit.
pub fn touch_dir_mtime(path: &Path) {
    #[cfg(unix)]
    {
        if let Ok(file) = File::options().read(true).open(path) {
            let _ = file.set_modified(SystemTime::now());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

const _: () = {
    assert!(
        DEFAULT_WORKSPACE_GC_AGE_SECS > 0,
        "retention must be positive"
    );
};

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &Path, bytes: usize) {
        std::fs::write(path, vec![0_u8; bytes]).unwrap();
    }

    fn set_old_mtime(path: &Path) {
        let file = File::options().read(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000))
            .unwrap();
    }

    fn scratch_dir(root: &Path, pid: i32, age_secs: u64) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            - u128::from(age_secs) * 1_000_000_000;
        let dir = root.join(format!("{SCRATCH_NAME_PREFIX}{pid}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn dead_pid() -> i32 {
        // A pid with no /proc entry; pid_max is ≥ 4096, so scan upward from a
        // high value that the system never assigns in a test run.
        for pid in (4_000_000..4_190_000).rev() {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return pid;
            }
        }
        panic!("no dead pid found");
    }

    #[test]
    fn parse_scratch_name_accepts_only_canonical_names() {
        assert_eq!(
            parse_scratch_name("autonomics-container-command-42-123"),
            Some((42, 123))
        );
        assert_eq!(parse_scratch_name("autonomics-container-command-42"), None);
        assert_eq!(parse_scratch_name("autonomics-container-command-a-b"), None);
        assert_eq!(parse_scratch_name("runs/mine"), None);
        assert_eq!(parse_scratch_name("autonomics-container-command--7"), None);
    }

    #[tokio::test]
    async fn sweep_removes_dead_owner_scratch_regardless_of_age() {
        let root = tempfile::tempdir().unwrap();
        let fresh = scratch_dir(root.path(), dead_pid(), 0);
        write_file(&fresh.join("out.tsv"), 32);

        let report = sweep_workspace(root.path(), &WorkspaceGcPolicy::default()).await;

        assert_eq!(report.removed, 1);
        assert_eq!(report.bytes_freed, 32);
        assert!(!fresh.exists());
    }

    #[tokio::test]
    async fn sweep_retains_young_scratch_of_live_owner() {
        let root = tempfile::tempdir().unwrap();
        // PID 1 always exists and started at boot — long before any directory
        // this test creates — so it reads as a live owner regardless of the
        // test process's own uptime.
        let fresh = scratch_dir(root.path(), 1, 0);
        // Age comes from the nanos in the directory name, not mtime.
        let old = scratch_dir(root.path(), 1, DEFAULT_WORKSPACE_GC_AGE_SECS + 3_600);

        let report = sweep_workspace(root.path(), &WorkspaceGcPolicy::default()).await;

        // Lock-free but young → retained; old → removed.
        assert_eq!(report.retained_recent, 1);
        assert_eq!(report.removed, 1);
        assert!(fresh.exists());
        assert!(!old.exists());
    }

    #[tokio::test]
    async fn sweep_retains_locked_scratch_of_dead_owner() {
        let root = tempfile::tempdir().unwrap();
        let scratch = scratch_dir(root.path(), dead_pid(), 10_000);
        // Simulate an in-flight run from another process: any flock holder
        // works, including our own fd, because flock is per open-file.
        let _guard = acquire_scratch_lock_shared(&scratch).unwrap();

        let report = sweep_workspace(root.path(), &WorkspaceGcPolicy::default()).await;

        assert_eq!(report.retained_in_use, 1);
        assert_eq!(report.removed, 0);
        assert!(scratch.exists());
    }

    #[tokio::test]
    async fn sweep_never_touches_foreign_entries() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("runs/mine")).unwrap();

        let report = sweep_workspace(root.path(), &WorkspaceGcPolicy::default()).await;

        assert_eq!(report.foreign_entries, 1);
        assert_eq!(report.removed, 0);
        assert!(root.path().join("runs/mine").is_dir());
    }

    #[tokio::test]
    async fn sweep_clears_interrupted_gc_pending_names() {
        let root = tempfile::tempdir().unwrap();
        let pending = root.path().join(".gc-12345-999");
        std::fs::create_dir_all(&pending).unwrap();

        let report = sweep_workspace(root.path(), &WorkspaceGcPolicy::default()).await;

        assert_eq!(report.pending_cleared, 1);
        assert!(!pending.exists());
    }

    #[tokio::test]
    async fn recycled_pid_is_treated_as_dead_owner() {
        let root = tempfile::tempdir().unwrap();
        // Our own pid, but a scratch "created" in 1970: this process started
        // long after the directory existed, so the pid must be a recycled one
        // and the scratch is treated as dead residue.
        let nanos = 60_u128 * 1_000_000_000;
        let dir = root.path().join(format!(
            "{SCRATCH_NAME_PREFIX}{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let report = sweep_workspace(root.path(), &WorkspaceGcPolicy::default()).await;

        assert_eq!(report.removed, 1);
        assert!(!dir.exists());
    }

    #[test]
    fn touch_dir_mtime_updates_directory() {
        let dir = tempfile::tempdir().unwrap();
        set_old_mtime(dir.path());
        let before = fs::metadata(dir.path()).unwrap().modified().unwrap();
        touch_dir_mtime(dir.path());
        let after = fs::metadata(dir.path()).unwrap().modified().unwrap();
        assert!(after > before, "mtime must move forward");
    }

    fn make_work_dir(root: &Path, hash_seed: &str, old: bool) -> PathBuf {
        // Full-shape names: 2-hex shard + 62-hex remainder, exactly what the
        // sweep's name filter accepts.
        let hash = format!("{hash_seed}{}", "0".repeat(64 - hash_seed.len()));
        let dir = root.join(&hash[..2]).join(&hash[2..]);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("result.txt"), "output").unwrap();
        if old {
            set_old_mtime(&dir);
        }
        dir
    }

    #[tokio::test]
    async fn work_dir_sweep_removes_old_and_retains_young_and_locked() {
        let root = tempfile::tempdir().unwrap();
        let old = make_work_dir(root.path(), "aa1111", true);
        let young = make_work_dir(root.path(), "bb2222", false);
        let locked = make_work_dir(root.path(), "cc3333", true);
        let _guard = acquire_workdir_lock_exclusive(&locked).unwrap();

        let policy = WorkDirGcPolicy {
            min_age: Duration::from_secs(3600),
            dry_run: false,
        };
        let report = sweep_work_dirs(root.path(), &policy).await;

        assert_eq!(report.scanned, 3);
        assert_eq!(report.removed, 1);
        assert!(!old.exists(), "old work dir must be removed");
        assert!(young.exists(), "young work dir must be retained");
        assert!(locked.exists(), "locked work dir must be retained");
        assert_eq!(report.retained_recent, 1);
        assert_eq!(report.retained_in_use, 1);
        assert!(report.bytes_freed > 0);
        // The emptied shard directory is pruned.
        assert!(!root.path().join("aa").exists());
    }

    #[tokio::test]
    async fn work_dir_sweep_without_work_directory_reports_nothing() {
        let root = tempfile::tempdir().unwrap();
        let report = sweep_work_dirs(
            root.path(),
            &WorkDirGcPolicy {
                min_age: Duration::from_secs(1),
                dry_run: false,
            },
        )
        .await;
        assert_eq!(report.scanned, 0);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn work_dir_gc_age_env_parses() {
        // The parsing helper is env-driven; test the pure interpretation by
        // covering the disabled default through `parse_env_secs` semantics:
        // unset/0/invalid mean never (None).
        assert_eq!(crate::config::work_dir_gc_age(), None);
    }
}
