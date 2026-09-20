//! Async Rust SDK for [NHANES](https://wwwn.cdc.gov/nchs/nhanes/) public data files.
//!
//! The CDC NHANES file listing and download services are public and require no
//! API key. This crate provides:
//!
//! - [`NhanesClient`] for component file listings and validated `.xpt` downloads.
//! - DAG source nodes under [`nodes`] (`source_nhanes_files`, `source_nhanes_download`).
//!
//! ```no_run
//! # use nhanes::NhanesClient;
//! # async fn run() -> nhanes::Result<()> {
//! let client = NhanesClient::new();
//! let html = client.listing_html("Laboratory", Some(2017)).await?;
//! let links = nhanes::parse_listing(&html);
//! let bytes = client.download_xpt(&links[0].href).await?;
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod error;
pub mod nodes;

pub use client::{NhanesClient, NhanesFileLink, is_soft_404, parse_listing};
pub use error::{NhanesError, Result};
