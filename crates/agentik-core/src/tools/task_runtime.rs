use crate::agent::InternalEvent;
use crate::tools::function::ProgressRecord;
use agentik_sdk::ToolResult;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::tools::error::ToolError;
use crate::tools::function::ProgressBuffer;

pub type TaskId = String;

// ─────────────────────────── TaskStore ───────────────────────────

/// Task list with a monotonic sequence counter.
///
/// Wraps `Vec<TaskEntry>` (via `Deref`/`DerefMut`) so all existing Vec
/// operations work unchanged. The additional [`alloc_seq`] method hands out
/// short, 1-based task numbers that the LLM uses to reference background tasks
/// — far friendlier than long `tool_use_id` UUIDs.
pub struct TaskStore {
    tasks: Vec<TaskEntry>,
    seq_counter: AtomicU64,
}

impl TaskStore {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            seq_counter: AtomicU64::new(0),
        }
    }

    /// Allocate the next sequential task number (1-based).
    /// Uses atomic `fetch_add` so a read-lock holder can call this.
    pub fn alloc_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Find a task by its sequence number.
    pub fn find_by_seq(&self, seq: u64) -> Option<&TaskEntry> {
        self.tasks.iter().find(|t| t.seq == seq)
    }
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Deref for TaskStore {
    type Target = Vec<TaskEntry>;
    fn deref(&self) -> &Self::Target {
        &self.tasks
    }
}

impl std::ops::DerefMut for TaskStore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tasks
    }
}

/// Lifecycle of a spawned tool invocation.
#[derive(Clone)]
pub enum TaskStatus {
    Running,
    Done(ToolResult),
    Failed(ToolError),
}

/// Sender type for background task completion notifications.
pub type BgTaskNotifyTx = tokio::sync::mpsc::UnboundedSender<InternalEvent>;

/// A single tool invocation tracked by [`Toolset`](super::toolset::Toolset).
///
/// Status is self-managed via a `watch` channel. A monitor task owns the
/// `JoinHandle` and updates the channel when the tool completes, so callers
/// can retrieve the result at any time via [`status`](Self::status) or
/// [`changed`](Self::changed).
///
/// When an async task completes, the monitor task sends
/// [`InternalEvent::BgTaskComplete`] through the optional `notify_tx`,
/// waking the agent so it can decide whether to pull the result via
/// `view_task_results` / `wait_task`. Sync tasks are consumed inline by
/// [`Toolset::execute`] and never need `notify_tx`.
pub struct TaskEntry {
    /// Short 1-based task number (allocated by [`TaskStore::alloc_seq`]).
    /// Used by the LLM to reference background tasks via `wait_task` /
    /// `view_task_results` / `view_task_status`.
    seq: u64,
    id: TaskId,
    /// The tool's display name (e.g. "run_bash"), distinct from the task id.
    name: String,
    status: watch::Receiver<TaskStatus>,
    cancel_token: CancellationToken,
    read: watch::Receiver<bool>,
    read_tx: watch::Sender<bool>,
    /// Structured, append-only progress buffer shared with the executing tool
    /// (via [`crate::tools::ToolContext`]). Readable at any time by observers
    /// (`view_task_status`). NOTE: this carries real-time / intermediate
    /// progress, distinct from the final tool result below.
    output: ProgressBuffer,
    /// Final output of tool result, only readable when tool execution has done.
    tool_result: watch::Receiver<Option<ToolResult>>,
}

impl TaskEntry {
    /// Spawn a monitor task that awaits the `JoinHandle` and updates the
    /// status channel on completion. The handle is consumed here; callers
    /// read results exclusively through the watch channel.
    ///
    /// Simplified constructor without notification — used by tests.
    pub fn new(
        seq: u64,
        id: TaskId,
        name: String,
        handle: JoinHandle<Result<ToolResult, ToolError>>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self::with_notify(
            seq,
            id,
            name,
            handle,
            cancel_token,
            None,
            Arc::new(Mutex::new(crate::tools::function::ProgressLog::new())),
        )
    }

