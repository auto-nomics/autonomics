//! Network-level events emitted by the conductor for external observers
//! (TUI, logging, metrics).

use serde::{Deserialize, Serialize};

/// An event emitted by [`AgentNetwork`](crate::AgentNetwork) during its run
/// loop. Observers subscribe via the `UnboundedSender<NetworkEvent>` passed
/// to [`AgentNetwork::build`](crate::AgentNetwork::build).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkEvent {
    /// The network started — all nodes spawned, initial prompts injected.
    Started { name: String, node_count: usize },

    /// A node received a message and is about to start processing.
    NodeTurn { node: String, round: usize },

    /// Streaming text from a node's LLM response.
    AgentText { node: String, text: String },

    /// A message was routed from one node to another across an edge.
    MessageRouted {
        from: String,
        to: String,
        round: usize,
    },

    /// A node emitted a tool call.
    ToolCall { node: String, tool: String },

    /// A node's tool returned a result.
    ToolResult { node: String, ok: bool },

    /// A node encountered an error.
    AgentError { node: String, error: String },

    /// The network has terminated.
    Finished {
        reason: String,
        rounds: usize,
        final_node: Option<String>,
    },
}
