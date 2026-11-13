//! DAG source nodes for the GWAS Catalog (`source_gwascatalog_*`).
//!
//! The tool layer ([`crate::tools`]) answers interactive, human-oriented
//! queries; these nodes are the dataflow half — the same three EBI APIs
//! (Solr search, curated REST, summary statistics) emitting DataFrames that
//! feed analysis pipelines.
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

pub mod associations;
pub mod efo_traits;
pub mod search;
pub mod snps;
pub mod studies;
pub mod summary_associations;
pub mod unpublished;

use std::sync::Arc;

use arrow_array::Array;
use arrow_schema::DataType;

/// Shared node plumbing. Kept `pub(crate)` so each node file stays focused
/// on its spec, factory, and Arrow schema.
pub(crate) mod util {
    use dag_core::dag::NodePorts;

    /// One DataFrame output port (the layout of every table-emitting kind).
    pub fn df_port() -> NodePorts {
        NodePorts::new().add_output_port(None)
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
