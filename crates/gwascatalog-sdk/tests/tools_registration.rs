//! Offline guard for the GWAS Catalog agent-tool migration boundary.

use std::sync::Arc;

use gwascatalog_sdk::{GwasCatalogClient, gwascatalog_registrations};
use vfs::OpendalFileStorage;

#[test]
fn registrations_exclude_migrated_search() {
    let tools = gwascatalog_registrations(
        Arc::new(GwasCatalogClient::new()),
        Arc::new(OpendalFileStorage::new_temp()),
    );
    assert_eq!(tools.len(), 1, "only download should remain registered");
    assert_eq!(
        tools[0].definition.name,
        "gwascatalog_download_summary_stats"
    );
    assert!(
        !tools
            .iter()
            .any(|tool| tool.definition.name == "gwascatalog_search"),
        "gwascatalog_search must not be registered; use source_gwascatalog_search"
    );
}
