//! GWAS Catalog SDK — async client for all three EBI GWAS Catalog APIs.
//!
//! - **Summary Statistics API** ([`summary_stats`]) — per-variant harmonised stats
//! - **REST API** ([`rest`]) — curated catalog metadata (studies, associations, traits, SNPs)
//! - **Search API** ([`search`]) — Solr full-text search across all resources
//!
//! All endpoints are read-only (`GET`), unauthenticated.
//!
//! # Example
//!
//! ```ignore
//! use gwascatalog_sdk::{GwasCatalogClient, search::SearchFilter};
//!
//! let client = GwasCatalogClient::new();
//! let resp = client.search(&SearchFilter {
//!     q: "resourcename:study AND \"breast cancer\"".into(),
//!     max: Some(5),
//!     ..Default::default()
//! }).await?;
//! ```

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod nodes;
pub mod rest;
pub mod search;
pub mod summary_stats;
pub mod tools;

// Back-compat: expose old module path `gwas_catalog` for tests that reference
// `gwascatalog_sdk::gwas_catalog::GwasCatalogError`.
pub mod gwas_catalog {
    pub use crate::error::{GwasCatalogError, Result};
}

pub use client::{GwasCatalogApi, GwasCatalogClient};
pub use error::{GwasCatalogError, Result};
pub use rest::{EfoTrait, RestAssociation, RestPage, RestStudy, Snp, UnpublishedStudy};
pub use search::{SearchDoc, SearchFilter, SearchResponse};
pub use summary_stats::{
    AssociationQuery, ChromosomeAssociationQuery, PaginatedResponse, PaginationQuery, RevealMode,
};
pub use tools::gwascatalog_registrations;

// DAG source-node surface (nodes/).
pub use nodes::associations::{AssociationsNodeFactory, StudyAssociationsNodeFactory};
pub use nodes::download::DownloadNodeFactory;
pub use nodes::efo_traits::EfoTraitsNodeFactory;
pub use nodes::search::SearchNodeFactory;
pub use nodes::snps::SnpsNodeFactory;
pub use nodes::studies::StudiesNodeFactory;
pub use nodes::summary_associations::SummaryAssociationsNodeFactory;
pub use nodes::unpublished::UnpublishedNodeFactory;

use dag_core::registry::NodeRegistry;

/// Register every `source_gwascatalog_*` node factory into a registry.
/// Called by the io bundle's plugin; kept here so the SDK owns its own
/// node surface the same way crossref / rcsb / uniprot / reactome do.
pub fn register_nodes(registry: &mut NodeRegistry) {
    registry.register(Box::new(nodes::search::SearchNodeFactory));
    registry.register(Box::new(nodes::studies::StudiesNodeFactory));
    registry.register(Box::new(nodes::associations::AssociationsNodeFactory));
    registry.register(Box::new(nodes::associations::StudyAssociationsNodeFactory));
    registry.register(Box::new(nodes::snps::SnpsNodeFactory));
    registry.register(Box::new(nodes::efo_traits::EfoTraitsNodeFactory));
    registry.register(Box::new(nodes::unpublished::UnpublishedNodeFactory));
    registry.register(Box::new(
        nodes::summary_associations::SummaryAssociationsNodeFactory,
    ));
    registry.register(Box::new(nodes::download::DownloadNodeFactory));
}
