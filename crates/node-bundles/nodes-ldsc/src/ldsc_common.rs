//! Shared helpers for LDSC nodes.
//!
//! Provides:
//! - [`LdScoreRef`] — catalog-aware resolution of an LD-score panel,
//!   transparently supporting both legacy Iceberg tables and the
//!   post-migration object-storage (`ListingTable`) path.
//! - [`LdMatrixRef`] — catalog-aware resolution of per-chromosome LD-matrix
//!   tables, same dual-path support.
//! - [`read_m_5_50`] — reading per-annotation M_5_50 values from companion
//!   `_m` tables.
//! - [`register_listing_table`] — bridge from a catalog
//!   [`ObjectStorageHandle`](dag_core::resource_catalog::ObjectStorageHandle)
//!   into a DataFusion `ListingTable` registered against the active
//!   `SessionContext`.

use std::sync::Arc;

use arrow_array::Float64Array;
use dag_core::resource_catalog::{
    IcebergIdent, ObjectStorageBackend, ObjectStorageHandle, ResourceCatalog,
};
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::prelude::SessionContext;

// ── Catalog-aware table references ────────────────────────────────────────

/// Resolve a PLINK reference prefix template from the catalog, falling back
/// to `fallback` when the logical name is not registered.
///
/// Returns the **raw template** (with `{N}` placeholder intact) so that
/// downstream tools (e.g. `magma::plink::BedFile::open_template`,
/// `lava::plink::load_reference_template`) can substitute chromosomes
/// themselves.
pub fn resolve_ref_prefix(catalog: &ResourceCatalog, fallback: &str) -> String {
    match catalog.resolve_path_raw("plink.1000g_eur.ref_prefix") {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => fallback.to_string(),
    }
}

/// A resolved LD-score table reference.
///
/// `sql` is the **legacy** Iceberg fully-qualified identifier
/// (`"iceberg"."ld_score"."1000g_eur"`) and is kept so existing Iceberg
/// callers and unit tests continue to work. `handle` is the post-migration
/// object-storage address; when `Some`, [`run_with_ctx`](super::ldsc_hsq::LdscHsqNode::run_with_ctx)
/// registers a `ListingTable` under `table_name` and the SQL is rewritten to
/// read from it. `m_*` mirror the same dual-path support for the
/// companion `_m` (M_5_50) table.
pub struct LdScoreRef {
    /// Fully-qualified SQL for the main panel (legacy Iceberg path).
    pub sql: String,
    /// Fully-qualified SQL for the companion `_m` table (legacy Iceberg path).
    pub m_sql: String,
    /// Object-storage address for the main panel (post-migration path),
    /// when the catalog entry was declared as `ResourceKind::ObjectStorage`.
    pub handle: Option<ObjectStorageHandle>,
    /// Object-storage address for the companion `_m` table.
    pub m_handle: Option<ObjectStorageHandle>,
    /// Backend descriptor (Oss / S3 / Local) for the panel — used by
    /// [`register_listing_table`] to compose the URL scheme. Mirrors
    /// `handle`'s backend when present; defaults to `Local{root: "/"}` in
    /// the legacy Iceberg path.
    pub backend: ObjectStorageBackend,
    /// Backend descriptor for the companion `_m` table.
    pub m_backend: ObjectStorageBackend,
    /// DataFusion table name under which a `ListingTable` is registered for
    /// `handle` (used in SQL as `<table_name>`). Stable per logical name.
    pub table_name: String,
    /// DataFusion table name for the companion `_m` `ListingTable`.
    pub m_table_name: String,
}

impl LdScoreRef {
    /// Resolve from the resource catalog by logical name, falling back to
    /// the hardcoded `fallback_table` when the logical name is not registered.
    ///
    /// Resolution order:
    /// 1. **`ResourceKind::ObjectStorage`** — preferred post-migration.
    ///    Produces a `handle` + `table_name`; SQL is rewritten to read from
    ///    the `ListingTable` registered under `table_name`.
    /// 2. **`ResourceKind::IcebergTable`** — legacy. `sql` and `m_sql` are
    ///    populated; nodes interpolate the Iceberg identifier into SQL.
    /// 3. **Not registered** — both `sql` / `m_sql` fall back to the
    ///    hardcoded `iceberg.ld_score."<name>"` form so legacy in-test
    ///    `MemTable`-based fixtures still pass.
    ///
    /// The `_m` companion name is derived by appending `_m` to the base
    /// name — matching the convention used when LD-score panels are built
    /// (`sink_ld_matrix` / the LDSC annotation pipeline).
    pub fn resolve(catalog: &ResourceCatalog, logical: &str, fallback_table: &str) -> Self {
        // 1. ObjectStorage first — preferred post-migration.
        if let Ok(handle) = catalog.resolve_object_storage(logical) {
            let m_logical = format!("{logical}.m");
            let m_handle = catalog.resolve_object_storage(&m_logical).ok();
            let m_backend = m_handle
                .as_ref()
                .map(|h| h.backend.clone())
                .unwrap_or_default();
            return Self {
                sql: String::new(),
                m_sql: String::new(),
                handle: Some(handle.clone()),
                m_handle: m_handle.clone(),
                backend: handle.backend.clone(),
                m_backend,
                table_name: object_storage_table_name(logical),
                m_table_name: object_storage_table_name(&m_logical),
            };
        }

        // 2. Legacy Iceberg.
        if let Ok(ident) = catalog.resolve_iceberg(logical) {
            let m_ident = ident.with_table_suffix("_m");
            return Self {
                sql: ident.sql(),
                m_sql: m_ident.sql(),
                handle: None,
                m_handle: None,
                backend: ObjectStorageBackend::default(),
                m_backend: ObjectStorageBackend::default(),
                table_name: String::new(),
                m_table_name: String::new(),
            };
        }

        // 3. Fallback (test fixture, not-registered legacy).
        Self {
            sql: format!("iceberg.ld_score.\"{fallback_table}\""),
            m_sql: format!("iceberg.ld_score.\"{fallback_table}_m\""),
            handle: None,
            m_handle: None,
            backend: ObjectStorageBackend::default(),
            m_backend: ObjectStorageBackend::default(),
            table_name: String::new(),
            m_table_name: String::new(),
        }
    }

