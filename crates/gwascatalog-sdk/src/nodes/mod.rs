//! DAG source nodes for the GWAS Catalog (`source_gwascatalog_*`).
//!
//! The tool layer ([`crate::tools`]) answers interactive, human-oriented
//! queries; these nodes are the dataflow half — the same three EBI APIs
//! (Solr search, curated REST, summary statistics) emitting DataFrames that
//! feed analysis pipelines, plus a FileSet-emitting bulk download node.
//! Kinds:
//!
//! - `search`: `source_gwascatalog_search` — Solr full-text search
//! - `studies`: `source_gwascatalog_studies` — curated studies
//! - `associations`: `source_gwascatalog_associations` /
//!   `source_gwascatalog_study_associations` — curated SNP-trait associations
//! - `snps`: `source_gwascatalog_snps` — SNP annotations
//! - `efo_traits`: `source_gwascatalog_efo_traits` — EFO trait ontology
//! - `unpublished`: `source_gwascatalog_unpublished_studies`
//! - `summary_associations`: `source_gwascatalog_summary_associations` —
//!   harmonised per-variant summary statistics
//! - `download`: `source_gwascatalog_download` — full summary-statistics
//!   files as a FileSet of VFS objects

pub mod associations;
pub mod download;
pub mod efo_traits;
pub mod search;
pub mod snps;
pub mod studies;
pub mod summary_associations;
pub mod unpublished;

use std::sync::Arc;

use arrow_array::Array;
use arrow_schema::DataType;
use dag_core::dag::{DagError, NodePorts};
use dag_core::registry::NodeCtx;
use dag_core::value::{FileFingerprint, FileRef, PortType};
use sha2::{Digest, Sha256};

/// Shared node plumbing. Kept `pub(crate)` so each node file stays focused
/// on its spec, factory, and Arrow schema.
pub(crate) mod util {
    use super::*;

    /// One DataFrame output port (the layout of every table-emitting kind).
    pub fn df_port() -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    /// One FileSet output port (bulk download).
    pub fn file_set_port() -> NodePorts {
        NodePorts::new().add_output_port_of_type(None, PortType::FileSet)
    }

    /// Normalize an output path to its canonical `vfs://` form: bare
    /// absolute paths (and legacy `file://`) are re-prefixed so the engine
    /// and the agent share one mounted object-store namespace. Relative
    /// paths fall through unchanged so the caller's error stays informative.
    pub fn to_vfs_uri(path: &str) -> String {
        if let Some(stripped) = path.strip_prefix("vfs://") {
            format!("vfs://{stripped}")
        } else if let Some(stripped) = path.strip_prefix("file://") {
            format!("vfs://{stripped}")
        } else if std::path::Path::new(path).is_absolute() {
            format!("vfs://{path}")
        } else {
            path.to_string()
        }
    }

    /// Validate a node output path: `vfs://` URI, `file://` URI, or bare
    /// absolute host path (auto-routed through the mounted VFS).
    pub fn validate_output_path(path: &str) -> Result<(), DagError> {
        if path.starts_with("vfs://") || path.starts_with("file://") {
            return Ok(());
        }
        if !std::path::Path::new(path).is_absolute() {
            return Err(DagError::Schedule(format!(
                "output path must be a `vfs://` URI or an absolute path (auto-routed \
                 through the mounted runtime VFS so the agent and engine share one \
                 object-store namespace); got `{path}`"
            )));
        }
        Ok(())
    }
}

/// Build a nullable UTF-8 Arrow column from optional strings.
pub(crate) fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    let refs: Vec<Option<&str>> = rows.iter().map(|o| o.as_deref()).collect();
    Arc::new(arrow_array::StringArray::from(refs))
}

/// Join string slices with `"; "` into an Option (None when empty), the
/// convention for repeated API fields in table outputs.
pub(crate) fn joined(items: &[String]) -> Option<String> {
    if items.is_empty() {
        None
    } else {
        Some(items.join("; "))
    }
}

/// UTF-8 field for Arrow schemas (every API column is nullable).
pub(crate) fn utf8(name: &str) -> arrow_schema::Field {
    arrow_schema::Field::new(name, DataType::Utf8, true)
}
