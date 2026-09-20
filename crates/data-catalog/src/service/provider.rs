use async_trait::async_trait;

use crate::model::{CatalogEntry, DatasetFile};

use super::model::{
    CatalogDataset, CatalogRecord, CatalogSearchQuery, CatalogSnapshot,
};

/// Query and refresh behavior for a process-level catalog view.
#[async_trait]
pub trait CatalogServiceTrait: Send + Sync {
    /// Reload the catalog index and current manifests from the backend.
    async fn refresh(&self) -> Result<CatalogSnapshot, String>;

    /// Return the cached current-entry snapshot.
    async fn snapshot(&self) -> CatalogSnapshot;

    /// Search current datasets by free text, kind, and tags.
    async fn search(&self, query: CatalogSearchQuery) -> Result<Vec<CatalogRecord>, String>;

    /// Describe a current or historical entry.
    ///
    /// Version and digest are both optional. When both are omitted, the
    /// current entry is returned.
    async fn describe(
        &self,
        id: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<CatalogDataset, String>;

    /// List a dataset's payload files with sizes and checksums.
    async fn list_files(
        &self,
        id: &str,
        version: Option<&str>,
        digest: Option<&str>,
    ) -> Result<Vec<DatasetFile>, String>;

    /// List all known versions for a dataset id, newest first.
    async fn list_versions(&self, id: &str) -> Result<Vec<CatalogEntry>, String>;
}
