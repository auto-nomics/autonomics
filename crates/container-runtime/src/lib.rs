//! Runtime-neutral execution primitives for ephemeral analysis containers.
//!
//! K3s and Podman implement the same ephemeral-container contract. Object
//! storage is authoritative, while shared workspace and panel directories
//! provide the POSIX data plane required by analysis tools.

pub mod config;
pub mod dev;
pub mod error;
pub mod execution;
pub mod k3s;
pub mod panel;
pub mod podman;
pub mod runtime;
pub mod types;

pub use config::{CONTAINER_BACKEND_ENV, ContainerBackend};
pub use error::ContainerRuntimeError;
pub use execution::{ContainerExecutionConfig, ContainerExecutionInfra};
pub use k3s::{K3sConfig, K3sRuntime};
pub use panel::{PanelCache, PanelFile, PanelManifest};
pub use podman::{PodmanConfig, PodmanRuntime};
pub use runtime::{
    ContainerRuntime, DEFAULT_CONTAINER_WORKDIR, DEFAULT_TIMEOUT_SECS, MAX_CAPTURED_OUTPUT_BYTES,
    SharedContainerRuntime, unique_container_name, workspace_ref,
};
pub use types::{
    CachedPanel, ContainerRunRequest, ContainerRunResult, DevExecRequest, DevImageBuildRequest,
    DevImageBuildResult, DevWorkspaceCreate, DevWorkspaceStatus, PanelRef, PullPolicy,
    WorkspaceRef,
};
