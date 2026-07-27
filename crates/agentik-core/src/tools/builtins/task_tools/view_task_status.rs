use std::sync::Arc;
use tokio::sync::RwLock;

use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::tools::task_runtime::{TaskEntry, TaskStatus};
use crate::tools::{ToolError, ToolFunction};

#[tool(
    name = "view_task_status",
    description = "View the status of all background tasks, including currently \
                  running ones with their accumulated output. Use this to check \
                  progress of long-running tools."
)]
pub struct ViewTaskStatusInput {
    #[desc = "ID of target background task"]
    task_id: String,
}

pub struct TaskStatusViewerTool {
    tasks: Arc<RwLock<Vec<TaskEntry>>>,
}

impl TaskStatusViewerTool {
    pub fn new(tasks: Arc<RwLock<Vec<TaskEntry>>>) -> Self {
        Self { tasks }
    }
}

impl From<TaskStatus> for &'static str {
    fn from(status: TaskStatus) -> &'static str {
        match status {
            TaskStatus::Running => "running",
            TaskStatus::Done(_) => "done",
            TaskStatus::Failed(_) => "failed",
        }
    }
}

#[async_trait]
impl ToolFunction for TaskStatusViewerTool {
    type Input = ViewTaskStatusInput;

    fn sync_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let tasks = self.tasks.read().await;
        let target_task = tasks.iter().find(|t| t.id() == input.task_id);

        if let Some(task) = target_task {
            let status: &str = task.status().into();
            // `output` is the tool's accumulated live/intermediate output
            // (e.g. per-node progress from a backgrounded `run_dag`). Surface
            // it only when non-empty so quick tasks stay noise-free.
            let output = task.output();
            let mut payload = serde_json::json!({
                "task_id": task.id(),
                "name": task.name(),
                "status": status,
            });
            if !output.is_empty() {
                payload["output"] = serde_json::Value::String(output);
            }
            let result = AgentToolResult::success_json(payload);

            Ok(result)
        } else {
            Ok(AgentToolResult::error(format!(
                "no background task with id {:?}",
                input.task_id
            )))
        }
    }
}
