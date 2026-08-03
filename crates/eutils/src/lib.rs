//! Async Rust SDK for the [NCBI Entrez Programming Utilities (E-utilities)](
//! https://www.ncbi.nlm.nih.gov/books/NBK25497/).
//!
//! This crate provides:
//!
//! - **`EutilsClient`** — an async HTTP client for all nine E-utilities
//!   (EInfo, ESearch, EPost, ESummary, EFetch, ELink, EGQuery, ESpell,
//!   ECitMatch).
//! - **Agent tools** — pre-built [`ToolFunction`] implementations that expose
//!   PubMed search, fetch, summary, related-articles, spell-check, and
//!   cross-database query capabilities to an agentik agent.
//!
//! # Quick start (SDK only)
//!
//! ```no_run
//! use eutils::{EutilsClient, types::ESearchRequest};
//!
//! # async fn run() -> eutils::error::Result<()> {
//! let client = EutilsClient::new("my-tool", "dev@example.com", None);
//! let resp = client
//!     .esearch(&ESearchRequest::new("pubmed", "CRISPR[Title]"))
//!     .await?;
//! println!("{} results", resp.result.count);
//! # Ok(())
//! # }
//! ```
//!
//! # Wiring tools into an agent
//!
//! ```no_run,ignore
//! use eutils::{EutilsClient, eutils_registrations};
//! use std::sync::Arc;
//!
//! let client = Arc::new(EutilsClient::from_env());
//! let tools = eutils_registrations(client);
//! // pass `tools` to Agent::builder().with_tools(tools)
//! ```
//!
//! # Environment variables
//!
//! | Variable         | Default          | Description                        |
//! |-----------------|------------------|------------------------------------|
//! | `EUTILS_TOOL`   | `"agentik"`      | Software identifier for NCBI       |
//! | `EUTILS_EMAIL`  | `"agentik@localhost"` | Contact e-mail for NCBI        |
//! | `EUTILS_API_KEY`| *(none)*         | API key for elevated rate limits   |

pub mod client;
pub mod convert;
pub mod error;
pub mod format;
pub mod query;
pub mod tools;
pub mod types;

pub use client::EutilsClient;
pub use convert::{esummary_to_articles, medline_to_articles};
pub use error::EutilsError;
pub use tools::eutils_registrations;
