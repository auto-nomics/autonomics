//! `NodeFactory` trait — build a node from `(kind, params)`.

use crate::error::Result;
use crate::executor::NodeExecutor;
use crate::model::{PortSpec, Tool};
use async_trait::async_trait;
use schemars::Schema;

/// Async factory that produces an executable node from a `kind` and JSON
/// `params` blob.
///
/// Implemented by every node kind the user wants to use. The registry in
/// [`super::node_registry`] is a `HashMap<&'static str, Box<dyn NodeFactory>>`.
#[async_trait]
pub trait NodeFactory: Send + Sync {
    /// Stable kind identifier (e.g. `"http_request"`).
    fn kind(&self) -> &'static str;

    /// Human-readable label for the TUI palette.
    fn label(&self) -> &'static str;

    /// One-line description used for fuzzy search and LLM tool selection.
    fn description(&self) -> &'static str;

    /// Category for grouping in the palette (e.g. `"I/O"`, `"LLM"`, `"Flow"`).
    fn category(&self) -> &'static str;

    /// JSON Schema describing the `params` blob this factory accepts.
    /// Validated at write time by [`crate::api::WorkflowClient::add_node`].
    fn spec_schema(&self) -> Schema;

    /// Input ports for new instances of this kind.
    fn inputs(&self) -> Vec<PortSpec>;

    /// Output ports for new instances of this kind.
    fn outputs(&self) -> Vec<PortSpec>;

    /// Long-form doc shown in the inspector's help pane.
    fn doc(&self) -> &'static str {
        ""
    }

    /// Build a `NodeExecutor` from the given `params`.
    ///
    /// Returning `None` means "this factory does not implement runtime
    /// execution yet" — the scheduler will refuse to run a workflow that
    /// contains such nodes. The default impl returns `None` so factories
    /// that only expose metadata can coexist with runnable ones.
    fn build_executor(&self, _params: serde_json::Value) -> Option<Box<dyn NodeExecutor>> {
        None
    }

    /// Materialize a `Tool` description from this factory.
    /// Convenience wrapper around the accessors above.
    fn tool(&self) -> Tool {
        Tool {
            name: self.kind().into(),
            label: self.label().into(),
            description: self.description().into(),
            input_schema: Box::new(self.spec_schema()),
            output_schema: Box::new(schemars::schema_for!(serde_json::Value)),
            category: self.category().into(),
        }
    }
}

/// Reference-stable convenience: register a factory and return the same Arc.
pub async fn noop_build(_params: serde_json::Value) -> Result<Box<dyn std::any::Any + Send + Sync>> {
    Ok(Box::new(()))
}
