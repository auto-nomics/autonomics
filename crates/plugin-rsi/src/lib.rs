//! Plugin-based recursive self-improvement substrate.
//!
//! The crate is organized as a set of daemon-owned domains behind a stable
//! root facade. Agent tools and runtime integrations consume the re-exported
//! API rather than reaching into storage, git, registries, or validators.
//!
//! * [`foundation`] primitives: errors, names, reports, safe workspaces.
//! * [`intake`] request/profiling inputs.
//! * [`plugin`] plugin stores, lifecycle, validation, and source registry.
//! * [`environment`] reusable runtime environments.
//! * [`publication`] trusted external publication adapters.
//! * [`runtime`] daemon layout and subsystem orchestration.
//! * [`vcs`] the only git adapter.
//! * [`tools`] narrow agent-facing tool surface.

pub mod tools;

pub(crate) mod environment;
pub(crate) mod foundation;
pub(crate) mod intake;
pub(crate) mod plugin;
pub(crate) mod publication;
pub(crate) mod runtime;
pub(crate) mod vcs;

pub use container_plugin::manifest::PluginStatus;
pub use environment::catalog::{Environment, EnvironmentCatalog};
pub use environment::development::{
    EnvironmentDevInfra, EnvironmentLocalActivationOutcome, EnvironmentValidationOutcome,
};
pub use environment::distiller::{
    EnvironmentDistillFailure, EnvironmentDistillReport, EnvironmentDistillationCandidate,
    EnvironmentDistiller,
};
pub use environment::image_registry::{
    ImagePublisher, ImageRegistryConfig, PodmanImagePublisher, PublishedImageReference,
    SharedImagePublisher,
};
pub use environment::manifest::{
    EnvironmentBase, EnvironmentLifecycleMetadata, EnvironmentManifest, EnvironmentSmokeTest,
    EnvironmentStatus,
};
pub use environment::registry::EnvironmentRegistry;
pub use environment::store::{
    DEFAULT_LOCAL_NAMESPACE, ENVIRONMENT_DEVELOPMENT_VFS_ROOT, EnvironmentOperator,
    EnvironmentStore, InstalledEnvironment,
};
pub use environment::validation::{EnvironmentValidationReport, validate_environment};
pub use evolution_core::{
    ObservationAudience, ObservationRoute, ObservationRouteStatus, ObservationRouteStore,
    RouteDecision,
};
pub use foundation::error::{Error, Result};
pub use foundation::names::validate_plugin_name;
pub use foundation::report::{GateResult, GateStatus, ValidationReport};
pub use foundation::workspace::PluginWorkspace;
pub use intake::feedback::ObservationRequest;
pub use intake::profile::AgentProfile;
pub use intake::request::{
    RequestIntent, RequestRecord, RequestSource, RequestStatus, RequestStore,
};
pub use plugin::distiller::{
    DistillFailure, DistillationCandidate, PluginDistillReport, PluginDistiller,
};
pub use plugin::lifecycle::{LocalActivationOutcome, PluginLifecycle, ValidationOutcome};
pub use plugin::source_registry::{
    GitInstalledPluginSource, InstalledPluginSource, LocalInstalledPluginSource,
    read_git_plugin_source, read_installed_plugin_source, remove_installed_plugin_source,
    write_git_plugin_source, write_local_plugin_source,
};
pub use plugin::store::{
    GitPluginSourceFetcher, PLUGIN_DEVELOPMENT_VFS_ROOT, PluginCacheActiveSource,
    PluginCacheReport, PluginOperator, PluginScriptCache, PluginSourceFetcher, PluginStore,
};
pub use plugin::validation::workspace_validator::validate_workspace;
pub use publication::github::{
    GhPublisher, GhPublisherConfig, GhStatus, MergeOutcome, PluginPublisher,
    PluginPullRequestPublisher, PublishOutcome, PullRequestOutcome,
};
pub use runtime::infra::{
    PluginInfra, PluginRegistryControl, SharedPluginPublisher, SharedPullRequestPublisher,
};
pub use runtime::layout::{PluginLayoutVersion, PluginStateLayout};
pub use tools::{
    PluginDevelopmentToolsetRegistry, environment_development_tool_registrations,
    plugin_development_tool_registrations,
};
pub use vcs::gitrepo::GitRepo;
