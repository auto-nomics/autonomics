//! Port types — input/output port descriptors on nodes and skills.
//!
//! Ports are referenced by `EdgeEntry` (`source_handle`, `target_handle`)
//! and by `WorkflowManifest` node entries. A port may optionally carry a
//! `schemars::Schema` describing the value flowing through it.

use schemars::Schema;
use serde::{Deserialize, Serialize};

/// Description of one port (input or output) on a node or skill surface.
///
/// Ports are addressed by `id` within the owning node/skill — an edge
/// references `source_handle` / `target_handle` which are these ids.
///
/// `schema` is wrapped in `Box` because `schemars::Schema` is heavyweight
/// and is read-only after construction. `Box<Schema>` has blanket serde impls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PortSpec {
    /// Stable id within the owning node / skill (e.g. `"trigger"`, `"body"`).
    pub id: String,
    /// Human-readable label for the UI.
    pub label: String,
    /// Optional JSON Schema describing values flowing through this port.
    /// `None` means "any".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Box<Schema>>,
}

impl PortSpec {
    /// Construct a port with no schema constraint.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            schema: None,
        }
    }

    /// Attach a JSON Schema to this port.
    pub fn with_schema(mut self, schema: Schema) -> Self {
        self.schema = Some(Box::new(schema));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_spec_new_minimal() {
        let p = PortSpec::new("in", "input");
        assert_eq!(p.id, "in");
        assert_eq!(p.label, "input");
        assert!(p.schema.is_none());
    }

    #[test]
    fn port_spec_serde_roundtrip() {
        let p = PortSpec::new("body", "response body");
        let json = serde_json::to_string(&p).unwrap();
        let back: PortSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
        // schema omitted → serde_json should not contain the field
        assert!(!json.contains("schema"));
    }
}
