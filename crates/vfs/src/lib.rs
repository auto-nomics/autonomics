mod authorized;

pub mod mount;
pub mod permission;
pub mod storage;
pub mod vbash;

pub use mount::{
    BackendConfig, BackendDefinition, MountDefinition, MountHandle, MountedObjectStore, VfsManifest,
};
pub use storage::OpendalFileStorage;
pub use vbash::vbash_registrations;

// Re-export so downstream crates can reference opendal error/operator types
// without taking a direct dependency on the opendal crate.
pub use opendal;
