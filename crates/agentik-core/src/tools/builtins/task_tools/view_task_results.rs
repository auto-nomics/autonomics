use std::sync::Arc;
use tokio::sync::RwLock;

use async_trait::async_trait;

use agentik_sdk::types::ToolResult as AgentToolResult;

use crate::tools::task_runtime::TaskStore;
use crate::tools::{ToolError, ToolFunction};
use agentik_proc::tool;

#[tool(
    name = "view_task_results",
    description = "View the stored result of a single background task by its task number. \
                  Returns the real tool result recorded inside the task entry. \
                  If the task has not finished yet, reports it as still running."
)]
pub struct ViewTaskResultsInput {
    #[desc = "Task number (#N) of the target background task, as shown when it was spawned"]
    task: u64,
}

pub struct TaskResultViewerTool {
    tasks: Arc<RwLock<TaskStore>>,
}

impl TaskResultViewerTool {
    pub fn new(tasks: Arc<RwLock<TaskStore>>) -> Self {
        Self { tasks }
    }
}

#[async_trait]
impl ToolFunction for TaskResultViewerTool {
    type Input = ViewTaskResultsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let tasks = self.tasks.read().await;
        let Some(task) = tasks.iter().find(|t| t.seq() == input.task) else {
            return Ok(AgentToolResult::error(format!(
                "no background task #{}, use `view_task_status` to list active tasks",
                input.task
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
            // Still running (or failed before storing a result) — leave the
            // entry unread so it can be queried again later.
            None => Ok(AgentToolResult::success_json(serde_json::json!({
                "task": task.seq(),
                "name": task.name(),
                "status": "running",
                "content": "task has not produced a result yet",
            }))),
        }
    }
}
