//! `Tool` — atomic callable that nodes may invoke.
//!
//! Tools are not stored as instances in the DB; they live in the
//! `NodeRegistry` and are referenced by `kind`. Their schemas live in the
//! `node_kinds` table.

use schemars::Schema;
use serde::{Deserialize, Serialize};

/// Description of an atomic tool / node kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tool {
    /// Stable kind identifier (e.g. `"http_request"`, `"llm_prompt"`).
    pub name: String,
    /// Human-readable label shown in the TUI palette.
    pub label: String,
    /// One-sentence description used for fuzzy search and LLM tool-selection.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: Box<Schema>,
    /// JSON Schema describing the tool's output value.
    pub output_schema: Box<Schema>,
    /// Category for grouping in the palette (e.g. `"I/O"`, `"LLM"`, `"Flow"`).
    pub category: String,
}

/// Lightweight summary returned by [`crate::registry::NodeRegistry::list_node_kinds`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeKindInfo {
    /// Same as [`Tool::name`].
    pub kind: String,
    /// Same as [`Tool::label`].
    pub label: String,
    /// Same as [`Tool::description`].
    pub description: String,
    /// Same as [`Tool::category`].
    pub category: String,
    /// Number of registered instances in the workspace, if known.
    #[serde(default)]
    pub in_use: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_serde_roundtrip() {
        let t = Tool {
            name: "http_request".into(),
            label: "HTTP Request".into(),
            description: "Make an HTTP call.".into(),
            input_schema: Box::new(schemars::schema_for!(())),
            output_schema: Box::new(schemars::schema_for!(())),
            category: "I/O".into(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: Tool = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }
}
