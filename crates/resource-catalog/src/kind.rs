//! Resource kinds and their typed addresses.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::iceberg_const::CATALOG_NAME;

/// The high-level kind of a resource. Used for indexing and typed resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    /// An Iceberg table, resolved to a fully-qualified SQL identifier.
    IcebergTable,
    /// A host filesystem path (may contain `{placeholder}` templates).
    FilePath,
    /// An external API base URL.
    Endpoint,
    /// A folded configuration/env address (`key -> value`).
    Config,
    /// A local database path (SQLite/Turso/Postgres).
    Database,
    /// A documentation/log/archive path.
    Doc,
}

impl ResourceKind {
    /// The stable string form used in serialization and diagnostics.
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceKind::IcebergTable => "iceberg_table",
            ResourceKind::FilePath => "file_path",
            ResourceKind::Endpoint => "endpoint",
            ResourceKind::Config => "config",
            ResourceKind::Database => "database",
            ResourceKind::Doc => "doc",
        }
    }

    /// Parse the stable string form back into a [`ResourceKind`].
    pub fn from_str(s: &str) -> Option<ResourceKind> {
        match s {
            "iceberg_table" => Some(ResourceKind::IcebergTable),
            "file_path" => Some(ResourceKind::FilePath),
            "endpoint" => Some(ResourceKind::Endpoint),
            "config" => Some(ResourceKind::Config),
            "database" => Some(ResourceKind::Database),
            "doc" => Some(ResourceKind::Doc),
            _ => None,
        }
    }
}

/// Backend variant for [`ResourceAddress::Database`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbKind {
    Sqlite,
    Turso,
    Postgres,
}

impl DbKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DbKind::Sqlite => "sqlite",
            DbKind::Turso => "turso",
            DbKind::Postgres => "postgres",
        }
    }
}

/// Document category for [`ResourceAddress::Doc`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocKind {
    Docs,
    Logs,
    Archive,
    Notes,
    Fixtures,
}

/// A typed, self-describing address. The concrete shape depends on the kind.
///
/// This is the physical side of the catalog: a [`ResourceEntry`](crate::entry::ResourceEntry)
/// maps a stable logical name to one of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceAddress {
    /// An Iceberg table. `catalog` defaults to [`CATALOG_NAME`].
    IcebergTable {
        catalog: String,
        schema: String,
        table: String,
    },
    /// A host filesystem path. May contain `{placeholder}` templates resolved
    /// via [`crate::catalog::ResourceCatalog::resolve_path_template`].
    FilePath(PathBuf),
    /// An external API base URL. Clients append path segments to it.
    Endpoint { url: String },
    /// A folded config/env address.
    Config { key: String, value: String },
    /// A local database path.
    Database { kind: DbKind, path: PathBuf },
    /// A documentation/log/archive path.
    Doc { kind: DocKind, path: PathBuf },
}

impl ResourceAddress {
    /// An Iceberg table in the default catalog (see [`CATALOG_NAME`]).
    pub fn iceberg(schema: impl Into<String>, table: impl Into<String>) -> Self {
        ResourceAddress::IcebergTable {
            catalog: CATALOG_NAME.to_string(),
            schema: schema.into(),
            table: table.into(),
        }
    }

    /// An Iceberg table in a specific catalog.
    pub fn iceberg_in(
        catalog: impl Into<String>,
        schema: impl Into<String>,
        table: impl Into<String>,
    ) -> Self {
        ResourceAddress::IcebergTable {
            catalog: catalog.into(),
            schema: schema.into(),
            table: table.into(),
        }
    }

    /// A host filesystem path.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        ResourceAddress::FilePath(path.into())
    }

    /// An external API base URL.
    pub fn endpoint(url: impl Into<String>) -> Self {
        ResourceAddress::Endpoint { url: url.into() }
    }

    /// A folded config/env address.
    pub fn config(key: impl Into<String>, value: impl Into<String>) -> Self {
        ResourceAddress::Config {
            key: key.into(),
            value: value.into(),
        }
    }

    /// A local database path.
    pub fn database(kind: DbKind, path: impl Into<PathBuf>) -> Self {
        ResourceAddress::Database {
            kind,
            path: path.into(),
        }
    }

    /// A documentation/log/archive path.
    pub fn doc(kind: DocKind, path: impl Into<PathBuf>) -> Self {
        ResourceAddress::Doc {
            kind,
            path: path.into(),
        }
    }
}
