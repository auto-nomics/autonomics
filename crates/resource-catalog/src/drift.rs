//! Declaration-first drift checking.
//!
//! The catalog is the authoritative declaration of what should exist. This
//! module checks the live state against that declaration and reports
//! mismatches as warnings — it never mutates the index.

use crate::catalog::ResourceCatalog;
use crate::kind::ResourceAddress;

/// A single drift warning: a registered resource that is not present in the
/// live state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftWarning {
    pub name: String,
    pub detail: String,
}

/// A point-in-time snapshot of the live state, used to compare against
/// registered resources.
///
/// Currently scoped to Database paths that can be checked synchronously.
/// Storage backend probing requires an async check and is surfaced as a
/// soft warning.
#[derive(Debug, Clone, Default)]
pub struct CatalogSnapshot {
    /// Known-good local paths (for Database/Doc drift detection).
    pub local_paths: std::collections::HashSet<String>,
}

impl ResourceCatalog {
    /// Check registered resources against a live snapshot. Returns warnings
    /// for registered-but-missing resources. Never mutates the index.
    pub fn check_drift(&self, snapshot: &CatalogSnapshot) -> Vec<DriftWarning> {
        let mut warnings = Vec::new();
        for entry in self.list() {
            match &entry.address {
                ResourceAddress::Database { path, .. } => {
                    let abs = self.absolutize(path);
                    if !snapshot.local_paths.contains(path) && !abs.exists() {
                        warnings.push(DriftWarning {
                            name: entry.name.clone(),
                            detail: format!(
                                "database path is registered but not found: {}",
                                abs.display()
                            ),
                        });
                    }
                }
                ResourceAddress::Storage {
                    backend, path, ..
                } => {
                    // If the backend is Local, we can check the filesystem.
                    if let Some(cfg) = self.backend_config(backend) {
                        if let Some(root) = cfg.local_root() {
                            let full = std::path::Path::new(root).join(path);
                            if !full.exists() {
                                warnings.push(DriftWarning {
                                    name: entry.name.clone(),
                                    detail: format!(
                                        "storage path '{path}' not found under local root '{root}'"
                                    ),
                                });
                            }
                            continue;
                        }
                    }
                    // Non-local backends: soft warning (requires async probe).
                    warnings.push(DriftWarning {
                        name: entry.name.clone(),
                        detail: format!(
                            "storage resource at backend '{backend}' path '{path}' — \
                             verify presence (no sync probe for remote backends)"
                        ),
                    });
                }
                _ => {}
            }
        }
        warnings
    }
}
