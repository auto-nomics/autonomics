//! The `update_todo` tool — the only tool the agent uses to drive
//! skill workflow progress. It mutates the shared [`SkillRuntime`]
//! and reports the resulting [`StepTransition`] as text.
//!
//! The state change happens directly through the shared `Arc<Mutex<...>>`,
//! and the agent's existing tool-result event emission surfaces the
//! progress text to observers.

use std::sync::Arc;

use agentik_proc::tool;
use async_trait::async_trait;
use tokio::sync::Mutex;

use agentik_sdk::types::tools::{ToolResult, ToolResultContent};

use crate::tools::{ToolError, ToolFunction};

use super::runtime::{SkillRuntime, StepTransition, TodoStatus};

/// Input for [`TodoUpdateTool`].
#[tool(
    name = "update_todo",
    description = "Update the status of a todo in the current skill step. \
                   Use `in_progress` when you begin a todo and `completed` when it is done. \
                   When every todo in the current step is completed, the workflow advances \
                   to the next step automatically."
)]
pub struct TodoUpdateInput {
    #[desc = "Index of the todo within the current step (0-based, as listed in the system prompt)."]
    pub index: usize,
    #[desc = "New status: one of \"in_progress\" or \"completed\"."]
    pub status: String,
}

/// Tool that updates the active skill's todo state.
pub struct TodoUpdateTool {
    runtime: Arc<Mutex<SkillRuntime>>,
}

impl TodoUpdateTool {
    pub fn new(runtime: Arc<Mutex<SkillRuntime>>) -> Self {
        Self { runtime }
    }

    /// Parse the user-supplied status string into a [`TodoStatus`].
    /// `completed` and `in_progress` are accepted (case-insensitive);
    /// anything else is rejected.
    fn parse_status(raw: &str) -> Result<TodoStatus, ToolError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "completed" | "complete" | "done" => Ok(TodoStatus::Completed),
            "in_progress" | "inprogress" | "started" => Ok(TodoStatus::InProgress),
            other => Err(ToolError::ValidationFailed {
                message: format!(
                    "unknown todo status `{other}`: expected `in_progress` or `completed`"
                ),
            }),
        }
    }

    fn render(transition: StepTransition, skill_name: &str) -> String {
        match transition {
            StepTransition::Updated {
                step,
                index,
                label,
                status,
            } => {
                let status_word = match status {
                    TodoStatus::Pending => "pending",
                    TodoStatus::InProgress => "in progress",
                    TodoStatus::Completed => "completed",
                };
                format!(
                    "[skill:{skill_name} step {step}] todo #{index} \"{label}\" marked {status_word}."
                )
            }
            StepTransition::Advanced { to_step, step_name } => format!(
                "[skill:{skill_name}] step complete — advancing to step {to_step}: \"{step_name}\"."
            ),
            StepTransition::SkillComplete => format!(
                "[skill:{skill_name}] final step complete. All workflow goals are satisfied — you may now produce your final answer."
            ),
        }
    }
}

#[async_trait]
impl ToolFunction for TodoUpdateTool {
    type Input = TodoUpdateInput;

    async fn run(&self, input: TodoUpdateInput) -> Result<ToolResult, ToolError> {
        let status = Self::parse_status(&input.status)?;

        // Hold the lock only for the synchronous state mutation, then drop
        // it before building the result string.
        let (transition, skill_name) = {
            let mut rt = self.runtime.lock().await;
            let name = rt.skill().name.clone();
            match rt.set_todo(input.index, status) {
                Some(t) => (t, name),
                None => {
                    return Ok(ToolResult::success(format!(
                        "[skill:{name}] no todo updated — index {} is out of bounds or the skill is already complete.",
                        input.index
                    )));
                }
            }
        };

        let text = Self::render(transition, &skill_name);
        Ok(ToolResult {
            tool_use_id: String::new(),
            content: ToolResultContent::Text(text),
            is_error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::definition::{Skill, SkillStep};
    use crate::tools::Toolset;
    use agentik_sdk::types::tools::ToolUse;
    use serde_json::json;

    fn demo_skill() -> Skill {
        Skill {
            name: "demo".to_string(),
            description: "demo".to_string(),
            steps: vec![
                SkillStep {
                    name: "s1".to_string(),
                    goal: "first".to_string(),
                    allowed_tools: vec![],
                    todos: vec!["a".to_string()],
                },
                SkillStep {
                    name: "s2".to_string(),
                    goal: "second".to_string(),
                    allowed_tools: vec![],
                    todos: vec!["b".to_string()],
                },
            ],
        }
    }

    #[tokio::test]
    async fn rejects_unknown_status() {
        let rt = Arc::new(Mutex::new(SkillRuntime::new(demo_skill())));
        let tool = TodoUpdateTool::new(Arc::clone(&rt));
        let err = tool
            .run(TodoUpdateInput {
                index: 0,
                status: "banana".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::ValidationFailed { .. }));
    }

    /// Exercises the same path the agent uses: register the todo tool into a
    /// Toolset, dispatch via `execute`, and confirm the shared runtime advances.
    #[tokio::test]
    async fn end_to_end_via_toolset_advances_step() {
        use crate::tools::ToolRegistry;

        let (runtime, todo_reg) = crate::skill::instantiate(demo_skill());

        let mut registry = ToolRegistry::new();
        registry.register(todo_reg).unwrap();
        let toolset = Toolset::from_registry(
            std::sync::Arc::new(registry),
            Some(
                tokio::sync::mpsc::unbounded_channel::<agentik_sdk::types::AgentEvent>().0,
            ),
        );

        // allowed_tools whitelist must always permit update_todo.
        let allowed = runtime.lock().await.allowed_tools_for_current_step();
        assert!(allowed.contains(&"update_todo".to_string()));

        // Complete the only todo of step 0 → auto-advance to step 1.
        let tc = ToolUse {
            id: "tu_1".to_string(),
            name: "update_todo".to_string(),
            input: json!({ "index": 0, "status": "completed" }),
        };
        let results = toolset
            .execute(std::slice::from_ref(&tc), None)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error.unwrap_or(false));

        // Runtime should now be on step 1 (index 1).
        assert_eq!(runtime.lock().await.current_step_index(), 1);

        // Complete step 1 → skill complete.
        let tc2 = ToolUse {
            id: "tu_2".to_string(),
            name: "update_todo".to_string(),
            input: json!({ "index": 0, "status": "completed" }),
        };
        let _ = toolset
            .execute(std::slice::from_ref(&tc2), None)
            .await
            .unwrap();
        assert!(runtime.lock().await.is_complete());
    }
}
