//! Typed resolution accessors.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::catalog::ResourceCatalog;
use crate::error::{ResourceError, Result};
use crate::kind::ResourceAddress;
pub use crate::storage::StorageRef;

impl ResourceCatalog {
    /// Resolve a logical name to an external API base URL.
    pub fn resolve_endpoint(&self, name: &str) -> Result<String> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            ResourceAddress::Endpoint { url } => Ok(url.clone()),
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "endpoint",
                found: kind_str(other),
            }),
        }
    }

    /// Resolve a logical name to a folded config value.
    pub fn resolve_config(&self, name: &str) -> Result<String> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            ResourceAddress::Config { value, .. } => Ok(value.clone()),
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "config",
                found: kind_str(other),
            }),
        }
    }

    /// Resolve a logical name to a database path.
    ///
    /// For SQLite, the path is absolutized against the catalog's `base_dir`.
    /// For Turso/Postgres, the path (a connection URL) is returned as-is.
    pub fn resolve_database(&self, name: &str) -> Result<PathBuf> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            ResourceAddress::Database { kind, path } => {
                if matches!(kind, crate::kind::DbKind::Sqlite) {
                    Ok(self.absolutize(path))
                } else {
                    Ok(PathBuf::from(path))
                }
            }
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "database",
                found: kind_str(other),
            }),
        }
    }

    /// Resolve the raw (unresolved) storage path template, without
    /// substituting placeholders or building an operator. Useful when the
    /// caller needs the template string itself (e.g. `{N}` patterns).
    pub fn resolve_storage_path_raw(&self, name: &str) -> Result<String> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            ResourceAddress::Storage { path, .. } => Ok(path.clone()),
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "storage",
                found: kind_str(other),
            }),
        }
    }
}

/// Human-readable kind string for a `ResourceAddress` variant.
pub(crate) fn kind_str(address: &ResourceAddress) -> &'static str {
    match address {
        ResourceAddress::Storage { .. } => "storage",
        ResourceAddress::Endpoint { .. } => "endpoint",
        ResourceAddress::Config { .. } => "config",
        ResourceAddress::Database { .. } => "database",
    }
}

/// Suppress unused import warning — `BTreeMap` is re-exported for
/// `resolve_storage_template` callers.
#[allow(dead_code)]
fn _ensure_btreemap_import(_m: &BTreeMap<String, String>) {}
