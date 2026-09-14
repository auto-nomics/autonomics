//! Single-writer advisory lock for the runtime state directory.
//!
//! `RuntimeHost::open` acquires an exclusive `flock` on
//! `<state_dir>/runtime.lock` before any database is opened. Two Autonomics
//! processes pointing at the same `state_dir` (e.g. the gateway daemon and
//! a second `tui serve`) would otherwise double-write `agent.db` /
//! `bib.db`; database-level dual-writer consistency is a much bigger
//! problem surface than mutual exclusion, so the second instance fails
//! fast with [`Error::InstanceLockHeld`](crate::Error::InstanceLockHeld)
//! instead.
//!
//! Frontends that only *talk* to the running host (the thin-client TUI,
//! web, desktop) never call `RuntimeHost::open` and are unaffected. The
//! standalone `kms` / `bib` / `cache` CLI subcommands open the databases
//! read-mostly and have coexisted with a running TUI for a long time; the
//! lock deliberately does not try to cover them.
//!
//! The lock is advisory and tied to the open file description: it is held
//! by the returned guard for the host's lifetime and released by `drop` —
//! a crashed holder leaves no stale lock behind.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Lock file name inside the state directory.
const LOCK_FILE: &str = "runtime.lock";

/// An held single-writer lock. Dropping it releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Path of the underlying lock file (for error reporting).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Acquire the exclusive single-writer lock for `state_dir`.
///
/// Contention is a hard error, never a reason to block startup: the
/// caller (`tui serve`) should surface the error and exit so the operator
/// connects to the running instance instead of racing it.
pub fn acquire(state_dir: &Path) -> Result<InstanceLock> {
    let path = state_dir.join(LOCK_FILE);
    std::fs::create_dir_all(state_dir)
        .map_err(|e| Error::Other(format!("create {}: {e}", state_dir.display())))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&path)
        .map_err(|e| Error::Other(format!("open {}: {e}", path.display())))?;
    // LOCK_NB: fail fast on contention.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return match err.raw_os_error() {
            // EWOULDBLOCK and EAGAIN share a value on Linux; both spell
            // "another open file description holds the lock".
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => {
                Err(Error::InstanceLockHeld { path })
            }
            _ => Err(Error::Other(format!("flock {}: {err}", path.display()))),
        };
    }
    Ok(InstanceLock { _file: file, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_on_same_dir_fails_then_succeeds_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path()).expect("first acquire succeeds");
        // flock is per open file description, so a second open of the same
        // file conflicts even within one process — exactly the gateway ↔
        // second-serve race the lock exists for.
        let err = acquire(dir.path()).expect_err("second acquire must fail");
        assert!(
            matches!(err, Error::InstanceLockHeld { ref path } if path == &dir.path().join(LOCK_FILE)),
            "expected InstanceLockHeld, got {err:?}"
        );
        drop(first);
        acquire(dir.path()).expect("lock is released on drop");
    }

    #[test]
    fn different_state_dirs_do_not_conflict() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _first = acquire(a.path()).unwrap();
        let _second = acquire(b.path()).unwrap();
    }
}
