//! Plugin-based recursive self-improvement substrate.
//!
//! This crate owns the RSI lifecycle: requests, plugin workspaces,
//! deterministic reports, and trusted publication. Agents draft through a
//! narrow host-owned API; they never receive raw git, gh, host-filesystem, or
//! live-registry access.

pub mod distill;
pub mod env_dev;
pub mod env_distill;
pub mod env_manifest;
pub mod env_store;
pub mod env_validate;
pub mod environments;
pub mod error;
pub mod feedback;
pub mod github;
pub mod gitrepo;
pub mod image_registry;
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
pub use env_dev::{
    EnvironmentDevInfra, EnvironmentLocalActivationOutcome, EnvironmentValidationOutcome,
};
pub use env_distill::{
    EnvironmentDistillFailure, EnvironmentDistillReport, EnvironmentDistillationCandidate,
    EnvironmentDistiller,
};
pub use env_manifest::{
    EnvironmentBase, EnvironmentLifecycleMetadata, EnvironmentManifest, EnvironmentSmokeTest,
    EnvironmentStatus,
};
pub use env_store::{
    DEFAULT_LOCAL_NAMESPACE, ENVIRONMENT_DEVELOPMENT_VFS_ROOT, EnvironmentOperator,
    EnvironmentStore, InstalledEnvironment,
};
pub use env_validate::{EnvironmentValidationReport, validate_environment};
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
pub use image_registry::{
    ImagePublisher, ImageRegistryConfig, PodmanImagePublisher, PublishedImageReference,
    SharedImagePublisher,
};
pub use infra::{
    PluginRegistryControl, RsiInfra, SharedPluginPublisher, SharedPullRequestPublisher,
};
pub use install::{
    GitInstalledPluginSource, InstalledPluginSource, LocalInstalledPluginSource,
    read_git_plugin_source, read_installed_plugin_source, remove_installed_plugin_source,
    write_git_plugin_source, write_local_plugin_source,
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
pub use tools::{
    PluginDevelopmentToolsetRegistry, environment_development_tool_registrations,
    plugin_development_tool_registrations,
};
pub use validate::{Environment, EnvironmentCatalog, validate_workspace};
pub use workspace::PluginWorkspace;
