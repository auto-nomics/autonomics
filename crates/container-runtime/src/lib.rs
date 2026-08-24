//! Runtime-neutral execution primitives for ephemeral analysis containers.
//!
//! The k3s backend is the sole execution path. Object storage is authoritative,
//! while shared workspace and panel PVCs provide the POSIX data plane required
//! by analysis tools.

pub mod error;
pub mod k3s;
pub mod panel;
pub mod runtime;
pub mod types;

pub use error::ContainerRuntimeError;
pub use k3s::workspace_ref;
pub use k3s::{K3sConfig, K3sRuntime};
pub use panel::{PanelCache, PanelFile, PanelManifest};
pub use runtime::{
    ContainerRuntime, DEFAULT_CONTAINER_WORKDIR, DEFAULT_TIMEOUT_SECS, MAX_CAPTURED_OUTPUT_BYTES,
    SharedContainerRuntime, unique_container_name,
};
pub use types::{
    CachedPanel, ContainerRunRequest, ContainerRunResult, PanelRef, PullPolicy, WorkspaceRef,
};
