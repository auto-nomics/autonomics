//! Shared helpers for LDSC nodes.
//!
//! Currently provides:
//! - [`LdScoreRef`] — catalog-aware resolution of LD-score table SQL identifiers
//! - [`LdMatrixRef`] — catalog-aware resolution of per-chromosome LD-matrix tables
//! - [`read_m_5_50`] — reading per-annotation M_5_50 values from companion `_m` tables

use arrow_array::Float64Array;
use dag_core::resource_catalog::{IcebergIdent, ResourceCatalog};

// ── Catalog-aware table references ────────────────────────────────────────

/// A resolved LD-score table reference: the fully-qualified SQL identifiers
/// for the main panel and its companion `_m` (M_5_50) table.
///
/// Nodes build this from the resource catalog (preferred) or fall back to a
/// hardcoded table name when the catalog is empty (backward compat).
pub struct LdScoreRef {
    /// Fully-qualified SQL for the main LD-score panel, e.g.
    /// `"iceberg"."ld_score"."1000g_eur"`.
    pub sql: String,
    /// Fully-qualified SQL for the companion `_m` table, e.g.
    /// `"iceberg"."ld_score"."1000g_eur_m"`.
    pub m_sql: String,
}

impl LdScoreRef {
    /// Resolve from the resource catalog by logical name, falling back to
    /// the hardcoded `fallback_table` when the logical name is not registered.
    ///
    /// The `_m` companion table name is derived by appending `_m` to the
    /// table name — matching the convention used when LD-score panels are
    /// built (`sink_ld_matrix` / the LDSC annotation pipeline).
    pub fn resolve(catalog: &ResourceCatalog, logical: &str, fallback_table: &str) -> Self {
        match catalog.resolve_iceberg(logical) {
            Ok(ident) => {
                let m_ident = ident.with_table_suffix("_m");
                Self {
                    sql: ident.sql(),
                    m_sql: m_ident.sql(),
                }
            }
            Err(_) => Self {
                sql: format!("iceberg.ld_score.\"{fallback_table}\""),
                m_sql: format!("iceberg.ld_score.\"{fallback_table}_m\""),
            },
        }
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

/// Read per-annotation M_5_50 values from the Iceberg companion table
/// (the `_m` table whose SQL identifier is `m_sql`).
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
    ctx: &datafusion::prelude::SessionContext,
    m_sql: &str,
    n_annot: usize,
) -> Result<Vec<f64>, LdscCommonError> {
    let sql = format!(r#"SELECT "m_5_50" FROM {m_sql}"#);
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
            "M table '{m_sql}' has {} rows but expected {n_annot} annotation(s)",
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
