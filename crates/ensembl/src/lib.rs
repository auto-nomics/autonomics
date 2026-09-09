//! Async Rust SDK for the [Ensembl REST API](https://rest.ensembl.org).
//!
//! The crate separates the two integration surfaces used by this workspace.
//! [`client`] and [`types`] provide typed structured data for future DAG
//! source and computation nodes; [`convert`] normalizes responses into
//! column/value rows before they are promoted to Arrow/DataFrame nodes.
//! [`format`] and [`tools`] provide concise previews suitable for agents.
//!
//! # SDK example
//!
//! ```no_run
//! # use ensembl::EnsemblClient;
//! # async fn run() -> ensembl::Result<()> {
//! let client = EnsemblClient::new();
//! let brca2 = client.lookup_id("ENSG00000139618", false).await?;
//! println!("{} {}:{}:{}-{}",
//!     brca2.display_name.as_deref().unwrap_or("-"),
//!     brca2.coordinates.assembly_name.as_deref().unwrap_or("-"),
//!     brca2.coordinates.seq_region_name,
//!     brca2.coordinates.start,
//!     brca2.coordinates.end);
//! # Ok(())
//! # }
//! ```
//!
//! Ensembl is free without authentication. The client sets a stable user
//! agent and reuses one HTTP connection pool. Production agents should still
//! respect Ensembl's service rate limits and use batch endpoints when more
//! than a few identifiers are needed.

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod sequence;
pub mod tools;
pub mod types;

pub use client::{DEFAULT_ENSEMBL_ENDPOINT, EnsemblClient};
pub use error::{EnsemblError, Result};
pub use sequence::{SequenceType, nucleotide_stats};
pub use tools::ensembl_registrations;
