//! Deterministic validation for plugin workspaces.
pub mod compiler;
pub mod manifest_loader;
pub mod policy;
pub mod script_review;
pub mod secret_scanner;
pub mod workspace_validator;

pub(crate) use compiler::{RegistryCompiler, registry_compile};
pub(crate) use manifest_loader::ManifestLoader;
pub(crate) use workspace_validator::{WorkspaceValidator, validate_workspace};
