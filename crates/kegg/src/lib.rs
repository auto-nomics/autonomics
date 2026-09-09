//! Async Rust SDK for the [KEGG REST API](https://www.kegg.jp/kegg/rest/).
//!
//! The crate separates the workspace's two integration surfaces. [`client`]
//! returns typed rows for structured DAG workflows; [`convert`] provides a
//! generic column/value table without taking an Arrow dependency. [`format`]
//! and [`tools`] provide bounded, human-readable previews for agent tools.
//!
//! # SDK example
//!
//! ```no_run
//! # async fn run() -> kegg::Result<()> {
//! let client = kegg::KeggClient::new();
//! let links = client.link("pathway", "hsa:10458").await?;
//! println!("{} pathways", links.len());
//! # Ok(())
//! # }
//! ```
//!
//! The client enforces KEGG's documented academic-use limit of three
//! requests per second within this process. Non-academic use requires a
//! KEGG license, and high-volume workflows should cache responses rather
//! than repeatedly fetching organism mappings.

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod parser;
pub mod rate;
pub mod tools;
pub mod types;

pub use client::{DEFAULT_KEGG_ENDPOINT, KEGG_RATE_LIMIT_PER_SECOND, KeggClient};
pub use convert::Table;
pub use error::{KeggError, Result};
pub use tools::kegg_registrations;
pub use types::{DrugInteraction, EntrySummary, FlatEntry, Info, Pair};
