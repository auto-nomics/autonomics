//! Shared helpers for the live-API integration tests.

use uniprot::UniProtClient;

pub fn client() -> UniProtClient {
    UniProtClient::new()
}
