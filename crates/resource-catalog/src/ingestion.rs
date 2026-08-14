//! Declarative ingestion specification: how to load source files into a
//! storage backend.
//!
//! An [`IngestionSpec`] is stored on the **target** `ResourceEntry` (which
//! must be `ResourceKind::Storage`). It describes the source path, format,
//! partition strategy, write mode, and optional archive lifecycle.
//!
//! The actual execution (reading files, writing to storage) is performed by
//! an ingestion executor in the runtime crate, which has access to the
//! catalog's storage backends. This keeps the resource-catalog crate free
//! of heavy execution dependencies.

use serde::{Deserialize, Serialize};

/// Declarative spec for ingesting source files into a storage backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestionSpec {
    /// Source file path or glob pattern (e.g. `"/data/chrom_*.parquet"`).
    pub source_path: String,
    /// Source file format.
    pub source_format: SourceFormat,
    /// Columns to partition the data by (directory partitioning).
    /// Empty = no partitioning.
    #[serde(default)]
    pub partition_by: Vec<String>,
    /// Write mode for the target.
    pub mode: WriteMode,
    /// If true, restore source files from archive before ingesting.
    #[serde(default)]
    pub restore_before: bool,
    /// If true, archive source files after successful ingest.
    #[serde(default)]
    pub archive_after: bool,
    /// Optional logical name of the source resource (for linked archive spec).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_resource: Option<String>,
    /// Optional CSV-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv_options: Option<CsvOptions>,
}

/// Source file format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    Parquet,
    Csv,
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

/// Write mode for the target storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteMode {
    /// Create if not exists; skip if already has data (idempotent).
    CreateIfNotExists,
    /// Drop and recreate, then insert all data (full refresh).
    CreateOrReplace,
    /// Append to existing data (must already exist).
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
    #[serde(default = "default_true")]
    pub has_header: bool,
    #[serde(default = "default_comma")]
    pub delimiter: char,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_extension: Option<String>,
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
