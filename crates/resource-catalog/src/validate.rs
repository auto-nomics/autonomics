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
        ResourceAddress::Storage {
            backend, path, ..
        } => {
            if backend.is_empty() {
                return Err(ResourceError::Validation(format!(
                    "storage resource '{}' has an empty backend name",
                    entry.name
                )));
            }
            if path.is_empty() {
                return Err(ResourceError::Validation(format!(
                    "storage resource '{}' has an empty path",
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
            if path.is_empty() {
                return Err(ResourceError::Validation(format!(
                    "database resource '{}' has an empty path",
                    entry.name
                )));
            }
        }
    }

    Ok(())
}
