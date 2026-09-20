use thiserror::Error;

/// Errors returned by the NHANES SDK.
#[derive(Debug, Error)]
pub enum NhanesError {
    #[error("NHANES request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error(
        "NHANES download failed validation for {url}: HTTP {status}, content-type {content_type:?}; \
         expected a non-HTML XPORT file starting with \"HEADER RECORD*******\"; first bytes: \
         {prefix:?}. If this looks like an HTML error page, re-run source_nhanes_files to \
         refresh the href."
    )]
    InvalidDownload {
        url: String,
        status: u16,
        content_type: String,
        prefix: String,
    },
}

/// Result alias for NHANES operations.
pub type Result<T> = std::result::Result<T, NhanesError>;
