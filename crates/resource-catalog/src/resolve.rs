//! Typed resolution accessors and the SQL-identifier builder.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::catalog::ResourceCatalog;
use crate::error::{ResourceError, Result};
use crate::kind::ResourceAddress;

/// A fully-qualified Iceberg table identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcebergIdent {
    pub catalog: String,
    pub schema: String,
    pub table: String,
}

impl IcebergIdent {
    /// Compose a double-quoted, fully-qualified SQL identifier:
    /// `` `catalog`.`schema`.`table` ``. This is the **only** place `iceberg.*`
    /// names are composed into SQL, so quoting and catalog-name handling live
    /// in one spot.
    pub fn sql(&self) -> String {
        format!("\"{}\".\"{}\".\"{}\"", self.catalog, self.schema, self.table)
    }

    /// The three components as a tuple, for catalog APIs that take
    /// `(catalog, namespace, table)`.
    pub fn ident(&self) -> (String, String, String) {
        (self.catalog.clone(), self.schema.clone(), self.table.clone())
    }

    /// Return a new ident with a suffix appended to the table name, useful
    /// for companion tables like `{table}_m`.
    pub fn with_table_suffix(&self, suffix: &str) -> Self {
        Self {
            catalog: self.catalog.clone(),
            schema: self.schema.clone(),
            table: format!("{}{}", self.table, suffix),
        }
    }
}

impl ResourceCatalog {
    /// Resolve a logical name to a fully-qualified Iceberg identifier.
    pub fn resolve_iceberg(&self, name: &str) -> Result<IcebergIdent> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            ResourceAddress::IcebergTable {
                catalog,
                schema,
                table,
            } => Ok(IcebergIdent {
                catalog: catalog.clone(),
                schema: schema.clone(),
                table: table.clone(),
            }),
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "iceberg_table",
                found: kind_str(other),
            }),
        }
    }

    /// Resolve a logical name to the raw (unresolved) filesystem path,
    /// without substituting template placeholders. Useful when the caller
    /// needs the template string itself (e.g. `{N}` patterns passed to
    /// downstream tools that do their own substitution).
    pub fn resolve_path_raw(&self, name: &str) -> Result<PathBuf> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            ResourceAddress::FilePath(p) => Ok(p.clone()),
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "file_path",
                found: kind_str(other),
            }),
        }
    }

    /// Resolve a logical name to a filesystem path (no placeholders).
    pub fn resolve_path(&self, name: &str) -> Result<PathBuf> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            ResourceAddress::FilePath(p) => Ok(self.absolutize(p)),
            ResourceAddress::Database { path, .. } => Ok(self.absolutize(path)),
            ResourceAddress::Doc { path, .. } => Ok(self.absolutize(path)),
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "file_path/database/doc",
                found: kind_str(other),
            }),
        }
    }

    /// Resolve a logical name to a filesystem path, substituting `{key}` from
    /// `subs` into any template placeholders (e.g. `{N}` for per-chromosome
    /// paths). Errors if any placeholder remains unresolved.
    pub fn resolve_path_template(
        &self,
        name: &str,
        subs: &BTreeMap<String, String>,
    ) -> Result<PathBuf> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        let raw = match &entry.address {
            ResourceAddress::FilePath(p) => p.to_string_lossy().to_string(),
            other => {
                return Err(ResourceError::KindMismatch {
                    name: name.to_string(),
                    expected: "file_path",
                    found: kind_str(other),
                })
            }
        };
        let mut out = raw.clone();
        for (k, v) in subs {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        if out.contains('{') {
            return Err(ResourceError::Validation(format!(
                "unresolved placeholder in path resource '{name}': {raw}"
            )));
        }
        Ok(self.absolutize(&PathBuf::from(out)))
    }

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
    pub fn resolve_database(&self, name: &str) -> Result<PathBuf> {
        let entry = self
            .get(name)
            .ok_or_else(|| ResourceError::UnknownResource(name.to_string()))?;
        match &entry.address {
            ResourceAddress::Database { path, .. } => Ok(self.absolutize(path)),
            other => Err(ResourceError::KindMismatch {
                name: name.to_string(),
                expected: "database",
                found: kind_str(other),
            }),
        }
    }
}

fn kind_str(address: &ResourceAddress) -> &'static str {
    match address {
        ResourceAddress::IcebergTable { .. } => "iceberg_table",
        ResourceAddress::FilePath(_) => "file_path",
        ResourceAddress::Endpoint { .. } => "endpoint",
        ResourceAddress::Config { .. } => "config",
        ResourceAddress::Database { .. } => "database",
        ResourceAddress::Doc { .. } => "doc",
    }
}
