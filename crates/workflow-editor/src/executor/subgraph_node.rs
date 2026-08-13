//! `SubgraphNode` — exposes a `Skill` as a single node in the outer graph.
//!
//! When this node executes, it:
//! 1. Pushes the skill's `sop_text` + `tool_refs` onto the SOP stack (RAII pop).
//! 2. Validates the skill (cheap; prevents wasted recursion on bad skills).
//! 3. Maps outer `inputs[surface_input_id]` → inner `seed_outputs[(node_id, port_id)]`
//!    by walking the skill's `manifest` and finding each surface-input's matching
//!    inner port.
//! 4. Recursively runs the embedded manifest via
//!    [`Scheduler::run_with_seed_outputs`].
//! 5. Maps inner surface outputs → outer node outputs.

use crate::error::{ExecutorError, Result};
use crate::executor::ports::{NodeCtx, NodeExecutor, NodeReporter, PortInputs, PortOutputs};
use crate::executor::scheduler::Scheduler;
use crate::model::Skill;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// A node that wraps a [`Skill`].
pub struct SubgraphNode {
    skill: Arc<Skill>,
    scheduler: Arc<Scheduler>,
}

impl SubgraphNode {
    /// Build a subgraph node from a shared skill and a scheduler to recurse into.
    pub fn new(skill: Arc<Skill>, scheduler: Arc<Scheduler>) -> Self {
        Self { skill, scheduler }
    }

    /// Borrow the underlying skill.
    pub fn skill(&self) -> &Skill {
        &self.skill
    }

    /// Map outer inputs (keyed by surface_input port id) into the inner
    /// manifest's pre-seeded outputs (`(inner_node_id, inner_port_id) -> value`).
    fn seed_outputs_from_inputs(
        &self,
        inputs: &PortInputs,
    ) -> HashMap<(Uuid, String), serde_json::Value> {
        let mut seed = HashMap::new();
        for surface in &self.skill.surface_inputs {
            if let Some(v) = inputs.get(&surface.id) {
                // Find the inner node that has a port matching this surface id.
                if let Some(inner_node) = self.skill.manifest.nodes.iter().find(|n| {
                    n.inputs.iter().any(|p| p.id == surface.id)
                        || n.outputs.iter().any(|p| p.id == surface.id)
                }) {
                    seed.insert((inner_node.id, surface.id.clone()), v.clone());
                }
            }
        }
        seed
    }

    /// Collect the inner surface outputs into outer node outputs.
    fn project_outputs(&self, inner: &crate::executor::scheduler::WorkflowResult) -> PortOutputs {
        let mut outs = PortOutputs::new();
        for surface in &self.skill.surface_outputs {
            if let Some(inner_node) = self
                .skill
                .manifest
                .nodes
                .iter()
                .find(|n| n.outputs.iter().any(|p| p.id == surface.id))
            {
                if let Some(v) = inner.outputs.get(&(inner_node.id, surface.id.clone())) {
                    outs.insert(surface.id.clone(), v.clone());
                }
            }
        }
        // Always emit a synthetic `out` port with the inner run summary so
        // downstream nodes that wire to a single `out` port have something.
        outs.entry("out".into()).or_insert_with(|| {
            serde_json::json!({
                "skill": self.skill.name,
                "version": self.skill.version,
                "inner_nodes": self.skill.manifest.nodes.len(),
            })
        });
        outs
    }
}

#[async_trait]
impl NodeExecutor for SubgraphNode {
    fn kind(&self) -> &'static str {
        "skill"
    }

    async fn execute(
        &self,
        ctx: &mut NodeCtx,
        inputs: &PortInputs,
        _reporter: &NodeReporter,
    ) -> Result<PortOutputs> {
        // 1. Push SOP frame for the skill (RAII pop on drop).
        let tools = if self.skill.tool_refs.is_empty() {
            None
        } else {
            Some(self.skill.tool_refs.clone())
        };
        let _guard = ctx.sop.push(self.skill.sop_text.clone(), tools);

        // 2. Validate the skill before descending (cheap, prevents wasted recursion).
        self.skill.validate().map_err(|e| {
            ExecutorError::Internal(format!(
                "skill '{}' failed validation: {}",
                self.skill.name, e
            ))
        })?;

        // 3. Map outer inputs → inner seed outputs.
        let seed = self.seed_outputs_from_inputs(inputs);

        // 4. Recurse into the skill's manifest.
        // The parent SOP context already has the skill's frame pushed
        // (by `_guard` above), so we pass `ctx.sop` as the parent — the
        // recursive scheduler appends the manifest-level SOP on top.
        let inner_result = self
            .scheduler
            .run_with_seed_outputs(
                &self.skill.manifest,
                seed,
                ctx.sop.clone(),
                ctx.cancel.clone(),
            )
            .await?;

        // 5. Project inner surface outputs → outer outputs.
        Ok(self.project_outputs(&inner_result))
    }
}
