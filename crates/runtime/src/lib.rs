//! Runtime for agentik agents.
//!
//! Two entry points:
//! - [`AgentRuntime`] — legacy single-agent runtime (one struct, one agent).
//! - [`RuntimeHost`] — multi-agent host with shared infrastructure.
//!
//! New code should prefer [`RuntimeHost`].

pub mod config;
pub mod host;
pub mod runtime;
pub mod tools;

pub use config::{RuntimeConfig, RuntimeConfigBuilder};
pub use host::{AgentHandle, HostError, HostResult, RuntimeHost, SharedInfra};
pub use runtime::AgentRuntime;
