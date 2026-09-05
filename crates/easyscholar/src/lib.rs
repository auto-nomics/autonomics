//! Async Rust SDK for the [EasyScholar](https://www.easyscholar.cc)
//! publication-rank API.
//!
//! EasyScholar resolves **journal-level** metrics by publication name:
//!
//! - JCR impact factor (2-year / 5-year)
//! - JCR and SSCI quartiles (`Q1`–`Q4`)
//! - CAS 分区（中科院大类 / 基础版 / 小类 / 顶刊 / 预警）
//!
//! The API requires a user-registered `secretKey` (read from
//! `EASYSCHOLAR_KEY` by [`EasyscholarClient::from_env`], hot-swappable via
//! [`EasyscholarClient::set_key`]). Without a key every call degrades to
//! "no data" instead of failing, so enrichment pipelines can invoke it
//! unconditionally.
//!
//! # Quick start
//!
//! ```no_run
//! # async fn run() -> easyscholar::error::Result<()> {
//! let client = easyscholar::EasyscholarClient::from_env();
//! match client.fetch_journal_rank("Nature Medicine").await? {
//!     Some(rank) => {
//!         println!("IF {:?}", rank.impact_factor);
//!         println!("JCR {:?}", rank.jcr_quartile);
//!     }
//!     None => println!("no rank data (unknown journal or key not configured)"),
//! }
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod error;
pub mod types;

pub use client::EasyscholarClient;
pub use error::{EasyScholarError, Result};
pub use types::JournalRank;
