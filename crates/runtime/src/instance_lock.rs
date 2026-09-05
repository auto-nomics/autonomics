//! Single-writer advisory lock for the runtime state directory.
//!
//! `RuntimeHost::open` acquires an exclusive `flock` on
//! `<state_dir>/runtime.lock` before any database is opened. Two Autonomics
//! processes pointing at the same `state_dir` (the TUI and the desktop
//! shell share `~/.autonomics` by default) would otherwise double-write
//! `agent.db` / `bib.db`; SQLite/Turso-level dual-writer consistency is a
//! much bigger problem surface than mutual exclusion, so the second
//! instance fails fast with [`Error::InstanceLockHeld`] instead
//! (docs/design/web-agent-runtime.md §9).
//!
//! The lock is advisory and tied to the open file description: it is held
//! by the returned `File` for the `RuntimeHost`'s lifetime and released by
//! `drop` — a crashed holder leaves no stale lock behind.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use crate::error::{Error, Result};

/// Lock file name inside the state directory.
const LOCK_FILE: &str = "runtime.lock";

/// Acquire the exclusive single-writer lock for `state_dir`.
///
/// The returned `File` owns the lock — keep it alive for the process
/// lifetime (the `RuntimeHost` does); dropping it releases the lock.
pub(crate) fn acquire(state_dir: &Path) -> Result<File> {
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
    // LOCK_NB: contention is a hard error, never a reason to block startup.
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
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_on_same_dir_fails_then_succeeds_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path()).expect("first acquire succeeds");
        // flock is per open file description, so a second open of the same
        // file conflicts even within one process — exactly the TUI ↔
        // desktop race the lock exists for.
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
