//! The `update_plan` tool — the agent's first-class persistent task plan.
//!
//! `update_plan` is a free-form, agent-level todo list that the model itself
//! creates and maintains.
//!
//! ## How it works
//!
//! 1. The model calls `update_plan` with a full plan snapshot (a list of
//!    steps, each with a status).
//! 2. The tool atomically replaces the agent's [`AgentPlan`] via the shared
//!    [`PlanHandle`].
//! 3. The tool persists the new plan to storage (fire-and-forget).
//! 4. The tool emits an [`AgentEvent::PlanUpdate`] event so the TUI can render
//!    the updated checklist.
//! 5. The tool returns "Plan updated" to the model.
//!
use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::tools::{ToolResult, ToolResultContent};
use agentik_types::{AgentEvent, AgentPlan, PlanUpdate};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use crate::storage::AgentStorage;
use crate::tools::{ToolError, ToolFunction};

/// Shared handle to the agent's plan state, held by [`UpdatePlanTool`].
///
/// This is a lightweight cloneable handle that wraps the `ArcSwap<AgentPlan>`
/// from `AgentShared`, plus the agent id and storage for persistence, and an
/// event sender for UI notification. It decouples the tool from the full
/// `AgentShared` struct (which is `pub(crate)`).
#[derive(Clone)]
pub struct PlanHandle {
    pub plan: Arc<ArcSwap<AgentPlan>>,
    pub agent_id: uuid::Uuid,
    pub storage: Option<Arc<dyn AgentStorage>>,
    pub event_tx: Option<UnboundedSender<AgentEvent>>,
}

impl PlanHandle {
    /// Create a new handle wrapping the given plan state.
    pub fn new(
        plan: Arc<ArcSwap<AgentPlan>>,
        agent_id: uuid::Uuid,
        storage: Option<Arc<dyn AgentStorage>>,
        event_tx: Option<UnboundedSender<AgentEvent>>,
    ) -> Self {
        Self {
            plan,
            agent_id,
            storage,
            event_tx,
        }
    }
}

/// Input for [`UpdatePlanTool`].
#[tool(
    name = "update_plan",
    description = "Update the agent's task plan (TODO list). \
                   Provide a full list of plan items — each with a short step description \
                   (ideally 5-7 words) and a status — to replace the current plan. \
                   At most one step should be `in_progress` at a time. \
                   Use this for multi-step, non-trivial tasks. \
                   Do NOT use it for simple single-step queries."
)]
pub struct UpdatePlanInput {
    #[desc = "Optional explanation for why the plan changed (e.g. redirecting mid-task)."]
    pub explanation: Option<String>,
    #[desc = "The full list of plan steps, each with `step` (string) and `status` \
              (\"pending\", \"in_progress\", or \"completed\")."]
    pub plan: Vec<PlanStepInput>,
}

/// One step in the plan input.
#[tool(name = "_plan_step", description = "")]
pub struct PlanStepInput {
    #[desc = "Short step description (5-7 words)."]
    pub step: String,
    #[desc = "Step status: \"pending\", \"in_progress\", or \"completed\"."]
    pub status: String,
}

/// The `update_plan` tool.
pub struct UpdatePlanTool {
    handle: PlanHandle,
}

impl UpdatePlanTool {
    pub fn new(handle: PlanHandle) -> Self {
        Self { handle }
    }

    /// Parse the raw status string into a `StepStatus`.
    fn parse_status(raw: &str) -> Result<agentik_types::StepStatus, ToolError> {
        agentik_types::StepStatus::from_str(raw.trim()).ok_or_else(|| ToolError::ValidationFailed {
            message: format!(
                "unknown step status `{raw}`: expected `pending`, `in_progress`, or `completed`"
            ),
        })
    }
}

#[async_trait]
impl ToolFunction for UpdatePlanTool {
    type Input = UpdatePlanInput;

