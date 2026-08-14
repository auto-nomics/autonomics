//! Shared helpers for LDSC nodes.
//!
//! The heavy per-panel ref abstraction is gone. Consumers resolve a catalog
//! resource by logical name, get a DataFusion listing URL, and register the
//! parquet files as a `ListingTable` on their own session context.

use std::sync::Arc;

use arrow_array::Float64Array;
use dag_core::resource_catalog::ResourceCatalog;
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::prelude::SessionContext;

/// Resolve a PLINK reference prefix template from the catalog, falling back
/// to `fallback` when the logical name is not registered.
pub fn resolve_ref_prefix(catalog: &ResourceCatalog, fallback: &str) -> String {
    match catalog.resolve_storage_path_raw("plink.1000g_eur.ref_prefix") {
        Ok(p) => p,
        Err(_) => fallback.to_string(),
    }
}

/// Quote a DataFusion table name for SQL interpolation.
///
/// Panel table names commonly start with a digit (e.g. `1000g_eur`), so they
/// must be quoted in SQL.
pub fn quote_table(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Register one catalog storage resource as a DataFusion parquet
/// `ListingTable` named `table_name`.
///
/// The catalog resolves the backend and path into a plain URL string; this
/// helper only bridges that string into the active `SessionContext`.
pub async fn register_catalog_table(
    ctx: &SessionContext,
    catalog: &ResourceCatalog,
    logical: &str,
    table_name: &str,
) -> Result<(), LdscCommonError> {
    let url = catalog.resolve_storage_url(logical)?;
    register_listing_table(ctx, table_name, &url).await
}

/// Register a DataFusion `ListingTable` for a resolved storage URL.
///
/// The engine must have registered a DataFusion `ObjectStore` on the shared
/// `runtime_env` before this is called. Re-registration is a no-op.
pub async fn register_listing_table(
    ctx: &SessionContext,
    table_name: &str,
    url: &str,
) -> Result<(), LdscCommonError> {
    match ctx.table_exist(table_name) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => return Err(LdscCommonError::ReadBatch(e)),
    }

    let table_url = ListingTableUrl::parse(url).map_err(|e| {
        LdscCommonError::InvalidInput(format!("failed to parse storage url '{url}': {e}"))
    })?;

    let file_format = Arc::new(ParquetFormat::default());
    let listing_options = ListingOptions::new(file_format).with_collect_stat(false);
    let state = ctx.state();
    let resolved_schema = listing_options
        .infer_schema(&state, &table_url)
        .await
        .map_err(LdscCommonError::ReadBatch)?;

    let config = ListingTableConfig::new(table_url)
        .with_listing_options(listing_options)
        .with_schema(resolved_schema);
    let provider: Arc<dyn TableProvider> = Arc::new(ListingTable::try_new(config)?);

    ctx.register_table(table_name, provider)
        .map_err(LdscCommonError::ReadBatch)?;
    Ok(())
}

/// Read per-annotation M_5_50 values from the companion `_m` table.
pub async fn read_m_5_50(
    ctx: &SessionContext,
    m_table_ref: &str,
    n_annot: usize,
) -> Result<Vec<f64>, LdscCommonError> {
    let sql = format!(r#"SELECT "m_5_50" FROM {}"#, quote_table(m_table_ref));
    let df = ctx.sql(&sql).await.map_err(LdscCommonError::ReadBatch)?;
    let batches = df.collect().await.map_err(LdscCommonError::ReadBatch)?;

    let mut m_values = Vec::new();
    for batch in &batches {
        let col = batch
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| {
                LdscCommonError::InvalidInput("M table 'm_5_50' column is not Float64".into())
            })?;
        for i in 0..batch.num_rows() {
            m_values.push(col.value(i));
        }
    }

    if m_values.len() != n_annot {
        return Err(LdscCommonError::InvalidInput(format!(
            "M table '{m_table_ref}' has {} rows but expected {n_annot} annotation(s)",
            m_values.len()
        )));
    }

    Ok(m_values)
}

/// Error type for shared LDSC helpers.
#[derive(Debug, thiserror::Error)]
pub enum LdscCommonError {
    #[error("failed to read from data lake: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("resource catalog error: {0}")]
    Catalog(#[from] dag_core::resource_catalog::ResourceError),
    #[error("{0}")]
    InvalidInput(String),
}
