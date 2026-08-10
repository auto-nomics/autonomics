//! # Resource Catalog
//!
//! A centralized, self-describing resource management module. A process-wide
//! singleton [`ResourceCatalog`] is the single source of truth for **all**
//! resources — Iceberg tables, file paths, external API endpoints, folded
//! config/env addresses, database paths, and docs/logs/archives.
//!
//! Every other module resolves resources through the catalog by a stable
//! **logical name** instead of hardcoding physical table names, paths, or URLs.
//!
//! ## Model
//!
//! - **Declaration-first**: resources are explicitly registered via
//!   [`ResourceCatalog::register`] / [`ResourceCatalog::register_provider`].
//!   The catalog is authoritative; scanning is used only for
//!   [`check_drift`](ResourceCatalog::check_drift) validation/warnings, never
//!   to build the index.
//! - **Self-describing**: each [`ResourceEntry`] carries its own description
//!   and metadata, so the catalog can be enumerated and understood without
//!   external docs.
//! - **Config-as-resource**: `.env`-sourced addresses are folded in as
//!   `Config`-kind entries, so configuration is itself a resource.
//! - **Persistable**: the registered manifest (never a live scan) persists to
//!   SQLite/Turso, matching the `DagHistory` pattern.

pub mod archive;
pub mod catalog;
pub mod drift;
pub mod entry;
pub mod error;
pub mod iceberg_const;
pub mod kind;
pub mod persist;
pub mod provider;
pub mod registry;
pub mod resolve;
pub mod validate;

pub use catalog::ResourceCatalog;
pub use drift::{CatalogSnapshot, DriftWarning};
pub use entry::ResourceEntry;
pub use error::{ResourceError, Result};
pub use iceberg_const::CATALOG_NAME;
pub use kind::{DbKind, DocKind, ResourceAddress, ResourceKind};
pub use persist::{ManifestStore, TursoManifestStore};
pub use provider::ResourceProvider;
pub use archive::{ArchiveOutcome, ArchiveSpec, ArchiveStatus, ArchivableResource};
pub use registry::ResourceRegistry;

/// Resolve an endpoint URL from the global catalog, falling back to
/// `fallback` when the global catalog is not set or the name is not
/// registered. Convenience for SDK crates that want one-liner resolution
/// without depending on the full catalog API.
pub fn endpoint_or(logical: &str, fallback: &str) -> String {
    ResourceCatalog::global()
        .and_then(|cat| cat.resolve_endpoint(logical).ok())
        .unwrap_or_else(|| fallback.to_string())
}
pub use resolve::IcebergIdent;

#[cfg(test)]
mod tests;
