//! Error types for the network engine.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetworkError {
    /// A node name referenced in an edge or termination condition does not
    /// exist in the node list.
    #[error("node not found in spec: {0}")]
    NodeNotFound(String),

    /// A profile name referenced by a node could not be resolved to an
    /// [`AgentProfile`](agentik_core::AgentProfile) by the caller.
    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    /// The underlying [`RuntimeHost`](runtime::RuntimeHost) failed to
    /// spawn an agent.
    #[error("host error: {0}")]
    Host(#[from] runtime::HostError),

    /// The network spec is malformed (dangling edges, missing nodes, etc.).
    #[error("topology error: {0}")]
    Topology(String),

    /// An agent in the network encountered an unrecoverable error.
    #[error("agent error in node '{node}': {error}")]
    AgentError { node: String, error: String },
}
