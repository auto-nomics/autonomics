//! Runtime state for an active [`Skill`](super::Skill).
//!
//! [`SkillRuntime`] tracks which step the workflow is on and the
//! completion status of the current step's todos. It exposes the
//! data the agent loop needs (allowed tools, prompt section) and
//! applies todo updates with automatic step advancement.

use super::definition::{Skill, SkillStep};

/// Public name of the skill tools that are always allowed
/// regardless of the current step's `allowed_tools`.
pub const UPDATE_TODO_TOOL: &str = "update_todo";

/// Completion status of a single todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    /// Glyph used in the injected prompt section.
    fn marker(self) -> &'static str {
        match self {
            TodoStatus::Pending => "[ ]",
            TodoStatus::InProgress => "[~]",
            TodoStatus::Completed => "[x]",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TodoItem {
    pub label: String,
    pub status: TodoStatus,
}

/// Outcome of a [`SkillRuntime::set_todo`] call, used by the
/// `update_todo` tool to produce a meaningful result string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepTransition {
    /// A todo status was updated but the step did not change.
    Updated {
        step: usize,
        index: usize,
        label: String,
        status: TodoStatus,
    },
    /// All todos of the current step completed and the workflow
    /// advanced to `to_step`.
    Advanced { to_step: usize, step_name: String },
    /// The final step's todos are all complete.
    SkillComplete,
}

pub struct SkillRuntime {
    skill: Skill,
    /// Index of the currently active step.
    current_step: usize,
    /// Per-step todo state: `todos[step_index]` mirrors
    /// `skill.steps[step_index].todos` with live status.
    todos: Vec<Vec<TodoItem>>,
}

impl SkillRuntime {
    /// Create a new runtime, initializing every todo to [`TodoStatus::Pending`]
    /// and skipping any leading steps that have no todos.
    pub fn new(skill: Skill) -> Self {
        let todos = skill
            .steps
            .iter()
            .map(|s| {
                s.todos
                    .iter()
                    .map(|label| TodoItem {
                        label: label.clone(),
                        status: TodoStatus::Pending,
                    })
                    .collect()
            })
            .collect();

        let mut rt = Self {
            skill,
            current_step: 0,
            todos,
        };
        rt.enter_step();
        rt
    }

    /// Skip past steps that have no todos (instantly complete) until
    /// landing on a step with todos or the final step. Called on
    /// construction and after each advancement.
    fn enter_step(&mut self) {
        while self.current_step + 1 < self.skill.steps.len()
            && self.todos[self.current_step].is_empty()
        {
            self.current_step += 1;
        }
    }

    pub fn skill(&self) -> &Skill {
        &self.skill
    }

    pub fn current_step_index(&self) -> usize {
        self.current_step
    }

    pub fn current_step(&self) -> Option<&SkillStep> {
        self.skill.steps.get(self.current_step)
    }

    /// Is the entire skill finished (last step's todos all completed)?
    pub fn is_complete(&self) -> bool {
        match self.todos.last() {
            Some(todos) => {
                !todos.is_empty() && todos.iter().all(|t| t.status == TodoStatus::Completed)
            }
            None => true,
        }
    }

    /// Tools the agent may call right now: the current step's declared
    /// tools, plus the always-available todo tool.
    pub fn allowed_tools_for_current_step(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .current_step()
            .map(|s| s.allowed_tools.clone())
            .unwrap_or_default();
        for always in [UPDATE_TODO_TOOL] {
            if !names.iter().any(|n| n == always) {
                names.push(always.to_string());
            }
        }
        names
    }

