//! Async Rust SDK for the [Enrichr](https://maayanlab.cloud/Enrichr/help#api)
//! gene-set enrichment API and its Speedrichr background-corrected companion.
//!
//! The crate is intentionally free of DAG and agent-framework entanglement in
//! its client layer: [`client`] and [`types`] provide typed HTTP operations
//! that higher-level DAG nodes ([`nodes`]) and agent tools ([`tools`]) wrap
//! without inheriting HTTP details, mirroring `string-sdk`.
//!
//! Enrichr is free and anonymous — no API key. The client spaces requests by
//! one second to honor Enrichr's fair-use guidance.
//!
//! # Example
//!
//! ```no_run
//! # use enrichr_sdk::EnrichrClient;
//! # async fn run() -> enrichr_sdk::Result<()> {
//! let client = EnrichrClient::new();
//!
//! // Submit a gene list once, reuse the ID across libraries.
//! let added = client
//!     .add_list(["TP53", "BRCA1", "EGFR", "MYC", "PTEN"], "oncogene probe")
//!     .await?;
//! let kegg = client.enrich(added.user_list_id, "KEGG_2021_Human").await?;
//! for term in kegg.terms.iter().take(3) {
//!     println!("{}  adj-p={:e}", term.term, term.adjusted_p_value);
//! }
//!
//! // Background-corrected enrichment through Speedrichr.
//! let background = client
//!     .speedrichr_add_background(["TP53", "BRCA1", "EGFR", "MYC", "PTEN", "AKT1", "KRAS"])
//!     .await?;
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod error;
pub mod format;
pub mod nodes;
pub mod request;
pub mod tools;
pub mod types;

pub use client::{
    DEFAULT_ENRICHR_ENDPOINT, DEFAULT_SPEEDRICHR_ENDPOINT, EnrichrClient, EnrichrClientBuilder,
};
pub use error::{EnrichrError, Result};
pub use request::EnrichrHost;
pub use tools::enrichr_registrations;
pub use types::*;
