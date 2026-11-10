//! Domain-relative object store metadata and keys for DataFusion.
use datafusion::object_store::{path::Path, prefix::PrefixStore, *};
use futures::stream::BoxStream;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct DomainStore(pub PrefixStore<Arc<crate::OpendalFileStorage>>);

impl std::fmt::Display for DomainStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "domain:{}", self.0)
    }
}

#[async_trait::async_trait]
impl ObjectStore for DomainStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> Result<PutResult> {
        self.0.put_opts(location, payload, opts).await
    }
    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> Result<Box<dyn MultipartUpload>> {
        self.0.put_multipart_opts(location, opts).await
    }
    async fn get_opts(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        let mut result = self.0.get_opts(location, options).await?;
        // PrefixStore remaps listing metadata but preserves the underlying
        // location on get/head. DataFusion needs both in the same namespace.
        result.meta.location = location.clone();
        Ok(result)
    }
    fn delete_stream(
        &self,
        locations: BoxStream<'static, Result<Path>>,
    ) -> BoxStream<'static, Result<Path>> {
        self.0.delete_stream(locations)
    }
    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, Result<ObjectMeta>> {
        self.0.list(prefix)
    }
    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> Result<ListResult> {
        self.0.list_with_delimiter(prefix).await
    }
    async fn copy_opts(&self, from: &Path, to: &Path, options: CopyOptions) -> Result<()> {
        self.0.copy_opts(from, to, options).await
    }
}
