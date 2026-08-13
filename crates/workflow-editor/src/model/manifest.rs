//! `WorkflowManifest` — the persisted representation of a workflow DAG.
//!
//! A manifest is a `serde_json::Value` that round-trips through SQLite and
//! is content-addressed by `blake3(serde_json::to_vec(manifest))` so identical
//! manifests deduplicate naturally across snapshots.
//!
//! The schema version starts at 1; bump when adding required fields.

use crate::model::port::PortSpec;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current schema version. Bump on breaking changes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// A complete workflow DAG.
///
/// This is the *persisted* shape: what gets serialized into SQLite, what
/// gets exported to JSON, what gets imported. The runtime executor uses a
/// richer in-memory form (`executor::Workflow`) but always seeds from this.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowManifest {
    /// Schema version — see [`CURRENT_SCHEMA_VERSION`].
    pub version: u32,
    /// Stable id for this workflow.
    pub id: Uuid,
    /// Human-readable name.
    pub name: String,
    /// All nodes in the DAG.
    pub nodes: Vec<NodeEntry>,
    /// All edges in the DAG.
    pub edges: Vec<EdgeEntry>,
    /// Editor viewport (pan + grid step). Persisted so the TUI restores its
    /// view on reload.
    pub viewport: Viewport,
    /// Optional workflow-level SOP prepended to the system prompt at run
    /// time. See crate docs for the injection order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sop: Option<String>,
}

/// A single node in the DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeEntry {
    /// Stable id (UUIDv4).
    pub id: Uuid,
    /// Registered node kind (e.g. `"http_request"`, `"skill"`).
    pub kind: String,
    /// Display label for the TUI.
    pub label: String,
    /// Canvas position in character cells, `(x, y)`.
    pub position: (i32, i32),
    /// Input ports.
    pub inputs: Vec<PortSpec>,
    /// Output ports.
    pub outputs: Vec<PortSpec>,
    /// JSON value of node-specific parameters.
    /// Validated against `node_kinds.spec_schema` on write.
    pub params: serde_json::Value,
}

/// A directed edge between two nodes' ports.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeEntry {
    /// Stable id.
    pub id: Uuid,
    /// Source node id.
    pub source: Uuid,
    /// Source port id (`source.outputs[*].id`).
    pub source_handle: String,
    /// Target node id.
    pub target: Uuid,
    /// Target port id (`target.inputs[*].id`).
    pub target_handle: String,
}

/// Editor viewport state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Viewport {
    /// Pan offset in character cells.
    pub pan: (i32, i32),
    /// Grid step (1, 2, or 4). Higher = more zoomed out.
    pub grid_step: u8,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            pan: (0, 0),
            grid_step: 1,
        }
    }
}