    /// Create a `TaskEntry` with optional agent notification on completion.
    ///
    /// `notify_tx`: when `Some`, the monitor task sends
    /// [`InternalEvent::BgTaskComplete`] when the tool finishes. Pass `Some`
    /// for **async** tools (their completion needs to wake the agent so it
    /// can pull the result); pass `None` for **sync** tools (consumed inline
    /// by `execute()`, no wake-up needed).
    ///
    /// `output` is the shared progress buffer the executing tool pushes
    /// structured [`ProgressRecord`]s onto (so it must be created before the
    /// tool runs).
    pub fn with_notify(
        seq: u64,
        id: TaskId,
        name: String,
        handle: JoinHandle<Result<ToolResult, ToolError>>,
        cancel_token: CancellationToken,
        notify_tx: Option<BgTaskNotifyTx>,
        output: ProgressBuffer,
    ) -> Self {
        let (status_tx, status) = watch::channel(TaskStatus::Running);
        let (read_tx, read) = watch::channel(false);
        let (tool_result_tx, tool_result) = watch::channel::<Option<ToolResult>>(None);

        let tx = status_tx.clone();
        let out = output.clone();
        let bg_notify = notify_tx.clone();
        let spwan_ts_tx = tool_result_tx.clone();
        let task_id = id.clone();
        let task_seq = seq;
        crate::supervise::spawn_safe_drop(&format!("task_monitor::{name}"), async move {
            match handle.await {
                Ok(Ok(tool_result)) => {
                    // Store the real result for later retrieval.
                    spwan_ts_tx.send(Some(tool_result.clone())).ok();
                    tx.send(TaskStatus::Done(tool_result)).ok();
                }
                Ok(Err(e)) => {
                    if let Ok(mut buf) = out.lock() {
                        buf.push(ProgressRecord::new("error").message(e.to_string()));
                    }
                    tx.send(TaskStatus::Failed(e)).ok();
                }
                Err(join_err) => {
                    let msg = format!("task panicked: {join_err}");
                    if let Ok(mut buf) = out.lock() {
                        buf.push(ProgressRecord::new("error").message(msg));
                    }
                    tx.send(TaskStatus::Failed(ToolError::ExecutionFailed {
                        source: Box::new(join_err),
                    }))
                    .ok();
                }
            }
            // Notify the agent's event loop when an async task finishes so
            // it can pull the result. notify_tx is Some only for async tools
            // — sync tools are consumed inline by execute() and never need
            // a wake-up.
            if let Some(notify) = bg_notify {
                let _ = notify.send(InternalEvent::BgTaskComplete {
                    id: task_id,
                    seq: task_seq,
                });
            }
        });

        Self {
            seq,
            id,
            name,
            status,
            cancel_token,
            read,
            read_tx,
            output,
            tool_result,
        }
    }

    /// Return the short task sequence number (1-based).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Return the task identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Return the tool's display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Non-blocking read of current status.
    ///
    /// When the task completes, returns `TaskStatus::Done(result)` —
    /// the result is embedded in the status itself.
    pub fn status(&self) -> TaskStatus {
        self.status.borrow().clone()
    }

    /// Check whether the task completed successfully, without cloning the
    /// full result. Used by the notification path to decide success vs.
    /// failure message wording without pulling the (potentially large)
    /// result content.
    pub fn is_done(&self) -> bool {
        matches!(*self.status.borrow(), TaskStatus::Done(_))
    }

    /// The real final result of the tool, once execution has finished.
    ///
    /// Returns `None` while the task is still running (no result stored yet).
    pub fn tool_result(&self) -> Option<ToolResult> {
        self.tool_result.borrow().clone()
    }

    /// Wait for the next status change (e.g. `Running` → `Done`).
    pub async fn changed(&mut self) {
        self.status.changed().await.ok();
    }

    /// Signal cancellation to the running task.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Wait for the task to complete (Done or Failed). No timeout —
    /// the caller is responsible for wrapping this in a `tokio::time::timeout`
    /// if needed (timeout is enforced inside the spawned task).
    pub async fn wait_for_result(&mut self) -> ToolResult {
        self.status.changed().await.ok();
        let seq = self.seq;
        match self.status.borrow().clone() {
            TaskStatus::Done(result) => {
                self.read_tx.send(true).ok();
                result
            }
            TaskStatus::Failed(err) => {
                self.read_tx.send(true).ok();
                ToolResult::error(err.to_string()).with_id(&self.id)
            }
            // Spurious wake — return a placeholder. The caller should retry.
            TaskStatus::Running => ToolResult::from_pending_task(&self.id, seq),
        }
    }

    pub fn is_read(&self) -> bool {
        *self.read.borrow()
    }

    /// Mark this task's result as consumed.
    pub fn mark_read(&self) {
        self.read_tx.send(true).ok();
    }

    /// Non-blocking snapshot of all retained progress records (oldest-first).
    pub fn output(&self) -> Vec<ProgressRecord> {
        self.output.lock().map(|v| v.snapshot()).unwrap_or_default()
    }

    /// Number of records evicted from the head by the [`ProgressLog`] cap.
    pub fn dropped(&self) -> usize {
        self.output.lock().map(|v| v.dropped()).unwrap_or_default()
    }

    /// Clone of the shared progress buffer, so an external subscriber can read
    /// the same live-output stream the tool is pushing to.
    pub fn output_buffer(&self) -> ProgressBuffer {
        Arc::clone(&self.output)
    }

    /// Clone the internal status watch receiver.
    ///
    /// Callers can `await receiver.changed()` outside any lock to wait for
    /// task completion without holding the task list's `RwLock`.
    pub fn status_receiver_clone(&self) -> watch::Receiver<TaskStatus> {
        self.status.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_task_completion() {
        let mut task = TaskEntry::new(
            1,
            "test-task-1".into(),
            "test_tool".into(),
            tokio::spawn(async { Ok(ToolResult::success("done")) }),
            CancellationToken::new(),
        );

        // Task completes quickly — wait_for_result should return the result.
        assert!(matches!(task.status(), TaskStatus::Running));
        let result = task.wait_for_result().await;
        assert!(result.text_content().contains("done"));
        assert!(task.is_read());
    }
}
