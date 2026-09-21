//! Execution primitives for ephemeral analysis containers on Podman.
//!
//! [`PodmanConnection`] fixes the capability contract of the connection
//! layer; the CLI implementation in [`podman`] is the production connection.
//! Object storage is authoritative, while shared workspace and panel
//! directories provide the POSIX data plane required by analysis tools.

pub mod config;
pub mod connection;
pub mod error;
pub mod execution;
pub mod gc;
pub mod panel;
pub mod podman;
pub mod types;

pub use config::{
    KEEP_WORKSPACE_ENV, REMOVED_BACKEND_ENV, WORKSPACE_GC_AGE_ENV, WORKSPACE_GC_INTERVAL_ENV,
    ensure_backend_env_removed, keep_workspace_enabled, parse_removed_backend_env,
    workspace_gc_age, workspace_gc_interval,
};
pub use connection::{
    DEFAULT_CONTAINER_WORKDIR, DEFAULT_TIMEOUT_SECS, MAX_CAPTURED_OUTPUT_BYTES, PodmanConnection,
    unique_container_name, workspace_ref,
};
pub use error::ContainerRuntimeError;
pub use execution::ContainerExecutionInfra;
pub use gc::{WorkspaceGcPolicy, WorkspaceGcReport, sweep_workspace};
pub use panel::{PanelCache, PanelFile, PanelManifest};
pub use podman::{PodmanConfig, PodmanRuntime};
pub use types::{
    CachedPanel, ContainerNetwork, ContainerRunRequest, ContainerRunResult, GpuRequest, PanelRef,
    PullPolicy, WorkspaceRef,
};
