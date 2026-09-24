use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::tools::task_runtime::{TaskStatus, TaskStore};
use crate::tools::{ToolError, ToolFunction};

#[tool(
    name = "wait_task",
    description = "Wait for a background task to finish. \
                  If the task is already done, returns its result immediately. \
                  If the task is still running, the agent pauses (exits its processing loop) \
                  and will be automatically resumed with the task's result when it completes \
                  or when the timeout expires. \
                  If the task does not exist, returns an error."
)]
pub struct WaitTaskInput {
    #[desc = "Task number (#N) of the background task to wait for, as shown when it was spawned"]
    task: u64,
    #[desc = "Maximum seconds to sleep before checking in on a still-running task. \
              You are woken IMMEDIATELY when the task finishes — this value does NOT \
              delay the result, it only caps the idle sleep. For delegate_to analyses \
              use 1800 or more. Defaults to 600."]
    #[default = 600]
    timeout_seconds: Option<u64>,
}

pub struct WaitTaskTool {
    tasks: Arc<tokio::sync::RwLock<TaskStore>>,
}

impl WaitTaskTool {
    pub fn new(tasks: Arc<tokio::sync::RwLock<TaskStore>>) -> Self {
        Self { tasks }
    }

    /// Read the actual result of a completed task.
    async fn read_result(&self, task_seq: u64) -> Result<AgentToolResult, ToolError> {
        let tasks = self.tasks.read().await;
        let Some(task) = tasks.iter().find(|t| t.seq() == task_seq) else {
            return Ok(AgentToolResult::error(format!(
                "task #{task_seq} no longer exists"
            )));
        };

        match task.tool_result() {
            Some(result) => {
                // Result consumed — mark the task as read so the toolset can
                // reclaim its entry on the next execution pass.
                task.mark_read();
                let is_error = result.is_error.unwrap_or(false);
                Ok(AgentToolResult::success_json(serde_json::json!({
                    "task": task.seq(),
                    "name": task.name(),
                    "status": if is_error { "error" } else { "done" },
                    "content": result.text_content(),
                })))
            }
            None => {
                // The status changed but no tool_result was stored.
                // This can happen if the task failed before storing a result.
                match task.status() {
                    TaskStatus::Failed(e) => Ok(AgentToolResult::success_json(serde_json::json!({
                        "task": task.seq(),
                        "name": task.name(),
                        "status": "error",
                        "content": e.to_string(),
                    }))),
                    TaskStatus::Done(_) => Ok(AgentToolResult::success_json(serde_json::json!({
                        "task": task.seq(),
                        "name": task.name(),
                        "status": "done",
                        "content": "(task completed but result was not stored)",
                    }))),
                    TaskStatus::Running => Ok(AgentToolResult::success_json(serde_json::json!({
                        "task": task.seq(),
                        "name": task.name(),
                        "status": "running",
                        "content": "task is still running",
                    }))),
                }
            }
        }
    }
}

#[async_trait]
impl ToolFunction for WaitTaskTool {
    type Input = WaitTaskInput;

    /// Non-blocking — returns immediately regardless of task status.
    /// The session loop intercepts "waiting" results and exits the agent
    /// loop, spawning a background watcher that re-injects a message when
    /// the task completes or times out.
    fn timeout_seconds(&self) -> u64 {
        10
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let timeout_secs = input.timeout_seconds.unwrap_or(600);

        let tasks = self.tasks.read().await;
        let Some(task) = tasks.iter().find(|t| t.seq() == input.task) else {
            return Ok(AgentToolResult::error(format!(
                "no background task #{}, use `view_task_status` to list active tasks",
                input.task
            )));
        };

        match task.status() {
            TaskStatus::Done(_) | TaskStatus::Failed(_) => {
                // Already finished — return the result right away.
                drop(tasks);
                self.read_result(input.task).await
            }
            TaskStatus::Running => {
                // Still running — return immediately with a "waiting" sentinel.
                // The session loop will detect this and:
                //   1. Spawn a background watcher for this task
                //   2. Exit the agent processing loop (like going Idle)
                //   3. When the task completes or times out, inject a message
                //      to wake the agent.
                let name = task.name().to_string();
                Ok(AgentToolResult::success_json(serde_json::json!({
                    "task": input.task,
                    "name": name,
                    "status": "waiting",
                    "timeout_seconds": timeout_secs,
                    "content": "The agent has paused to wait for this background task. \
                                It will automatically resume when the task completes \
                                or the timeout expires."
                })))
            }
        }
    }
}
