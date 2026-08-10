//! Declarative ingestion specification: how to load source files into an
//! Iceberg table.
//!
//! An [`IngestionSpec`] is stored on the **target** `ResourceEntry` (which
//! must be `ResourceKind::IcebergTable`). It describes the source file path,
//! format, partition strategy, write mode, and optional archive lifecycle
//! (restore before / archive after).
//!
//! The actual execution (reading files, creating Iceberg tables, inserting
//! data) is performed by an `IngestionExecutor` in the runtime crate, which
//! has access to `Datalake` + `DataFusion`. This keeps the resource-catalog
//! crate free of heavy execution dependencies.

use serde::{Deserialize, Serialize};

// ── Types ────────────────────────────────────────────────────────────────

/// Declarative spec for ingesting source files into an Iceberg table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestionSpec {
    /// Source file path or glob pattern (e.g. `"/data/chrom_*.parquet"`).
    pub source_path: String,
    /// Source file format: `"parquet"`, `"csv"`, or `"tsv"`.
    pub source_format: SourceFormat,
    /// Columns to partition the Iceberg table by (identity transform).
    /// Empty = no partitioning.
    #[serde(default)]
    pub partition_by: Vec<String>,
    /// Write mode for the target table.
    pub mode: WriteMode,
    /// If true, restore source files from archive before ingesting (when the
    /// source is not present locally). Requires `source_resource` to have an
    /// `archive_spec`.
    #[serde(default)]
    pub restore_before: bool,
    /// If true, archive source files after successful ingest.
    #[serde(default)]
    pub archive_after: bool,
    /// Optional logical name of the source resource (for linked archive spec).
    /// When set, `restore_before` / `archive_after` operate on this resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_resource: Option<String>,
    /// Optional CSV-specific options (only used when `source_format` is Csv/Tsv).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv_options: Option<CsvOptions>,
}

/// Source file format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    /// Parquet files (self-describing schema; no explicit schema needed).
    Parquet,
    /// CSV files with configurable delimiter and header.
    Csv,
    /// TSV files (tab-delimited CSV with header).
    Tsv,
}

impl SourceFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceFormat::Parquet => "parquet",
            SourceFormat::Csv => "csv",
            SourceFormat::Tsv => "tsv",
        }
    }
}

/// Write mode for the target Iceberg table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteMode {
    /// Create table if not exists; skip if already has data (idempotent).
    CreateIfNotExists,
    /// Drop and recreate the table, then insert all data (full refresh).
    CreateOrReplace,
    /// Append to an existing table (table must already exist).
    Append,
}

impl WriteMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteMode::CreateIfNotExists => "create_if_not_exists",
            WriteMode::CreateOrReplace => "create_or_replace",
            WriteMode::Append => "append",
        }
    }
}

/// CSV/TSV reading options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvOptions {
    /// Whether the file has a header row.
    #[serde(default = "default_true")]
    pub has_header: bool,
    /// Delimiter character (for Csv, default ','; for Tsv, '\t').
    #[serde(default = "default_comma")]
    pub delimiter: char,
    /// File extension to filter by (e.g. "csv", "tsv", "zst").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_extension: Option<String>,
    /// Compression type: "zstd", "gzip", or None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            has_header: true,
            delimiter: ',',
            file_extension: None,
            compression: None,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_comma() -> char {
    ','
}

/// Outcome of an ingestion operation.
#[derive(Debug, Clone)]
pub struct IngestionOutcome {
    pub rows_written: u64,
    pub files_processed: u64,
    pub duration_ms: u128,
    pub skipped: bool,
}
