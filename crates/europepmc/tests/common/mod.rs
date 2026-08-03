//! Shared helpers for Europe PMC integration tests.

use europepmc::EuropePmcClient;

/// Create a client for testing.
pub fn client() -> EuropePmcClient {
    EuropePmcClient::new()
}
