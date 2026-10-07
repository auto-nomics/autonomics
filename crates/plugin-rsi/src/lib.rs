//! Plugin-based recursive self-improvement substrate.
//!
//! This crate owns the RSI lifecycle: requests, plugin workspaces,
//! deterministic reports, and trusted publication. Agents draft through a
//! narrow host-owned API; they never receive raw git, gh, host-filesystem, or
//! live-registry access.

pub mod distill;
pub mod environments;
pub mod error;
pub mod feedback;
pub mod github;
pub mod gitrepo;
pub mod infra;
pub mod install;
pub mod layout;
pub mod lifecycle;
pub mod names;
pub mod plugin;
pub mod profile;
pub mod report;
pub mod request;
pub mod tools;
pub mod validate;
pub mod workspace;

pub use container_plugin::manifest::PluginStatus;
pub use distill::{DistillFailure, DistillationCandidate, PluginDistillReport, PluginDistiller};
pub use environments::EnvironmentRegistry;
pub use error::{Error, Result};
pub use evolution_core::{
    ObservationAudience, ObservationRoute, ObservationRouteStatus, ObservationRouteStore,
    RouteDecision,
};
pub use feedback::ObservationRequest;
pub use github::{
    GhPublisher, GhPublisherConfig, GhStatus, MergeOutcome, PluginPublisher,
    PluginPullRequestPublisher, PublishOutcome, PullRequestOutcome,
};
pub use gitrepo::GitRepo;
pub use infra::{
    PluginRegistryControl, RsiInfra, SharedPluginPublisher, SharedPullRequestPublisher,
};
pub use install::{
    GitInstalledPluginSource, InstalledPluginSource, LocalInstalledPluginSource,
    read_git_plugin_source, read_installed_plugin_source, write_git_plugin_source,
    write_local_plugin_source,
};
pub use layout::{PluginLayoutVersion, PluginStateLayout};
pub use lifecycle::{LocalActivationOutcome, PluginLifecycle, ValidationOutcome};
pub use names::validate_plugin_name;
pub use plugin::{
    GitPluginSourceFetcher, PLUGIN_DEVELOPMENT_VFS_ROOT, PluginOperator, PluginSourceFetcher,
    PluginStore,
};
pub use profile::AgentProfile;
pub use report::{GateResult, GateStatus, ValidationReport};
pub use request::{RequestIntent, RequestRecord, RequestSource, RequestStatus, RequestStore};
pub use tools::{PluginDevelopmentToolsetRegistry, plugin_development_tool_registrations};
pub use validate::{Environment, EnvironmentCatalog, validate_workspace};
pub use workspace::PluginWorkspace;