impl WorkflowManifest {
    /// Construct an empty manifest with a fresh id and the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            name: name.into(),
            nodes: Vec::new(),
            edges: Vec::new(),
            viewport: Viewport::default(),
            sop: None,
        }
    }

    /// Content hash of this manifest (blake3 over the canonical JSON form).
    /// Used by the snapshot store to dedupe identical history entries.
    pub fn content_hash(&self) -> blake3::Hash {
        let bytes = serde_json::to_vec(self).expect("manifest serialization");
        blake3::hash(&bytes)
    }

    /// Validate the manifest's structural invariants (schema version, no
    /// self-loops via edge ids, all `source_handle` / `target_handle`
    /// reference existing ports). Does **not** run topological sort — that
    /// lives in [`crate::executor::scheduler`].
    pub fn validate(&self) -> Result<(), crate::error::ModelError> {
        if self.version > CURRENT_SCHEMA_VERSION {
            return Err(crate::error::ModelError::UnsupportedSchemaVersion {
                found: self.version,
                max: CURRENT_SCHEMA_VERSION,
            });
        }
        for n in &self.nodes {
            for p in &n.inputs {
                if p.id.is_empty() {
                    return Err(crate::error::ModelError::InvalidManifest(format!(
                        "node {} has empty input port id",
                        n.id
                    )));
                }
            }
            for p in &n.outputs {
                if p.id.is_empty() {
                    return Err(crate::error::ModelError::InvalidManifest(format!(
                        "node {} has empty output port id",
                        n.id
                    )));
                }
            }
        }
        for e in &self.edges {
            if e.source == e.target {
                return Err(crate::error::ModelError::InvalidManifest(format!(
                    "edge {} is a self-loop on node {}",
                    e.id, e.source
                )));
            }
            let src = self.nodes.iter().find(|n| n.id == e.source);
            let tgt = self.nodes.iter().find(|n| n.id == e.target);
            let (Some(src), Some(tgt)) = (src, tgt) else {
                return Err(crate::error::ModelError::InvalidManifest(format!(
                    "edge {} references missing node",
                    e.id
                )));
            };
            if !src.outputs.iter().any(|p| p.id == e.source_handle) {
                return Err(crate::error::ModelError::InvalidManifest(format!(
                    "edge {}: source node {} has no output port '{}'",
                    e.id, src.id, e.source_handle
                )));
            }
            if !tgt.inputs.iter().any(|p| p.id == e.target_handle) {
                return Err(crate::error::ModelError::InvalidManifest(format!(
                    "edge {}: target node {} has no input port '{}'",
                    e.id, tgt.id, e.target_handle
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manifest_defaults() {
        let m = WorkflowManifest::new("hello");
        assert_eq!(m.version, CURRENT_SCHEMA_VERSION);
        assert_eq!(m.name, "hello");
        assert!(m.nodes.is_empty());
        assert!(m.edges.is_empty());
        assert_eq!(m.viewport, Viewport::default());
        assert!(m.sop.is_none());
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let mut m = WorkflowManifest::new("rt");
        let n = NodeEntry {
            id: Uuid::new_v4(),
            kind: "echo".into(),
            label: "Echo".into(),
            position: (10, 5),
            inputs: vec![PortSpec::new("in", "in")],
            outputs: vec![PortSpec::new("out", "out")],
            params: serde_json::json!({"x": 1}),
        };
        let n_id = n.id;
        m.nodes.push(n);
        m.edges.push(EdgeEntry {
            id: Uuid::new_v4(),
            source: n_id,
            source_handle: "out".into(),
            target: n_id,
            target_handle: "in".into(),
        });

        let json = serde_json::to_string(&m).unwrap();
        let back: WorkflowManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
        assert_eq!(m.content_hash(), back.content_hash());
    }

    #[test]
    fn validate_rejects_self_loop() {
        let mut m = WorkflowManifest::new("loop");
        let n_id = Uuid::new_v4();
        m.nodes.push(NodeEntry {
            id: n_id,
            kind: "echo".into(),
            label: "n".into(),
            position: (0, 0),
            inputs: vec![PortSpec::new("in", "in")],
            outputs: vec![PortSpec::new("out", "out")],
            params: serde_json::json!({}),
        });
        m.edges.push(EdgeEntry {
            id: Uuid::new_v4(),
            source: n_id,
            source_handle: "out".into(),
            target: n_id,
            target_handle: "in".into(),
        });
        assert!(m.validate().is_err());
    }

    #[test]
    fn validate_rejects_missing_port() {
        let mut m = WorkflowManifest::new("bad");
        let n_id = Uuid::new_v4();
        m.nodes.push(NodeEntry {
            id: n_id,
            kind: "echo".into(),
            label: "n".into(),
            position: (0, 0),
            inputs: vec![PortSpec::new("in", "in")],
            outputs: vec![PortSpec::new("out", "out")],
            params: serde_json::json!({}),
        });
        m.edges.push(EdgeEntry {
            id: Uuid::new_v4(),
            source: n_id,
            source_handle: "DOES_NOT_EXIST".into(),
            target: n_id,
            target_handle: "in".into(),
        });
        assert!(m.validate().is_err());
    }

    #[test]
    fn validate_rejects_unknown_schema_version() {
        let mut m = WorkflowManifest::new("future");
        m.version = 999;
        match m.validate() {
            Err(crate::error::ModelError::UnsupportedSchemaVersion { found, max }) => {
                assert_eq!(found, 999);
                assert_eq!(max, CURRENT_SCHEMA_VERSION);
            }
            other => panic!("expected UnsupportedSchemaVersion, got {:?}", other),
        }
    }
}
