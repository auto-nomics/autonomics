//! Registration-time validation.

use crate::entry::ResourceEntry;
use crate::error::{ResourceError, Result};
use crate::kind::ResourceAddress;

/// Validate a [`ResourceEntry`] before it is registered.
///
/// Catches malformed addresses early (at registration time) so a mistyped
/// resource surfaces at startup rather than at first resolution.
pub fn validate(entry: &ResourceEntry) -> Result<()> {
    if entry.name.is_empty() {
        return Err(ResourceError::Validation("empty resource name".into()));
    }

    match &entry.address {
        ResourceAddress::IcebergTable {
            catalog,
            schema,
            table,
        } => {
            if catalog.is_empty() {
                return Err(ResourceError::Validation(format!(
                    "iceberg resource '{}' has empty catalog",
                    entry.name
                )));
            }
            if schema.is_empty() || table.is_empty() {
                return Err(ResourceError::Validation(format!(
                    "iceberg resource '{}' needs both schema and table",
                    entry.name
                )));
            }
        }
        ResourceAddress::FilePath(p) => {
            if p.as_os_str().is_empty() {
                return Err(ResourceError::Validation(format!(
                    "file-path resource '{}' has an empty path",
                    entry.name
                )));
            }
        }
        ResourceAddress::Endpoint { url } => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(ResourceError::Validation(format!(
                    "endpoint '{}' is not an http(s) URL: {url}",
                    entry.name
                )));
            }
        }
        ResourceAddress::Config { key, .. } => {
            if key.is_empty() {
                return Err(ResourceError::Validation(format!(
                    "config resource '{}' has an empty key",
                    entry.name
                )));
            }
        }
        ResourceAddress::Database { path, .. } => {
            if path.as_os_str().is_empty() {
                return Err(ResourceError::Validation(format!(
                    "database resource '{}' has an empty path",
                    entry.name
                )));
            }
        }
        ResourceAddress::Doc { path, .. } => {
            if path.as_os_str().is_empty() {
                return Err(ResourceError::Validation(format!(
                    "doc resource '{}' has an empty path",
                    entry.name
                )));
            }
        }
        ResourceAddress::ObjectStorage {
            bucket, prefix, ..
        } => {
            if bucket.is_empty() {
                return Err(ResourceError::Validation(format!(
                    "object_storage resource '{}' has an empty bucket",
                    entry.name
                )));
            }
            if !prefix.starts_with('/') {
                return Err(ResourceError::Validation(format!(
                    "object_storage resource '{}' prefix must start with '/': {}",
                    entry.name,
                    prefix
                )));
            }
        }
    }

    Ok(())
}
