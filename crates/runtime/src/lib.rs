//! Runtime for agentik agents.
//!
//! [`RuntimeHost`] is the single entry point — a multi-agent host that owns
//! shared infrastructure, a persistent [`AgentNetwork`] for topology routing,
//! and an agent registry for multiplexed event access. Agents can control
//! the host via [`HostControl`] tools.

pub mod config;
pub mod control;
pub mod host;
pub mod host_tools;
pub mod tools;

pub use config::{RuntimeConfig, RuntimeConfigBuilder};
pub use control::{HostCommand, HostControl, HostStatus};
pub use host::{
    AgentHandle, HostError, HostEvent, HostResult, RuntimeHost, SharedInfra, TaggedEvent,
};
pub use host_tools::host_tools;
