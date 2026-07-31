//! Error types for the GWAS Catalog SDK.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GwasCatalogError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API returned error: {status} {message}")]
    Api { status: u16, message: String },
    /// Failed to parse an FTP HTML directory listing (e.g. EBI changed
    /// listing format or the accession does not exist).
    #[error("FTP directory listing parse failed for {url}: {reason}")]
    FtpListing { url: String, reason: String },
    /// The requested accession does not match the expected `GCSTnnnnnn`
    /// pattern, so the FTP block directory cannot be derived.
    #[error("invalid GWAS Catalog accession: {0}")]
    InvalidAccession(String),
    /// Storage (OpenDAL) I/O error while writing a downloaded file.
    #[error("storage error: {0}")]
    Storage(String),
}

pub type Result<T> = std::result::Result<T, GwasCatalogError>;
