//! Plugin-based recursive self-improvement substrate.
//!
//! This crate owns the RSI lifecycle: requests, proposal workspaces,
//! deterministic reports, and trusted publication. Agents draft through a
//! narrow host-owned API; they never receive raw git, gh, host-filesystem, or
//! live-registry access.

mod candidate;
pub mod development;
pub mod error;
pub mod github;
pub mod gitrepo;
pub mod install;
pub mod lifecycle;
pub mod names;
pub mod node;
pub mod profile;
pub mod proposal;
pub mod report;
pub mod request;
pub mod tools;
pub mod validate;
pub mod workspace;

pub use candidate::DevelopmentChanges;
pub use development::PluginDevelopment;
pub use error::{Error, Result};
pub use github::{
    GhPublisher, GhPublisherConfig, GhStatus, MergeOutcome, PluginPublisher,
    PluginPullRequestPublisher, PublishOutcome, PullRequestOutcome,
};
pub use gitrepo::GitRepo;
pub use install::{InstalledPluginSource, read_git_plugin_source, write_git_plugin_source};
pub use lifecycle::{PluginLifecycle, ValidationOutcome};
pub use names::validate_plugin_name;
pub use node::NodeDevelopment;
pub use profile::AgentProfile;
pub use proposal::{
    GitPluginSourceFetcher, PluginSourceFetcher, Proposal, ProposalAction, ProposalStatus,
    ProposalStore,
};
pub use report::{GateResult, GateStatus, ValidationReport};
pub use request::{RequestIntent, RequestRecord, RequestSource, RequestStatus, RequestStore};
pub use tools::{PluginDevelopmentToolsetRegistry, plugin_development_tool_registrations};
pub use validate::{Environment, EnvironmentCatalog, validate_workspace};
pub use workspace::ProposalWorkspace;
