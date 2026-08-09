//! Shared helpers for OpenAlex integration tests.

use openalex::OpenAlexClient;

/// The OpenAlex API key used by integration tests.
///
/// Set via the `OPENALEX_API_KEY` environment variable, or falls back to
/// the provided key.
pub fn api_key() -> Option<String> {
    std::env::var("OPENALEX_API_KEY").ok()
}

/// Create a client configured with the test API key (if available).
pub fn client() -> OpenAlexClient {
    OpenAlexClient::new(api_key().as_deref())
}

/// Whether to run live API tests. Defaults to true.
pub fn run_live() -> bool {
    std::env::var("OPENALEX_SKIP_LIVE").map(|v| v != "1").unwrap_or(true)
}
