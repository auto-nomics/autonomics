//! Async Rust SDK for the [Open Targets Platform GraphQL API](
//! https://platform.opentargets.org/api).
//!
//! The API is public (no authentication). Create an [`OpenTargetsClient`]
//! and call the typed helpers, or use [`OpenTargetsClient::query`] for
//! arbitrary GraphQL.
//!
//! # Example
//!
//! ```no_run
//! # use opentargets::OpenTargetsClient;
//! # #[tokio::main] async fn main() -> opentargets::Result<()> {
//! let client = OpenTargetsClient::new();
//! let brca1 = client.target("ENSG00000012048").await?.unwrap();
//! println!("{} — {}", brca1.approved_symbol, brca1.approved_name);
//!
//! for assoc in client.associated_diseases_all("ENSG00000012048").await? {
//!     println!("{:.3}\t{}\t{}", assoc.score, assoc.disease.id, assoc.disease.name);
//! }
//! # Ok(()) }
//! ```

pub mod associations;
pub mod client;
pub mod error;
pub mod search;
pub mod types;

pub use associations::{
    AssociationPage, AssociatedDisease, AssociatedTarget, Pagination,
};
pub use client::{OpenTargetsClient, DEFAULT_ENDPOINT};
pub use error::{OpenTargetsError, Result};
pub use search::{SearchResult, SearchResults};
pub use types::{
    ApiVersion, DataVersion, Disease, Drug, GenomicLocation, Meta, Sample,
    ScoredComponent, Study, Target, Variant,
};
