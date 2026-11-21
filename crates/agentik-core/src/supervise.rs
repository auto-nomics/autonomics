//! Panic-safe task spawning utilities.
//!
//! Every `tokio::spawn` in the system should go through [`spawn_safe`]
//! instead of bare `tokio::spawn`.  The wrapper uses `catch_unwind` to turn a
//! panicking future into an `Err(TaskPanic)` that is logged with a backtrace,
//! so a bug in one task can never silently kill a background worker without
//! leaving a trace.
//!
//! Fire-and-forget callers write `let _ = spawn_safe(...)`: dropping the
//! `JoinHandle` does not abort the task, and panics are already logged inside.

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use tokio::task::JoinHandle;

/// Error returned when a spawned task panics.
#[derive(Debug, thiserror::Error)]
#[error("task '{task}' panicked: {msg}")]
pub struct TaskPanic {
    pub task: String,
    pub msg: String,
}

/// Extract a human-readable message from a panic payload box.
fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.to_string();
    }
    "<non-string panic payload>".to_string()
}

/// Spawn a future on the given tokio runtime handle with panic catching.
///
/// If the future panics, the panic message and a backtrace are logged via
/// `tracing::error!` and the `JoinHandle` resolves to `Ok(Err(TaskPanic))`.
///
/// Callers that need the result await the `JoinHandle`; fire-and-forget
/// callers write `let _ = spawn_safe(...)` — the task keeps running after
/// the handle is dropped and panics are still logged.  To spawn on the
/// current runtime, pass `&tokio::runtime::Handle::current()`.
pub fn spawn_safe<F, T>(
    handle: &tokio::runtime::Handle,
    task_name: &str,
    future: F,
) -> JoinHandle<Result<T, TaskPanic>>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let task_name = task_name.to_string();
    handle.spawn(async move {
        match AssertUnwindSafe(future).catch_unwind().await {
            Ok(val) => Ok(val),
            Err(payload) => {
                let msg = panic_payload_to_string(&payload);
                let bt = std::backtrace::Backtrace::force_capture();
                tracing::error!(
                    target: "spawn_safe",
                    task = %task_name,
                    panic = %msg,
                    backtrace = %bt,
                    "spawned task panicked"
                );
                Err(TaskPanic {
                    task: task_name,
                    msg,
                })
            }
        }
    })
}
