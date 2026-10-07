//! Live integration tests for the GWAS Catalog summary-statistics file download.
//!
//! These hit the real EBI FTP (HTTPS mirror). Run with:
//! ```sh
//! cargo test -p gwascatalog-sdk --test download_e2e -- --ignored
//! ```

use std::sync::Arc;

use gwascatalog_sdk::client::GwasCatalogClient;
use vfs::OpendalFileStorage;

#[tokio::test]
#[ignore]
async fn list_ftp_directory_for_known_accession() {
    let client = GwasCatalogClient::new();
    let block = client.ftp_block_url("GCST90000061").unwrap();
    let study_url = format!("{block}/GCST90000061");
    let entries = client.list_ftp_directory(&study_url).await.unwrap();
    // Root must contain at least one raw .tsv and the harmonised/ dir.
    assert!(
        entries.iter().any(|e| e.contains("buildGRCh")),
        "expected a raw build file, got: {entries:?}"
    );
    assert!(
        entries.iter().any(|e| e.starts_with("harmonised")),
        "expected a harmonised/ directory, got: {entries:?}"
    );
}

#[tokio::test]
#[ignore]
async fn list_harmonised_directory() {
    let client = GwasCatalogClient::new();
    let block = client.ftp_block_url("GCST90000061").unwrap();
    let harm_url = format!("{block}/GCST90000061/harmonised");
    let entries = client.list_ftp_directory(&harm_url).await.unwrap();
    assert!(
        entries.iter().any(|e| e.ends_with(".h.tsv.gz")),
        "expected a harmonised .h.tsv.gz file, got: {entries:?}"
    );
}

#[tokio::test]
#[ignore]
async fn download_harmonised_meta_yaml() {
    let client = GwasCatalogClient::new();
    let storage = Arc::new(OpendalFileStorage::new_temp());

    // Download just the small meta.yaml file by using the raw client method
    // directly (avoids pulling the full multi-MB .tsv.gz in CI).
    let block = client.ftp_block_url("GCST90000061").unwrap();
    let harm_url = format!("{block}/GCST90000061/harmonised");
    let entries = client.list_ftp_directory(&harm_url).await.unwrap();
    let meta = entries
        .iter()
        .find(|e| e.ends_with(".h.tsv.gz-meta.yaml"))
        .expect("meta.yaml not found");

    let url = format!("{harm_url}/{meta}");
    let file = client
        .download_stream_to_storage(&url, &storage, "/GCST90000061/meta.yaml", |_, _| {})
        .await
        .unwrap();

    assert!(file.bytes > 0, "downloaded file should be non-empty");
    assert_eq!(file.sha256.len(), 64, "sha256 must be reported");
}

#[tokio::test]
#[ignore]
async fn ftp_block_url_for_invalid_accession_errors() {
    let client = GwasCatalogClient::new();
    assert!(client.ftp_block_url("INVALID").is_err());
    assert!(client.ftp_block_url("GCST").is_err());
}
