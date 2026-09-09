//! Async Rust SDK for the [ChEMBL REST API](
//! https://www.ebi.ac.uk/chembl/api/data/docs).
//!
//! The crate has two layers:
//!
//! - **Typed client** - [`ChEMBLClient`] covers common molecule, target,
//!   assay, activity, document, mechanism, and indication queries, plus a
//!   generic resource API for endpoints that do not yet have named methods.
//!   [`ResourceQuery`] supports server-side filters, ordering, field
//!   projection, and offset pagination; [`list_all`](ChEMBLClient::list_all)
//!   walks pages for structured-data workflows.
//! - **Agent tools** - [`chembl_registrations`] exposes compact previews and
//!   summaries for LLM tool callers, without returning full record dumps.
//!
//! # SDK example
//!
//! ```no_run
//! # use chembl::{ChEMBLClient, ResourceQuery};
//! # #[tokio::main]
//! # async fn main() -> chembl::Result<()> {
//! let client = ChEMBLClient::new();
//! let query = ResourceQuery::new().limit(100);
//! let activities = client.activities_for_molecule("CHEMBL25", &query).await?;
//! println!("fetched {} activities", activities.records.len());
//! # Ok(())
//! # }
//! ```
//!
//! # Tool example
//!
//! ```no_run
//! # use chembl::{ChEMBLClient, chembl_registrations};
//! # use std::sync::Arc;
//! let client = Arc::new(ChEMBLClient::new());
//! let tools = chembl_registrations(client);
//! ```

pub mod client;
pub mod error;
pub mod format;
pub mod query;
pub mod tools;
pub mod types;

pub use client::{ChEMBLClient, DEFAULT_ENDPOINT, endpoint};
pub use error::{ChemblError, Result};
pub use query::ResourceQuery;
pub use tools::chembl_registrations;
pub use types::{
    Activity, Assay, CrossReference, Document, DrugIndication, Mechanism, Molecule,
    MoleculeProperties, MoleculeStructure, MoleculeSynonym, Page, PageMeta, Reference, Status,
    Target, TargetComponent,
};
