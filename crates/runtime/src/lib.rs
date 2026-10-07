//! Runtime for agentik agents.
//!
//! [`RuntimeHost`] is the single entry point — a multi-agent host that owns
//! shared infrastructure, a persistent [`AgentNetwork`] for topology routing,
//! and an agent registry for multiplexed event access. Agents can control
//! the host via [`HostControl`] tools.

pub mod catalog_tools;
pub mod config;
pub mod control;
pub mod error;
pub mod evolution_tools;
pub mod host;
pub mod host_tools;
pub mod instance_lock;
pub mod memory_kms;
pub mod model_bootstrap;
pub mod skill_workflow_tools;
pub mod tools;

pub use config::{RuntimeConfig, RuntimeConfigBuilder};
pub use control::{HostCommand, HostControl, HostStatus};
pub use error::{Error, Result};
pub use host::{
    AgentHandle, HostEvent, NextEvent, RuntimeHost, SharedInfra, TaggedEvent,
    bibliography_file_storage,
};
pub use host_tools::host_tools;