    /// Read-only snapshot of the current step's todos.
    pub fn current_todos(&self) -> &[TodoItem] {
        self.todos
            .get(self.current_step)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Update the status of todo `index` within the current step.
    ///
    /// If this completes the last pending todo of the step, the workflow
    /// advances automatically (skipping empty steps). Returns a
    /// [`StepTransition`] describing what happened. Returns `None` if
    /// `index` is out of bounds or the skill is already complete.
    pub fn set_todo(&mut self, index: usize, status: TodoStatus) -> Option<StepTransition> {
        if self.is_complete() {
            return None;
        }
        let todos = self.todos.get_mut(self.current_step)?;
        let item = todos.get_mut(index)?;
        item.status = status;
        let label = item.label.clone();

        // Auto-advance when every todo in the current step is completed.
        let all_done = !todos.is_empty() && todos.iter().all(|t| t.status == TodoStatus::Completed);
        if all_done {
            if self.current_step + 1 < self.skill.steps.len() {
                self.current_step += 1;
                self.enter_step();
                let step_name = self
                    .current_step()
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                return Some(StepTransition::Advanced {
                    to_step: self.current_step,
                    step_name,
                });
            } else {
                return Some(StepTransition::SkillComplete);
            }
        }

        Some(StepTransition::Updated {
            step: self.current_step,
            index,
            label,
            status,
        })
    }

    /// Render the skill's progress as a section to inject into the system prompt.
    pub fn current_prompt_section(&self) -> String {
        let mut out = String::new();
        out.push_str("## Active Skill: ");
        out.push_str(&self.skill.name);
        out.push('\n');
        if !self.skill.description.is_empty() {
            out.push_str(&self.skill.description);
            out.push('\n');
        }

        let total = self.skill.steps.len();
        let step_no = self.current_step + 1;
        if let Some(step) = self.current_step() {
            out.push_str(&format!(
                "Current step {}/{} — {}\n",
                step_no, total, step.name
            ));
            if !step.goal.is_empty() {
                out.push_str(&format!("Goal: {}\n", step.goal));
            }
        }

        if self.is_complete() {
            out.push_str("All steps complete. Produce your final answer.\n");
            return out;
        }

        let todos = self.current_todos();
        if !todos.is_empty() {
            out.push_str("Todos:\n");
            for (i, t) in todos.iter().enumerate() {
                out.push_str(&format!("  {} {}. {}\n", t.status.marker(), i, t.label));
            }
            out.push_str(
                "Call `update_todo(index, status)` to track progress. Mark a todo \
                 `in_progress` when you start it and `completed` when done. When all \
                 todos in the current step are complete, the workflow advances to the \
                 next step automatically.\n",
            );
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::definition::{Skill, SkillStep};
    use super::*;

    fn two_step_skill() -> Skill {
        Skill {
            name: "demo".to_string(),
            description: "A demo skill".to_string(),
            steps: vec![
                SkillStep {
                    name: "gather".to_string(),
                    goal: "Gather inputs".to_string(),
                    allowed_tools: vec!["read".to_string()],
                    todos: vec!["read a".to_string(), "read b".to_string()],
                },
                SkillStep {
                    name: "write".to_string(),
                    goal: "Write output".to_string(),
                    allowed_tools: vec!["write".to_string()],
                    todos: vec!["write report".to_string()],
                },
            ],
        }
    }

    #[test]
    fn new_initializes_pending_and_starts_at_step_zero() {
        let rt = SkillRuntime::new(two_step_skill());
        assert_eq!(rt.current_step_index(), 0);
        assert_eq!(rt.current_todos().len(), 2);
        assert!(
            rt.current_todos()
                .iter()
                .all(|t| t.status == TodoStatus::Pending)
        );
        assert!(!rt.is_complete());
    }

    #[test]
    fn allowed_tools_always_include_todo() {
        let rt = SkillRuntime::new(two_step_skill());
        let allowed = rt.allowed_tools_for_current_step();
        assert!(allowed.contains(&"read".to_string()));
        assert!(allowed.contains(&UPDATE_TODO_TOOL.to_string()));
        // write tool not yet allowed (it belongs to step 2)
        assert!(!allowed.contains(&"write".to_string()));
    }

    #[test]
    fn marking_partial_todos_does_not_advance() {
        let mut rt = SkillRuntime::new(two_step_skill());
        let t = rt.set_todo(0, TodoStatus::Completed).unwrap();
        assert!(matches!(t, StepTransition::Updated { index: 0, .. }));
        assert_eq!(rt.current_step_index(), 0);

        let t = rt.set_todo(1, TodoStatus::InProgress).unwrap();
        assert!(matches!(
            t,
            StepTransition::Updated {
                status: TodoStatus::InProgress,
                ..
            }
        ));
        assert_eq!(rt.current_step_index(), 0);
    }

    #[test]
    fn completing_all_todos_advances_step() {
        let mut rt = SkillRuntime::new(two_step_skill());
        rt.set_todo(0, TodoStatus::Completed).unwrap();
        let t = rt.set_todo(1, TodoStatus::Completed).unwrap();
        assert!(matches!(t, StepTransition::Advanced { to_step: 1, .. }));
        assert_eq!(rt.current_step_index(), 1);
        // New step's allowed tool now visible
        assert!(
            rt.allowed_tools_for_current_step()
                .contains(&"write".to_string())
        );
    }

    #[test]
    fn completing_last_step_marks_skill_complete() {
        let mut rt = SkillRuntime::new(two_step_skill());
        rt.set_todo(0, TodoStatus::Completed).unwrap();
        rt.set_todo(1, TodoStatus::Completed).unwrap();
        assert_eq!(rt.current_step_index(), 1);

        let t = rt.set_todo(0, TodoStatus::Completed).unwrap();
        assert_eq!(t, StepTransition::SkillComplete);
        assert!(rt.is_complete());

        // Further updates are no-ops.
        assert!(rt.set_todo(0, TodoStatus::Completed).is_none());
    }

    #[test]
    fn empty_todo_steps_are_skipped_on_entry() {
        let skill = Skill {
            name: "skip".to_string(),
            description: String::new(),
            steps: vec![
                SkillStep {
                    name: "noop".to_string(),
                    goal: "no todos".to_string(),
                    allowed_tools: vec![],
                    todos: vec![],
                },
                SkillStep {
                    name: "real".to_string(),
                    goal: "do work".to_string(),
                    allowed_tools: vec!["work".to_string()],
                    todos: vec!["task".to_string()],
                },
            ],
        };
        let rt = SkillRuntime::new(skill);
        assert_eq!(rt.current_step_index(), 1, "empty leading step skipped");
        assert_eq!(rt.current_step().unwrap().name, "real");
    }

    #[test]
    fn out_of_bounds_index_returns_none() {
        let mut rt = SkillRuntime::new(two_step_skill());
        assert!(rt.set_todo(99, TodoStatus::Completed).is_none());
    }

    #[test]
    fn prompt_section_contains_step_goal_and_todos() {
        let rt = SkillRuntime::new(two_step_skill());
        let section = rt.current_prompt_section();
        assert!(section.contains("## Active Skill: demo"));
        assert!(section.contains("Goal: Gather inputs"));
        assert!(section.contains("read a"));
        assert!(section.contains("update_todo"));
    }

    #[test]
    fn prompt_section_says_complete_when_done() {
        let mut rt = SkillRuntime::new(two_step_skill());
        rt.set_todo(0, TodoStatus::Completed).unwrap();
        rt.set_todo(1, TodoStatus::Completed).unwrap();
        rt.set_todo(0, TodoStatus::Completed).unwrap();
        let section = rt.current_prompt_section();
        assert!(section.contains("All steps complete"));
    }
}
