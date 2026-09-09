//! Async Rust SDK for the [STRING](https://string-db.org) protein
//! association API.
//!
//! The crate is intentionally free of DAG and agent-framework dependencies:
//! it provides typed HTTP operations and preview formatting that higher-level
//! DAG nodes and agent tools can wrap without inheriting HTTP details.
//!
//! Conventional STRING API calls require no API key. Only asynchronous
//! Values/Ranks enrichment jobs require the free anonymous key returned by
//! [`StringDbClient::get_api_key`].

pub mod client;
pub mod error;
pub mod format;
pub mod request;
pub mod types;

pub use client::{DEFAULT_STRING_ENDPOINT, Image, StringDbClient, StringDbClientBuilder};
pub use error::{Result, StringError};
pub use request::{
    AnnotationQuery, EnrichmentQuery, HomologyQuery, ImageFormat, InteractionPartnerQuery,
    NetworkFlavor, NetworkImageQuery, NetworkQuery, NetworkType, OutputFormat, StringIdQuery,
};
pub use types::*;
