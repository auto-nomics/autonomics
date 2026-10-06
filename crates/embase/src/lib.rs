//! Async Rust SDK for the [Elsevier Embase API](
//! https://nonprod-devportal.elsevier.com/embase_apis.html).
//!
//! This crate provides:
//!
//! - **`EmbaseClient`** — an async HTTP client for the Embase Search and
//!   Retrieval APIs (search by query, retrieve by DOI / PII / PMID / Embase
//!   accession number / LUI).
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! use embase::{EmbaseClient, types::SearchRequest};
//!
//! # async fn run() -> embase::error::Result<()> {
//! let client = EmbaseClient::from_env();
//! let resp = client
//!     .search(&SearchRequest::new("'heart attack':ti,ab AND aspirin:ti,ab"))
//!     .await?;
//! println!("{} results", resp.total_results);
//! for entry in &resp.entry {
//!     println!("  {} — {}", entry.doi.as_deref().unwrap_or(""), entry.title);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Environment variables
//!
//! | Variable          | Default          | Description                              |
//! |-------------------|------------------|------------------------------------------|
//! | `EMBASE_API_KEY`  | *(none)*         | Elsevier API key (required)              |
//! | `EMBASE_INSTTOKEN`| *(none)*         | Institution token (optional)             |
//! | `EMBASE_AUTHTOKEN`| *(none)*         | OAuth bearer token for user entitlements |

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod query;
pub mod types;

pub use client::EmbaseClient;
pub use convert::search_results_to_articles;
pub use error::EmbaseError;
