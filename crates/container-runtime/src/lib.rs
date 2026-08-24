//! Runtime-neutral execution primitives for ephemeral analysis containers.
//!
//! The current default backend is the rootless Podman CLI. This avoids
//! requiring a daemon and keeps the host-owned application in the same user
//! session and lifecycle domain as the user's Podman runtime.

pub mod error;
pub mod image;
pub mod podman;
pub mod process;
pub mod runtime;
pub mod types;

pub use error::ContainerRuntimeError;
pub use image::{
    EnsureImageResult, ImageInspect, ImageListOptions, ImageManager, ImagePullResult, ImageRecord,
    ImageRemoveOptions, ImageRemoveResult,
};
pub use podman::PodmanRuntime;
pub use runtime::{
    ContainerRuntime, DEFAULT_CONTAINER_WORKDIR, DEFAULT_TIMEOUT_SECS, MAX_CAPTURED_OUTPUT_BYTES,
    SharedContainerRuntime, unique_container_name,
};
pub use types::{ContainerMount, ContainerRunRequest, ContainerRunResult, PullPolicy};
