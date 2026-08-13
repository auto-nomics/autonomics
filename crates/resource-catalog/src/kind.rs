//! Resource kinds and their typed addresses.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::iceberg_const::CATALOG_NAME;

/// The high-level kind of a resource. Used for indexing and typed resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    /// An Iceberg table, resolved to a fully-qualified SQL identifier.
    ///
    /// **Legacy**: kept for backwards compatibility during the
    /// Iceberg → ObjectStorage migration. New resources should prefer
    /// [`ResourceKind::ObjectStorage`].
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
    /// A partition of parquet (or other self-describing) files in an
    /// object-store bucket — read via DataFusion's `ListingTable`. This is
    /// the post-migration default for tabular reference data (LD-score
    /// panels, frequency tables, SNP lists, …). Replaces the Iceberg
    /// catalog path for purely-read-only archives.
    ObjectStorage,
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
            ResourceKind::ObjectStorage => "object_storage",
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
            "object_storage" => Some(ResourceKind::ObjectStorage),
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
    ///
    /// **Legacy**: kept for backwards compatibility. New tabular resources
    /// should use [`ResourceAddress::ObjectStorage`].
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
    /// A partition of self-describing files in an object-store bucket.
    ///
    /// `bucket` is the scheme-agnostic bucket name (e.g. `"autonomics"`); the
    /// actual scheme (`s3://`, `oss://`, `file://`) is supplied at runtime by
    /// the registered [`opendal::Operator`]. `prefix` is the path within the
    /// bucket (e.g. `"/ld_score/1000g_eur/"`). `file_format` selects the
    /// reader (currently only `Parquet` is wired through `ListingTable`).
    /// `partition_columns` lists the hive-style partition columns present
    /// under `prefix` (e.g. `["chr"]` for chr-partitioned LD matrices); empty
    /// means unpartitioned.
    ObjectStorage {
        bucket: String,
        prefix: String,
        file_format: ObjectFileFormat,
        #[serde(default)]
        partition_columns: Vec<String>,
    },
}

/// Self-describing file format for an [`ResourceAddress::ObjectStorage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectFileFormat {
    /// Apache Parquet. Read by DataFusion's `ListingTable` via the registered
    /// `opendal::Operator` for the bucket.
    Parquet,
}

impl ObjectFileFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ObjectFileFormat::Parquet => "parquet",
        }
    }
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

    /// A parquet (or other self-describing) partition in an object-store bucket.
    /// Convenience for the common case of un-partitioned, single-format data.
    pub fn object_storage(
        bucket: impl Into<String>,
        prefix: impl Into<String>,
    ) -> Self {
        ResourceAddress::ObjectStorage {
            bucket: bucket.into(),
            prefix: prefix.into(),
            file_format: ObjectFileFormat::Parquet,
            partition_columns: Vec::new(),
        }
    }

    /// Full constructor for [`ResourceAddress::ObjectStorage`] with explicit
    /// `file_format` and `partition_columns`.
    pub fn object_storage_with(
        bucket: impl Into<String>,
        prefix: impl Into<String>,
        file_format: ObjectFileFormat,
        partition_columns: Vec<String>,
    ) -> Self {
        ResourceAddress::ObjectStorage {
            bucket: bucket.into(),
            prefix: prefix.into(),
            file_format,
            partition_columns,
        }
    }
}
