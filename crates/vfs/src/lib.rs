mod authorized;
mod execution_mount;
mod ssh;
mod ssh_endpoint;

pub mod browse;
pub mod cli;
pub mod credential;
pub mod mount;
pub mod permission;
pub mod ssh_channel;
pub mod storage;
pub mod uri;
pub mod vbash;

pub use mount::{
    BackendConfig, BackendDefinition, MountDefinition, MountHandle, MountedObjectStore, VfsManifest,
};
pub use ssh::{SshLocation, build_ssh_operator};
pub use ssh_endpoint::SshEndpoint;
pub use storage::OpendalFileStorage;
pub use vbash::vbash_registrations;

// Re-export so downstream crates can reference opendal error/operator types
// without taking a direct dependency on the opendal crate.
pub use opendal;
