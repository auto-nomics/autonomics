//! Error types for the GWAS Catalog SDK.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GwasCatalogError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API returned error: {status} {message}")]
    Api { status: u16, message: String },
}

pub type Result<T> = std::result::Result<T, GwasCatalogError>;