    /// The DataFusion table reference for the main panel:
    /// the registered `ListingTable` name when ObjectStorage is wired,
    /// otherwise the legacy Iceberg SQL identifier.
    pub fn panel_table_ref(&self) -> &str {
        if self.handle.is_some() {
            &self.table_name
        } else {
            &self.sql
        }
    }

    /// The DataFusion table reference for the companion `_m` table.
    pub fn m_table_ref(&self) -> &str {
        if self.m_handle.is_some() {
            &self.m_table_name
        } else {
            &self.m_sql
        }
    }

    /// `true` when this ref was resolved against an `ObjectStorage` entry —
    /// callers should call [`register_listing_table`] for both `panel`
    /// and `m_panel` before issuing SQL.
    pub fn uses_object_storage(&self) -> bool {
        self.handle.is_some()
    }
}

/// A resolved LD-matrix reference for per-chromosome pairwise r² tables.
///
/// The base table name (e.g. `eur_chr`) is resolved from the catalog;
/// individual chromosome tables are formed by appending the chromosome number
/// (e.g. `eur_chr1`, `eur_chr22`).
pub struct LdMatrixRef {
    pub catalog_name: String,
    pub schema: String,
    pub base_table: String,
}

impl LdMatrixRef {
    /// Resolve from the resource catalog, returning `None` when the logical
    /// name is not registered (caller falls back to hardcoded construction).
    pub fn resolve(catalog: &ResourceCatalog, logical: &str) -> Option<Self> {
        let ident = catalog.resolve_iceberg(logical).ok()?;
        Some(Self {
            catalog_name: ident.catalog,
            schema: ident.schema,
            base_table: ident.table,
        })
    }

    /// Fully-qualified SQL for a specific chromosome's LD-matrix table.
    pub fn chr_sql(&self, chrom: u32) -> String {
        format!(
            "\"{}\".\"{}\".{}{}",
            self.catalog_name, self.schema, self.base_table, chrom
        )
    }
}

/// Build a stable DataFusion table name for an ObjectStorage logical entry.
///
/// The name is `os_<sanitized_logical>` where sanitization strips non-ASCII
/// alphanumerics. Stable per logical name, so repeated calls against the
/// same catalog produce the same `ListingTable` name.
pub fn object_storage_table_name(logical: &str) -> String {
    let mut sanitized = String::with_capacity(logical.len());
    for ch in logical.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    format!("os_{sanitized}")
}

/// Register a DataFusion `ListingTable` for the given object-storage handle,
/// using the bucket's already-registered `opendal`-backed `ObjectStore`.
///
/// **The engine is responsible for the actual ObjectStore**: the
/// `runtime_env` shared by every node execution must have a DataFusion
/// `ObjectStore` registered under `backend.scheme()` (e.g. `"oss://"`,
/// `"s3://"`, `"file://"`) *before* this helper is called. The engine layer
/// (`data-engine`) reads `ResourceKind::ObjectStorage` entries from the
/// catalog at startup and wires OSS / S3 / Local ObjectStores based on the
/// entries' [`ObjectStorageBackend`] descriptors (endpoint, region, ak/sk).
/// This helper only:
/// - Composes the URL `<scheme>://<bucket><prefix>` from the handle + backend.
/// - Infers the parquet schema at that URL.
/// - Registers the resulting `ListingTable` under `table_name`.
///
/// Re-registration with the same `table_name` is a no-op.
pub async fn register_listing_table(
    ctx: &SessionContext,
    table_name: &str,
    handle: &ObjectStorageHandle,
    backend: &ObjectStorageBackend,
) -> Result<(), LdscCommonError> {
    // Already registered (e.g. catalog warmed by a prior call) → skip.
    match ctx.table_exist(table_name) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => return Err(LdscCommonError::ReadBatch(e)),
    }

    let url = format!("{}://{}{}", backend.scheme(), handle.bucket, handle.prefix);
    let table_url = ListingTableUrl::parse(&url).map_err(|e| {
        LdscCommonError::InvalidInput(format!(
            "failed to parse object-storage url '{url}': {e}"
        ))
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

/// Read per-annotation M_5_50 values from the companion `_m` table
/// (whose SQL identifier or registered `ListingTable` name is `m_table_ref`).
///
/// The table has two columns:
/// - `annotation` (Utf8) — annotation name (e.g. `"baseline"`)
/// - `m_5_50` (Float64) — L2-summed SNP count for that annotation
///
/// Returns the `m_5_50` values in table order. This is the correct M for the
/// LDSC regression normalisation (`h² = slope · M / N`), matching the
/// `.l2.M_5_50` file written when LD scores are computed.
///
/// `n_annot` is the expected number of annotations (1 for baseline h²/rg,
/// 97 for baselineLD v2.2 S-LDSC). An error is returned if the count differs.
pub async fn read_m_5_50(
    ctx: &SessionContext,
    m_table_ref: &str,
    n_annot: usize,
) -> Result<Vec<f64>, LdscCommonError> {
    let sql = format!(r#"SELECT "m_5_50" FROM {m_table_ref}"#);
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
    #[error("{0}")]
    InvalidInput(String),
}