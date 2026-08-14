//! Resource kinds, typed addresses, and format descriptors.
//!
//! This is the **data model** layer of the catalog: pure types with no
//! dependency on opendal or any I/O backend. The opendal-backed types live
//! in [`crate::storage`].

use serde::{Deserialize, Serialize};

/// The high-level kind of a resource. Used for indexing and typed resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    /// A data resource in a registered storage backend (Local, S3, ...).
    /// Resolved to an [`crate::storage::StorageRef`] (operator + path).
    ///
    /// This is the unified replacement for the former `IcebergTable`,
    /// `ObjectStorage`, `FilePath`, and `Doc` kinds.
    Storage,
    /// An external API base URL.
    Endpoint,
    /// A folded configuration/env address (`key -> value`).
    Config,
    /// A local database connection (SQLite/Turso/Postgres).
    Database,
}

impl ResourceKind {
    /// The stable string form used in serialization and diagnostics.
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceKind::Storage => "storage",
            ResourceKind::Endpoint => "endpoint",
            ResourceKind::Config => "config",
            ResourceKind::Database => "database",
        }
    }

    /// Parse the stable string form back into a [`ResourceKind`].
    pub fn from_str(s: &str) -> Option<ResourceKind> {
        match s {
            "storage" => Some(ResourceKind::Storage),
            "endpoint" => Some(ResourceKind::Endpoint),
            "config" => Some(ResourceKind::Config),
            "database" => Some(ResourceKind::Database),
            _ => None,
        }
    }
}

/// Self-describing data format for a [`ResourceAddress::Storage`] entry.
///
/// Determines how downstream readers (DataFusion `ListingTable`, custom
/// parsers) interpret the files at the resolved path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataFormat {
    /// Apache Parquet — read via DataFusion `ListingTable`.
    Parquet,
    /// CSV with configurable delimiter and header.
    Csv,
    /// TSV (tab-delimited CSV with header).
    Tsv,
    /// Non-tabular files (docs, binaries, directories of mixed content).
    Raw,
}

impl DataFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            DataFormat::Parquet => "parquet",
            DataFormat::Csv => "csv",
            DataFormat::Tsv => "tsv",
            DataFormat::Raw => "raw",
        }
    }
}

/// Backend variant for [`ResourceAddress::Database`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Document category — kept as an organizational hint for metadata.
///
/// No longer part of [`ResourceAddress`] after the storage unification;
/// callers that need to categorize a doc/logs/archive resource put the kind
/// into `ResourceEntry::metadata` (e.g. `metadata["doc_kind"] = "notes"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocKind {
    Docs,
    Logs,
    Archive,
    Notes,
    Fixtures,
}

impl DocKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocKind::Docs => "docs",
            DocKind::Logs => "logs",
            DocKind::Archive => "archive",
            DocKind::Notes => "notes",
            DocKind::Fixtures => "fixtures",
        }
    }
}

/// A typed, self-describing address. The concrete shape depends on the kind.
///
/// This is the physical side of the catalog: a [`ResourceEntry`](crate::entry::ResourceEntry)
/// maps a stable logical name to one of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceAddress {
    /// A data resource in a registered storage backend.
    ///
    /// `backend` is the name of a backend registered on the catalog at
    /// bootstrap (e.g. `"default"`, `"s3-prod"`). The catalog resolves it
    /// to an [`opendal::Operator`](opendal::Operator) via
    /// [`resolve_storage`](crate::catalog::ResourceCatalog::resolve_storage).
    ///
    /// `path` is the operator-relative path (e.g. `"ld_score/1000g_eur/"`).
    /// `format` selects the reader. `partition_columns` lists hive-style
    /// partition columns present under `path` (e.g. `["chr"]` for
    /// chr-partitioned data); empty means unpartitioned.
    Storage {
        backend: String,
        path: String,
        format: DataFormat,
        #[serde(default)]
        partition_columns: Vec<String>,
    },
    /// An external API base URL. Clients append path segments to it.
    Endpoint { url: String },
    /// A folded config/env address.
    Config { key: String, value: String },
    /// A database connection. `path` is a filesystem path for SQLite, or a
    /// connection URL for Turso/Postgres.
    Database { kind: DbKind, path: String },
}

impl ResourceAddress {
    // ── Storage constructors ───────────────────────────────────────

    /// A Parquet dataset in backend `backend` at `path`.
    pub fn storage(backend: impl Into<String>, path: impl Into<String>) -> Self {
        ResourceAddress::Storage {
            backend: backend.into(),
            path: path.into(),
            format: DataFormat::Parquet,
            partition_columns: Vec::new(),
        }
    }

    /// Full constructor for a Storage address with explicit format and
    /// partition columns.
    pub fn storage_with(
        backend: impl Into<String>,
        path: impl Into<String>,
        format: DataFormat,
        partition_columns: Vec<String>,
    ) -> Self {
        ResourceAddress::Storage {
            backend: backend.into(),
            path: path.into(),
            format,
            partition_columns,
        }
    }

    // ── Other constructors ─────────────────────────────────────────

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

    /// A database connection. `path` is a filesystem path (SQLite) or URL.
    pub fn database(kind: DbKind, path: impl Into<String>) -> Self {
        ResourceAddress::Database {
            kind,
            path: path.into(),
        }
    }
}