    async fn run(&self, input: UpdatePlanInput) -> Result<ToolResult, ToolError> {
        // Convert input steps to typed plan steps.
        let mut steps = Vec::with_capacity(input.plan.len());
        for item in input.plan {
            steps.push(agentik_types::PlanStep {
                step: item.step,
                status: Self::parse_status(&item.status)?,
            });
        }

        let update = PlanUpdate {
            explanation: input.explanation,
            plan: steps,
        };

        // Atomically replace the plan and get the new revision.
        let mut new_plan = AgentPlan::clone(&self.handle.plan.load());
        new_plan.replace(update);
        let revision = new_plan.revision;
        let update_clone = new_plan.update.clone();
        self.handle.plan.store(Arc::new(new_plan));

        // Persist (fire-and-forget — don't block the agent loop on storage).
        if let Some(storage) = &self.handle.storage {
            let snapshot = self.handle.plan.load();
            let storage = Arc::clone(storage);
            let agent_id = self.handle.agent_id;
            let plan_clone = AgentPlan::clone(&snapshot);
            crate::supervise::spawn_safe_drop("plan::save_plan", async move {
                let _ = storage.save_plan(agent_id, &plan_clone).await;
            });
        }

        // Emit event for UI.
        if let Some(tx) = &self.handle.event_tx {
            let _ = tx.send(AgentEvent::PlanUpdate {
                revision,
                update: update_clone,
            });
        }

        Ok(ToolResult {
            tool_use_id: String::new(),
            content: ToolResultContent::Text("Plan updated".to_string()),
            is_error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_types::StepStatus;

    fn test_handle() -> PlanHandle {
        PlanHandle::new(
            Arc::new(ArcSwap::new(Arc::new(AgentPlan::new()))),
            uuid::Uuid::new_v4(),
            None,
            None,
        )
    }

    #[tokio::test]
    async fn update_plan_replaces_state() {
        let handle = test_handle();
        let tool = UpdatePlanTool::new(handle.clone());

        let input = UpdatePlanInput {
            explanation: Some("starting work".into()),
            plan: vec![
                PlanStepInput {
                    step: "Step 1".into(),
                    status: "in_progress".into(),
                },
                PlanStepInput {
                    step: "Step 2".into(),
                    status: "pending".into(),
                },
            ],
        };

        let result = tool.run(input).await.unwrap();
        assert!(!result.is_error.unwrap_or(false));

        // Verify the plan state was updated.
        let plan = handle.plan.load();
        assert_eq!(plan.revision, 1);
        assert_eq!(plan.update.plan.len(), 2);
        assert_eq!(plan.update.plan[0].status, StepStatus::InProgress);
        assert_eq!(plan.update.plan[1].status, StepStatus::Pending);
    }

    #[tokio::test]
    async fn update_plan_rejects_bad_status() {
        let handle = test_handle();
        let tool = UpdatePlanTool::new(handle.clone());

        let input = UpdatePlanInput {
            explanation: None,
            plan: vec![PlanStepInput {
                step: "bad".into(),
                status: "banana".into(),
            }],
        };

        let err = tool.run(input).await.unwrap_err();
        assert!(matches!(err, ToolError::ValidationFailed { .. }));
    }

    #[tokio::test]
    async fn update_plan_increments_revision() {
        let handle = test_handle();
        let tool = UpdatePlanTool::new(handle.clone());

        // First update.
        tool.run(UpdatePlanInput {
            explanation: None,
            plan: vec![PlanStepInput {
                step: "A".into(),
                status: "pending".into(),
            }],
        })
        .await
        .unwrap();
        assert_eq!(handle.plan.load().revision, 1);

        // Second update.
        tool.run(UpdatePlanInput {
            explanation: None,
            plan: vec![PlanStepInput {
                step: "A".into(),
                status: "completed".into(),
            }],
        })
        .await
        .unwrap();
        assert_eq!(handle.plan.load().revision, 2);
    }

    #[tokio::test]
    async fn update_plan_emits_event() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let handle = PlanHandle::new(
            Arc::new(ArcSwap::new(Arc::new(AgentPlan::new()))),
            uuid::Uuid::new_v4(),
            None,
            Some(tx),
        );
        let tool = UpdatePlanTool::new(handle);

        tool.run(UpdatePlanInput {
            explanation: None,
            plan: vec![PlanStepInput {
                step: "test".into(),
                status: "in_progress".into(),
            }],
        })
        .await
        .unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            AgentEvent::PlanUpdate { revision, update } => {
                assert_eq!(revision, 1);
                assert_eq!(update.plan.len(), 1);
            }
            other => panic!("expected PlanUpdate, got {other:?}"),
        }
    }
}
