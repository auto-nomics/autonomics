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

/// Whether to run live API tests. Defaults to false so the workspace test
/// suite remains deterministic; set `OPENALEX_RUN_LIVE=1` to include them.
pub fn run_live() -> bool {
    if std::env::var("OPENALEX_SKIP_LIVE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return false;
    }
    std::env::var("OPENALEX_RUN_LIVE")
        .map(|v| v == "1")
        .unwrap_or(false)
}
