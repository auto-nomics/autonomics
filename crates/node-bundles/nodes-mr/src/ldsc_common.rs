//! Shared helpers for LDSC nodes.
//!
//! Nodes receive a runtime-resolved `DataBundle` and register its Parquet file
//! as a `ListingTable` on their isolated DataFusion session.

use std::sync::Arc;

use arrow_array::Float64Array;
use dag_core::node::DataBundle;
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::prelude::SessionContext;

/// Stable bundle IDs resolved by the runtime `BundleRegistry`.
pub const BUNDLE_LDSCORE_1000G_EUR: &str = "ldscore.1000g_eur";
pub const BUNDLE_LDSCORE_1000G_EUR_M: &str = "ldscore.1000g_eur_m";
pub const BUNDLE_LDSCORE_BASELINELD_V2_2_EUR: &str = "ldscore.baselineLD_v2_2_eur";
pub const BUNDLE_LDSCORE_BASELINELD_V2_2_EUR_M: &str = "ldscore.baselineLD_v2_2_eur_m";
pub const BUNDLE_LDSCORE_UKBB_EUR: &str = "ldscore.ukbb_eur";

/// Convert a catalog-resolved virtual path to the engine's DataFusion URL.
pub fn storage_url(bundle: &DataBundle) -> String {
    format!("vfs://{}", bundle.vpath)
}

/// Quote a DataFusion table name for SQL interpolation.
///
/// Panel table names commonly start with a digit (e.g. `1000g_eur`), so they
/// must be quoted in SQL.
pub fn quote_table(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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
    tracing::debug!(table = table_name, url, "registering LDSC panel");
    match ctx.table_exist(table_name) {
        Ok(true) => {
            tracing::debug!(table = table_name, "LDSC panel already registered");
            return Ok(());
        }
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
    tracing::debug!(table = table_name, url, "registered LDSC panel");
    Ok(())
}

/// Read per-annotation M_5_50 values from the companion `_m` table.
pub async fn read_m_5_50(
    ctx: &SessionContext,
    m_table_ref: &str,
    n_annot: usize,
) -> Result<Vec<f64>, LdscCommonError> {
    tracing::debug!(
        table = m_table_ref,
        expected_annotations = n_annot,
        "reading LDSC M table"
    );
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
    #[error("failed to read from the VFS: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("{0}")]
    InvalidInput(String),
}
