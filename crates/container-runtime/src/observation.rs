//! Advisory execution observations. Payloads contain identifiers/counters only,
//! never request argv, scripts, environment values or credentials.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeObservation {
    pub phase: String,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub unit: Option<String>,
    pub message: Option<String>,
    pub worker: Option<Value>,
    pub observed_at_ms: u64,
}

#[derive(Clone, Default)]
pub struct RunObserver(Option<Arc<dyn Fn(RuntimeObservation) + Send + Sync>>);

impl RunObserver {
    pub fn new(callback: impl Fn(RuntimeObservation) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(callback)))
    }

    pub fn emit(&self, mut observation: RuntimeObservation) {
        observation.observed_at_ms = now_ms();
        if let Some(callback) = &self.0 {
            callback(observation);
        }
    }

    pub fn phase(&self, phase: &str, message: &str) {
        self.emit(RuntimeObservation {
            phase: phase.into(),
            message: Some(message.into()),
            ..Default::default()
        });
    }

    pub fn progress(&self, phase: &str, current: u64, total: u64) {
        self.emit(RuntimeObservation {
            phase: phase.into(),
            current: Some(current),
            total: Some(total),
            unit: Some("bytes".into()),
            ..Default::default()
        });
    }

    pub fn process(&self, phase: &str, pid: u32, state: &str, role: &str) {
        self.emit(RuntimeObservation {
            phase: phase.into(),
            worker: Some(json!({
                "evidence":"process", "id":pid.to_string(), "state":state,
                "role":role, "observed_at_ms":now_ms()
            })),
            ..Default::default()
        });
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

/// Reap the exact child while periodically reporting its wait state. A running
/// wrapper is identified as such; this does not certify payload progress or a
/// stopped process subtree. Cancellation retains the caller's kill/stop policy.
pub async fn child_output(
    mut child: tokio::process::Child,
    observer: &RunObserver,
    role: &str,
) -> std::io::Result<std::process::Output> {
    use tokio::io::AsyncReadExt;
    let pid = child.id().unwrap_or(0);
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    async fn read(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut bytes).await?;
        }
        Ok(bytes)
    }
    let wait = async {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                result = child.wait() => {
                    match &result {
                        Ok(_) => observer.process("collecting_outputs", pid, "terminal", role),
                        Err(_) => observer.process("executing", pid, "unknown", role),
                    }
                    break result;
                }
                _ = tick.tick() => observer.process("executing", pid, "running", role),
            }
        }
    };
    let (status, stdout, stderr) = tokio::try_join!(wait, read(stdout), read(stderr))?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exact_child_wait_reports_process_evidence_without_output_or_argv() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let saved = events.clone();
        let observer = RunObserver::new(move |event| saved.lock().unwrap().push(event));
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "printf synthetic-private-output; sleep 0.02"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let output = child_output(child, &observer, "container_attach")
            .await
            .unwrap();
        assert!(output.status.success());
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.worker.as_ref().is_some_and(|w| w["state"] == "running"))
        );
        let terminal = events.last().unwrap().worker.as_ref().unwrap();
        assert_eq!(terminal["id"], pid.to_string());
        assert_eq!(terminal["state"], "terminal");
        assert_eq!(terminal["role"], "container_attach");
        assert!(
            !serde_json::to_string(&*events)
                .unwrap()
                .contains("synthetic-private-output")
        );
    }

    #[tokio::test]
    async fn cancelled_observer_does_not_fabricate_terminal_receipt() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let saved = events.clone();
        let started = Arc::new(tokio::sync::Notify::new());
        let signal = started.clone();
        let observer = RunObserver::new(move |event| {
            saved.lock().unwrap().push(event);
            signal.notify_one();
        });
        let child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let waiter =
            tokio::spawn(async move { child_output(child, &observer, "test_child").await });
        started.notified().await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.worker.as_ref().is_some_and(|w| w["state"] == "terminal"))
        );
        // The fixture owns/reaps its exact toy process, without treating the
        // cancellation event above as evidence of termination.
        for _ in 0..100 {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("toy child was not reaped");
    }
}
