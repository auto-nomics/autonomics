//! Runtime for agentik agents.
//!
//! [`RuntimeHost`] is the single entry point — a multi-agent host that owns
//! shared infrastructure, a persistent [`AgentNetwork`] for topology routing,
//! and an agent registry for multiplexed event access.

pub mod config;
pub mod host;
pub mod tools;

pub use config::{RuntimeConfig, RuntimeConfigBuilder};
pub use host::{AgentHandle, HostError, HostResult, RuntimeHost, SharedInfra, TaggedEvent};
