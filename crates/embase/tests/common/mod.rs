#![allow(dead_code)]
//! Shared test helpers for Embase E2E tests.
//!
//! All E2E tests are `#[ignore]` because they require a valid `EMBASE_API_KEY`.
//! Run with: `cargo test -p embase -- --ignored`

/// Create a test client from the `EMBASE_API_KEY` environment variable.
pub fn test_client() -> embase::EmbaseClient {
    embase::EmbaseClient::from_env()
}

/// Re-export the serial macro for convenience.
pub use serial_test::serial;
