#![allow(dead_code)]
//! Shared test helpers for arXiv E2E tests.
//!
//! arXiv recommends a 3-second delay between consecutive API calls. The
//! client enforces this automatically, but we also use `#[serial]` to
//! prevent tests within the same binary from interleaving.

use std::net::TcpListener;

/// Create a test client.
pub fn test_client() -> arxiv::ArxivClient {
    arxiv::ArxivClient::new()
}

/// Fixed localhost port used as a cross-process mutex (same pattern as eutils).
const RATE_LOCK_ADDR: &str = "127.0.0.1:38438";

/// Wait between API calls to respect arXiv's rate limit, coordinating
/// across all concurrently-running test binaries via a shared TCP-port lock.
pub fn rate_limit() {
    let listener = loop {
        match TcpListener::bind(RATE_LOCK_ADDR) {
            Ok(l) => break l,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(40)),
        }
    };
    std::thread::sleep(std::time::Duration::from_millis(500));
    drop(listener);
}

/// Re-export the serial macro for convenience.
pub use serial_test::serial;

/// Well-known arXiv IDs used across tests (unlikely to disappear).
pub const ARXID_ATTENTION: &str = "1706.03762"; // "Attention Is All You Need"
