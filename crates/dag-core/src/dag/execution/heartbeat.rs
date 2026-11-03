//! Advisory, attempt-bound worker heartbeats. No execution admission, output
//! success, scientific release or process-subtree stop proof is inferred here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};

pub const FILE_NAME: &str = ".autonomics-worker-heartbeat.json";

pub struct WorkerHeartbeat {
    state: Arc<(Mutex<(bool, String)>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WorkerHeartbeat {
    /// Only ordinary Slurm workers opt in. Failure to write advisory telemetry
    /// leaves the computation unchanged and the coordinator's liveness unknown.
    pub fn start(manifest: &Path, task_id: &str) -> Option<Self> {
        let job_id = std::env::var("SLURM_JOB_ID").ok()?;
        Self::start_for_job(manifest, task_id, job_id)
    }

    fn start_for_job(manifest: &Path, task_id: &str, job_id: String) -> Option<Self> {
        if job_id.is_empty() || !job_id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // Advisory telemetry must not turn a malformed manifest into an
        // unbounded allocation. The worker validates its dispatch separately.
        use std::io::Read;
        let file = std::fs::File::open(manifest).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 16 * 1024 * 1024 {
            return None;
        }
        let root = manifest.parent()?.to_path_buf();
        let process = std::fs::read_to_string("/proc/self/stat").ok()?;
        let start = process
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(19)?
            .to_string();
        let identity = json!({"schema":"AUTONOMICS_WORKER_HEARTBEAT_V1",
            "task_id":task_id, "job_id":job_id, "manifest_sha256":format!("{:x}",Sha256::digest(&bytes)),
            "pid":std::process::id(), "process_start_ticks":start,
            "boot_id":std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?.trim(),
            "hostname":std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?.trim()});
        let state = Arc::new((
            Mutex::new((false, "preparing_worker".to_string())),
            Condvar::new(),
        ));
        let observed = state.clone();
        let thread = std::thread::Builder::new()
            .name("worker-observation".into())
            .spawn(move || {
                let mut sequence = 0u64;
                loop {
                    let (stop, phase) =
                        observed.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let mut sample = identity.clone();
                    sample["state"] = if stop { "exiting" } else { "active" }.into();
                    sample["phase"] = phase.into();
                    sample["sequence"] = sequence.into();
                    sample["observed_at_ms"] = crate::dag::live::observation_time_ms().into();
                    let _ = publish(&root, &sample);
                    if stop {
                        break;
                    }
                    sequence = sequence.saturating_add(1);
                    let guard = observed.0.lock().unwrap_or_else(|e| e.into_inner());
                    drop(observed.1.wait_timeout_while(
                        guard,
                        Duration::from_secs(5),
                        |(stop, _)| !*stop,
                    ));
                }
            })
            .ok()?;
        Some(Self {
            state,
            thread: Some(thread),
        })
    }

    pub fn phase(&self, phase: &str) {
        self.state.0.lock().unwrap_or_else(|e| e.into_inner()).1 = phase.into();
    }
}

impl Drop for WorkerHeartbeat {
    fn drop(&mut self) {
        self.state.0.lock().unwrap_or_else(|e| e.into_inner()).0 = true;
        self.state.1.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn publish(root: &Path, sample: &serde_json::Value) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path: PathBuf = root.join(format!("{FILE_NAME}.pending-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(sample)?)?;
        std::fs::rename(&path, root.join(FILE_NAME))
    })();
    if result.is_err() {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(held), Ok(named)) = (file.metadata(), std::fs::symlink_metadata(&path)) {
            if held.dev() == named.dev() && held.ino() == named.ino() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_binds_exact_manifest_and_exits_without_claiming_worker_stop() {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join("task.json");
        std::fs::write(&manifest, b"synthetic manifest\n").unwrap();
        let heartbeat = WorkerHeartbeat::start_for_job(&manifest, "s2", "42".into()).unwrap();
        heartbeat.phase("executing");
        drop(heartbeat);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(value["task_id"], "s2");
        assert_eq!(value["job_id"], "42");
        assert_eq!(
            value["manifest_sha256"],
            format!("{:x}", Sha256::digest(b"synthetic manifest\n"))
        );
        assert_eq!(value["state"], "exiting");
        assert_eq!(value["phase"], "executing");
        assert_eq!(value["pid"], std::process::id());
        assert!(
            value["process_start_ticks"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .is_ok()
        );
        assert!(!value.to_string().contains("synthetic manifest"));
    }

    #[test]
    fn failed_pending_admission_does_not_remove_foreign_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join(format!("{FILE_NAME}.pending-{}", std::process::id()));
        std::fs::write(&path, b"retain original").unwrap();
        assert!(publish(root.path(), &json!({"synthetic":true})).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"retain original");
    }
}
