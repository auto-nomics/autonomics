//! HTTPS-specific helpers.
//!
//! The manager owns request execution so redirect targets cannot bypass its
//! validation loop. This module exposes metadata helpers shared by future
//! protocol adapters.
use crate::error::{DownloadError, Result};

pub fn content_length(response: &reqwest::Response) -> Option<u64> {
    response.content_length()
}

pub fn ensure_success(response: &reqwest::Response) -> Result<()> {
    response
        .error_for_status_ref()
        .map(|_| ())
        .map_err(|error| {
            DownloadError::request(
                response.url().as_str(),
                format!("HTTP {}", error.status().unwrap_or_default()),
            )
        })
}
