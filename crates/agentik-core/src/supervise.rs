//! Panic-safe task spawning utilities.
//!
//! Every `tokio::spawn` in the system should go through [`spawn_safe`] or
//! [`spawn_safe_drop`] instead of bare `tokio::spawn`.  The wrappers use
//! `catch_unwind` to turn a panicking future into an `Err(TaskPanic)` that is
//! logged with a backtrace, so a bug in one task can never silently kill a
//! background worker without leaving a trace.

use std::panic::AssertUnwindSafe;
use std::future::Future;

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
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}

/// Spawn a future on the tokio runtime with panic catching.
///
/// If the future panics, the panic message and a backtrace are logged via
/// `tracing::error!` and the `JoinHandle` resolves to
/// `Ok(Err(TaskPanic))`.
///
/// Use this when the caller cares about the result.  For fire-and-forget
/// tasks where nobody awaits the `JoinHandle`, prefer [`spawn_safe_drop`].
pub fn spawn_safe<F, T>(task_name: &str, future: F) -> JoinHandle<Result<T, TaskPanic>>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let task_name = task_name.to_string();
    tokio::spawn(async move {
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
                Err(TaskPanic { task: task_name, msg })
            }
        }
    })
}

/// Fire-and-forget variant of [`spawn_safe`].
///
/// Spawns the future, catches panics, logs them, and discards the result.
/// Use this for tasks where nobody checks the `JoinHandle` — e.g. background
/// persistence, fire-and-forget notifications.
pub fn spawn_safe_drop<F, T>(task_name: &str, future: F)
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    spawn_safe(task_name, future); // JoinHandle intentionally dropped
}

/// Like [`spawn_safe`] but spawns on a specific runtime handle.
///
/// Use this in the TUI and host code where `Handle::spawn` is preferred over
/// the implicit current-thread runtime.
pub fn spawn_safe_on<F, T>(
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
                Err(TaskPanic { task: task_name, msg })
            }
        }
    })
}

/// Fire-and-forget variant of [`spawn_safe_on`].
pub fn spawn_safe_on_drop<F, T>(handle: &tokio::runtime::Handle, task_name: &str, future: F)
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    spawn_safe_on(handle, task_name, future); // JoinHandle intentionally dropped
}
