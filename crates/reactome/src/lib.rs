//! Async Rust SDK for the [Reactome Content Service](https://reactome.org/ContentService/)
//! and [Analysis Service](https://reactome.org/AnalysisService/).
//!
//! Reactome is a curated, open-source pathway database. This crate separates
//! the two integration surfaces used by this workspace:
//!
//! - [`client`] and [`types`] provide typed structured data for future DAG
//!   source and computation nodes; [`convert`] normalizes responses into
//!   column/value rows before they are promoted to Arrow/DataFrame nodes.
//! - [`format`] and [`tools`] provide concise previews suitable for agents.
//!
//! # SDK example
//!
//! ```no_run
//! # use reactome::ReactomeClient;
//! # async fn run() -> reactome::Result<()> {
//! let client = ReactomeClient::new();
//! let info = client.database_info().await?;
//! println!("Reactome {} v{}", info.name, info.version);
//!
//! let pathways = client.top_level_pathways("Homo sapiens").await?;
//! for p in pathways.iter().take(3) {
//!     println!("  {} {}", p.stable_id.as_deref().unwrap_or("?"), p.display_name);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Pathway over-representation analysis
//!
//! ```no_run
//! # use reactome::ReactomeClient;
//! # async fn run() -> reactome::Result<()> {
//! let client = ReactomeClient::new();
//! let result = client
//!     .analyse_identifiers(
//!         &["TP53".to_string(), "BRCA1".to_string(), "EGFR".to_string()],
//!         true,
//!     )
//!     .await?;
//! for p in result.pathways.iter().take(5) {
//!     println!("  {}  p={:e}  FDR={:e}",
//!         p.name,
//!         p.entities.p_value.unwrap_or(1.0),
//!         p.entities.fdr.unwrap_or(1.0),
//!     );
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Reactome is free without authentication (no API key required). Both
//! services sit behind Cloudflare and accept anonymous requests; the client
//! sets a stable user agent and reuses one connection pool.

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod nodes;
pub mod tools;
pub mod types;

pub use client::{DEFAULT_ANALYSIS_ENDPOINT, DEFAULT_CONTENT_ENDPOINT, ReactomeClient};
pub use convert::Table;
pub use error::{ReactomeError, Result};
pub use tools::reactome_registrations;
