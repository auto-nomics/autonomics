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
pub mod panel;
pub mod podman;
pub mod types;

pub use config::{REMOVED_BACKEND_ENV, ensure_backend_env_removed, parse_removed_backend_env};
pub use connection::{
    DEFAULT_CONTAINER_WORKDIR, DEFAULT_TIMEOUT_SECS, MAX_CAPTURED_OUTPUT_BYTES, PodmanConnection,
    unique_container_name, workspace_ref,
};
pub use error::ContainerRuntimeError;
pub use execution::ContainerExecutionInfra;
pub use panel::{PanelCache, PanelFile, PanelManifest};
pub use podman::{PodmanConfig, PodmanRuntime};
pub use types::{
    CachedPanel, ContainerNetwork, ContainerRunRequest, ContainerRunResult, PanelRef, PullPolicy,
    WorkspaceRef,
};
