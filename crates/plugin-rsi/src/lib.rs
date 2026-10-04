//! Plugin-based recursive self-improvement substrate.
//!
//! This crate owns the RSI lifecycle: requests, proposal workspaces,
//! deterministic reports, and trusted publication. Agents draft through a
//! narrow host-owned API; they never receive raw git, gh, host-filesystem, or
//! live-registry access.

mod agent_run;
pub mod development;
pub mod error;
pub mod github;
pub mod gitrepo;
pub mod install;
pub mod names;
pub mod node;
pub mod proposal;
pub mod report;
pub mod request;
pub mod validate;
pub mod workspace;

pub use agent_run::CodingAgentRun;
pub use development::PluginDevelopment;
pub use error::{Error, Result};
pub use github::{GhPublisher, GhPublisherConfig, GhStatus};
pub use gitrepo::GitRepo;
pub use install::write_git_plugin_source;
pub use names::validate_plugin_name;
pub use node::NodeDevelopment;
pub use proposal::{Proposal, ProposalAction, ProposalStatus, ProposalStore};
pub use report::{GateResult, GateStatus, ValidationReport};
pub use request::{RequestIntent, RequestRecord, RequestSource, RequestStatus, RequestStore};
pub use validate::{Environment, EnvironmentCatalog, validate_workspace};
pub use workspace::ProposalWorkspace;
