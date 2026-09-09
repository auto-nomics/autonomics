//! JSON input support backed by DataFusion's native JSON reader.
//!
//! DataFusion distinguishes JSON arrays from newline-delimited JSON at the
//! read-options level. The helper below reads a small prefix from the first
//! object so callers do not have to know which representation a `.json` file
//! uses.

use std::io::Read;
use std::sync::Arc;

use bytes::Bytes;
use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
use datafusion::datasource::listing::ListingTableUrl;
use datafusion::error::{DataFusionError, Result};
use datafusion::execution::context::DataFilePaths;
use datafusion::execution::options::JsonReadOptions;
use datafusion::object_store::ObjectMeta;
use datafusion::object_store::{ObjectStore, ObjectStoreExt};
use datafusion::prelude::{DataFrame, SessionContext};
use futures::StreamExt;

use crate::datasource::BioReadOptions;
use crate::datasource::core::infer_file_extension;

/// Read JSON objects as a [`DataFrame`].
///
/// Both JSON arrays and newline-delimited JSON are accepted. Gzip-compressed
/// inputs are detected from the object's magic bytes. The options are kept in
/// biofusion's shared builder style; `batch_size` is controlled by the session
/// while `file_extension`, `columns`, and `limit` are honored here.
pub async fn read_json<P>(
    ctx: &SessionContext,
    table_paths: P,
    options: BioReadOptions,
) -> Result<DataFrame>
where
    P: DataFilePaths + Send,
{
    let urls = table_paths.to_urls()?;
    let url = urls
        .first()
        .ok_or_else(|| DataFusionError::Plan("read_json: no table path provided".to_string()))?;

    let extension = json_listing_extension(url, options.file_extension());
    let prefix = read_json_prefix(ctx, url).await?;
    let compression = if prefix.first() == Some(&0x1f) && prefix.get(1) == Some(&0x8b) {
        FileCompressionType::GZIP
    } else {
        FileCompressionType::UNCOMPRESSED
    };
    let decoded_prefix = decode_prefix(prefix.as_ref(), compression)?;
    let newline_delimited = first_json_byte(&decoded_prefix) != Some(b'[');

    let json_options = JsonReadOptions::default()
        .file_extension(&extension)
        .file_compression_type(compression)
        .newline_delimited(newline_delimited);
    let mut df = ctx.read_json(urls, json_options).await?;

    if let Some(columns) = options.columns() {
        if !columns.is_empty() {
            let columns: Vec<&str> = columns.iter().map(String::as_str).collect();
            df = df.select_columns(&columns)?;
        }
    }
    if let Some(limit) = options.limit() {
        df = df.limit(0, Some(limit))?;
    }
    Ok(df)
}

fn json_listing_extension(url: &ListingTableUrl, explicit: Option<&str>) -> String {
    let extension = infer_file_extension(url, explicit, "json");
    if extension.starts_with('.') {
        extension
    } else {
        format!(".{extension}")
    }
}

async fn read_json_prefix(ctx: &SessionContext, url: &ListingTableUrl) -> Result<Bytes> {
    let store: Arc<dyn ObjectStore> = ctx.runtime_env().object_store(url)?;
    let meta = match store.head(url.prefix()).await {
        Ok(meta) => meta,
        Err(head_error) => {
            let mut objects = store.list(Some(url.prefix()));
            let Some(first) = objects.next().await else {
                return Err(DataFusionError::External(Box::new(head_error)));
            };
            first?
        }
    };
    let length = meta.size.min(256) as u64;
    if length == 0 {
        return Ok(Bytes::new());
    }
    Ok(store.get_range(url.prefix(), 0..length).await?)
}

fn decode_prefix(bytes: &[u8], compression: FileCompressionType) -> Result<Vec<u8>> {
    if matches!(compression, FileCompressionType::GZIP) {
        let mut decoded = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes)
            .take(16)
            .read_to_end(&mut decoded)
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        Ok(decoded)
    } else {
        Ok(bytes.to_vec())
    }
}

fn first_json_byte(bytes: &[u8]) -> Option<u8> {
    bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
}
