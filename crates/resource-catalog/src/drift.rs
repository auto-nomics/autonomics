//! Declaration-first drift checking.
//!
//! The catalog is the authoritative declaration of what should exist. This
//! module checks the live state (Iceberg catalog, filesystem) against that
//! declaration and reports mismatches as warnings — it never mutates the
//! index and never builds the index from a scan.

use std::collections::HashSet;

use crate::catalog::ResourceCatalog;
use crate::kind::ResourceAddress;

/// A single drift warning: a registered resource that is not present in the
/// live state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftWarning {
    /// The logical resource name.
    pub name: String,
    /// Human-readable description of the mismatch.
    pub detail: String,
}

/// A point-in-time snapshot of the live Iceberg catalog, used to compare
/// against registered tables. The caller builds this (e.g. from
/// `Datalake::list_all_tables`) so this crate stays decoupled from `datalake`.
#[derive(Debug, Clone, Default)]
pub struct CatalogSnapshot {
    /// The set of live tables as `(namespace, table)` pairs. Multi-segment
    /// namespaces are joined with `.`.
    pub tables: HashSet<(String, String)>,
}

impl ResourceCatalog {
    /// Check registered resources against a live snapshot. Returns warnings
    /// for registered-but-missing resources. Never mutates the index; never
    /// fails startup on its own.
    pub fn check_drift(&self, snapshot: &CatalogSnapshot) -> Vec<DriftWarning> {
        let mut warnings = Vec::new();
        for entry in self.list() {
            match &entry.address {
                ResourceAddress::IcebergTable { schema, table, .. } => {
                    if !snapshot.tables.contains(&(schema.clone(), table.clone())) {
                        warnings.push(DriftWarning {
                            name: entry.name.clone(),
                            detail: format!(
                                "iceberg table `{schema}.{table}` is registered but not found in the live catalog"
                            ),
                        });
                    }
                }
                ResourceAddress::FilePath(p)
                | ResourceAddress::Database { path: p, .. }
                | ResourceAddress::Doc { path: p, .. } => {
                    let abs = self.absolutize(p);
                    if !abs.exists() {
                        warnings.push(DriftWarning {
                            name: entry.name.clone(),
                            detail: format!("path is registered but not found: {}", abs.display()),
                        });
                    }
                }
                ResourceAddress::ObjectStorage { bucket, prefix, .. } => {
                    // ObjectStorage drift requires a live object store probe; we
                    // surface a soft warning that includes the resolved prefix
                    // so an operator can verify out-of-band. The snapshot type
                    // is intentionally Iceberg-scoped — extending it to OSS is
                    // tracked in a separate task.
                    warnings.push(DriftWarning {
                        name: entry.name.clone(),
                        detail: format!(
                            "object_storage resource declared at bucket `{bucket}` prefix `{prefix}` — \
                             verify presence in the OSS bucket (no live probe wired)"
                        ),
                    });
                }
                _ => {}
            }
        }
        warnings
    }
}
