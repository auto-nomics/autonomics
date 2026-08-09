//! Shared helpers for Semantic Scholar integration tests.

use semantic_scholar::S2Client;

/// Create a client for testing.
///
/// If the `S2_API_KEY` environment variable is set, it is passed to the
/// client for higher rate limits. Otherwise the shared unauthenticated
/// pool is used.
pub fn client() -> S2Client {
    match std::env::var("S2_API_KEY") {
        Ok(key) if !key.is_empty() => S2Client::new().with_api_key(key),
        _ => S2Client::new(),
    }
}
