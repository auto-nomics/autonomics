//! Data-driven skill definitions.
//!
//! A [`Skill`] is a declarative, ordered workflow built on top of tool
//! primitives. Each [`SkillStep`] declares the tools that are available
//! during that phase and the todos that must be completed before the
//! workflow advances to the next step.
//!
//! Both structs are `serde`-serializable so skills can be authored in code
//! or loaded from config files (TOML/JSON).

use serde::{Deserialize, Serialize};

/// A named workflow composed of ordered [`SkillStep`]s.
///
/// When attached to an [`Agent`](crate::Agent) via
/// [`AgentBuilder::with_skill`](crate::agent_builder::AgentBuilder::with_skill),
/// the agent runs its normal request/response loop but is constrained to
/// the current step's `allowed_tools`, and the step's todo progress is
/// injected into the system prompt. The workflow advances automatically
/// once every todo in the current step is completed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique skill name (also shown in the injected prompt section).
    pub name: String,
    /// Human-readable summary of what the skill accomplishes.
    pub description: String,
    /// Ordered phases. The workflow begins at step 0 and advances linearly.
    pub steps: Vec<SkillStep>,
}

/// A single phase of a [`Skill`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillStep {
    /// Short label for the step.
    pub name: String,
    /// What the agent must accomplish in this phase. Injected into the
    /// system prompt as the step's "Goal".
    pub goal: String,
    /// Tools the agent is allowed to call during this step. The skill's own
    /// `update_todo` tool is always available in addition to these.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Sub-tasks for this step. The workflow auto-advances to the next
    /// step once every todo here is marked completed. A step with no
    /// todos is considered instantly complete and skipped on entry.
    #[serde(default)]
    pub todos: Vec<String>,
}

impl Skill {
    /// Create a new skill with the given name and description, no steps yet.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            steps: Vec::new(),
        }
    }

    /// Begin building a skill incrementally.
    pub fn builder() -> SkillBuilder {
        SkillBuilder::default()
    }

    /// Append a step and return self for chaining.
    pub fn with_step(mut self, step: SkillStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Total number of steps.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
}

/// A builder for constructing a [`Skill`] ergonomically.
#[derive(Debug, Default)]
pub struct SkillBuilder {
    name: Option<String>,
    description: Option<String>,
    steps: Vec<SkillStep>,
}

impl SkillBuilder {
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Push a fully-constructed step.
    pub fn step(mut self, step: SkillStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Push a step from its name, goal, allowed tools and todos.
    pub fn simple_step(
        self,
        name: impl Into<String>,
        goal: impl Into<String>,
        allowed_tools: Vec<String>,
        todos: Vec<String>,
    ) -> Self {
        let step = SkillStep {
            name: name.into(),
            goal: goal.into(),
            allowed_tools,
            todos,
        };
        self.step(step)
    }

    pub fn build(self) -> Skill {
        Skill {
            name: self.name.unwrap_or_default(),
            description: self.description.unwrap_or_default(),
            steps: self.steps,
        }
    }
}

/// Convenience constructor for a [`SkillStep`].
pub fn step(
    name: impl Into<String>,
    goal: impl Into<String>,
    allowed_tools: Vec<String>,
    todos: Vec<String>,
) -> SkillStep {
    SkillStep {
        name: name.into(),
        goal: goal.into(),
        allowed_tools,
        todos,
    }
}
