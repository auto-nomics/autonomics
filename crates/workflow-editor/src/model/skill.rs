//! `Skill` — a reusable DAG subgraph with SOP + tool whitelist.
//!
//! A skill carries:
//! - `manifest` — the embedded DAG (nodes + edges) that runs as a unit,
//! - `surface_inputs` / `surface_outputs` — ports exposed to the outer
//!   graph; the `SubgraphNode` executor wires these to the inner DAG,
//! - `sop_text` — prompt prepended to the system prompt for the subgraph's
//!   execution context (after the workflow SOP, before node-level instructions),
//! - `tool_refs` — allow-list of tool names; calls outside this list are
//!   rejected at runtime.

use crate::model::manifest::WorkflowManifest;
use crate::model::port::PortSpec;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A reusable, versioned DAG subgraph.
///
/// Skills are first-class entities stored in the `skills` / `skill_versions`
/// tables. Versions are monotonically increasing per `name`; saving the same
/// `name` twice produces version 2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skill {
    /// Stable id for this skill version.
    pub id: Uuid,
    /// Logical name (unique across versions).
    pub name: String,
    /// Monotonically increasing version for `name` (starts at 1).
    pub version: u32,
    /// One-line description shown in the skill picker.
    pub description: String,
    /// SOP prompt prepended when the skill executes.
    pub sop_text: String,
    /// Allowed tool names. Empty = inherits parent context's whitelist.
    pub tool_refs: Vec<String>,
    /// The embedded DAG.
    pub manifest: WorkflowManifest,
    /// Ports exposed to the outer graph on the *input* side.
    pub surface_inputs: Vec<PortSpec>,
    /// Ports exposed to the outer graph on the *output* side.
    pub surface_outputs: Vec<PortSpec>,
}

impl Skill {
    /// Construct a skill with a fresh id, version 1, and the given name.
    pub fn new(name: impl Into<String>, manifest: WorkflowManifest) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            version: 1,
            description: String::new(),
            sop_text: String::new(),
            tool_refs: Vec::new(),
            manifest,
            surface_inputs: Vec::new(),
            surface_outputs: Vec::new(),
        }
    }

    /// Builder-style setter for [`Skill::description`].
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Builder-style setter for [`Skill::sop_text`].
    pub fn with_sop(mut self, sop: impl Into<String>) -> Self {
        self.sop_text = sop.into();
        self
    }

    /// Builder-style setter for [`Skill::tool_refs`].
    pub fn with_tool_refs(mut self, refs: Vec<String>) -> Self {
        self.tool_refs = refs;
        self
    }

    /// Builder-style setter for [`Skill::surface_inputs`].
    pub fn with_surface_inputs(mut self, ports: Vec<PortSpec>) -> Self {
        self.surface_inputs = ports;
        self
    }

    /// Builder-style setter for [`Skill::surface_outputs`].
    pub fn with_surface_outputs(mut self, ports: Vec<PortSpec>) -> Self {
        self.surface_outputs = ports;
        self
    }

    /// Validate the skill's structural invariants.
    pub fn validate(&self) -> Result<(), crate::error::ModelError> {
        self.manifest.validate()?;
        // All surface inputs must appear as inputs of at least one inner node.
        for s in &self.surface_inputs {
            if !self.manifest.nodes.iter().any(|n| {
                n.inputs.iter().any(|p| p.id == s.id) || n.outputs.iter().any(|p| p.id == s.id)
            }) {
                return Err(crate::error::ModelError::InvalidSkill(format!(
                    "surface input '{}' does not match any inner port",
                    s.id
                )));
            }
        }
        for s in &self.surface_outputs {
            if !self
                .manifest
                .nodes
                .iter()
                .any(|n| n.outputs.iter().any(|p| p.id == s.id))
            {
                return Err(crate::error::ModelError::InvalidSkill(format!(
                    "surface output '{}' does not match any inner output port",
                    s.id
                )));
            }
        }
        Ok(())
    }
}

/// Lightweight summary returned by `SkillRegistry::list_skills`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct SkillInfo {
    /// Skill id.
    pub id: Uuid,
    /// Logical name.
    pub name: String,
    /// Latest version.
    pub version: u32,
    /// Description.
    pub description: String,
    /// Number of inner nodes.
    pub node_count: u32,
    /// Last update timestamp.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::manifest::NodeEntry;

    #[test]
    fn skill_builder_chains() {
        let s = Skill::new("summarize", WorkflowManifest::new("inner"))
            .with_description("Summarize a document")
            .with_sop("You are a careful summarizer.")
            .with_tool_refs(vec!["http_request".into()]);
        assert_eq!(s.name, "summarize");
        assert_eq!(s.version, 1);
        assert_eq!(s.sop_text, "You are a careful summarizer.");
    }

    #[test]
    fn skill_serde_with_nested_manifest() {
        let mut inner = WorkflowManifest::new("inner");
        inner.nodes.push(NodeEntry {
            id: Uuid::new_v4(),
            kind: "echo".into(),
            label: "e".into(),
            position: (0, 0),
            inputs: vec![PortSpec::new("in", "in")],
            outputs: vec![PortSpec::new("out", "out")],
            params: serde_json::json!({}),
        });
        let s = Skill::new("sk", inner);
        let json = serde_json::to_string(&s).unwrap();
        let back: Skill = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn skill_validate_detects_orphan_surface() {
        let mut inner = WorkflowManifest::new("inner");
        inner.nodes.push(NodeEntry {
            id: Uuid::new_v4(),
            kind: "echo".into(),
            label: "e".into(),
            position: (0, 0),
            inputs: vec![PortSpec::new("in", "in")],
            outputs: vec![PortSpec::new("out", "out")],
            params: serde_json::json!({}),
        });
        let s = Skill::new("sk", inner)
            .with_surface_inputs(vec![PortSpec::new("GHOST", "missing")]);
        assert!(s.validate().is_err());
    }
}