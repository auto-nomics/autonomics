//! # Resource Catalog
//!
//! A centralized, self-describing resource management module. A process-wide
//! singleton [`ResourceCatalog`] is the single source of truth for **all**
//! resources — object storage, endpoints, folded config/env addresses, and
//! database connections.
//!
//! Every other module resolves resources through the catalog by a stable
//! **logical name** instead of hardcoding physical paths, URLs, or table names.
//!
//! ## Model
//!
//! - **Declaration-first**: resources are explicitly registered via
//!   [`ResourceCatalog::register`]. The catalog is authoritative.
//! - **Storage-backed**: all data I/O flows through [`opendal::Operator`]
//!   via named backends registered at bootstrap. A `Storage` resource
//!   references a backend by name + a path within it.
//! - **Config-as-resource**: `.env`-sourced addresses are folded in as
//!   `Config`-kind entries.
//! - **Persistable**: the registered manifest persists to SQLite/Turso.

pub mod archive;
pub mod catalog;
pub mod drift;
pub mod entry;
pub mod error;
pub mod ingestion;
pub mod kind;
pub mod patch;
pub mod persist;
pub mod registry;
pub mod resolve;
pub mod storage;
pub mod validate;

pub use archive::{ArchivableResource, ArchiveOutcome, ArchiveSpec, ArchiveStatus};
pub use catalog::ResourceCatalog;
pub use drift::{CatalogSnapshot, DriftWarning};
pub use entry::ResourceEntry;
pub use patch::ResourcePatch;
pub use error::{ResourceError, Result};
pub use ingestion::{CsvOptions, IngestionOutcome, IngestionSpec, SourceFormat, WriteMode};
pub use kind::{DataFormat, DbKind, DocKind, ResourceAddress, ResourceKind};
pub use persist::{ManifestStore, TursoManifestStore};
pub use registry::ResourceRegistry;
pub use resolve::StorageRef;
pub use storage::{BackendRegistry, SharedBackendRegistry, StorageBackend, StorageConfig};

/// Resolve an endpoint URL from the global catalog, falling back to
/// `fallback` when the global catalog is not set or the name is not
/// registered.
pub fn endpoint_or(logical: &str, fallback: &str) -> String {
    ResourceCatalog::global()
        .and_then(|cat| cat.resolve_endpoint(logical).ok())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests;
