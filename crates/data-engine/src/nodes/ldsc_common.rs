//! Shared helpers for LDSC nodes.
//!
//! Currently provides [`read_m_5_50`] — reading per-annotation M_5_50 values
//! from the Iceberg `ld_score.{table}_m` companion table — so that the h² and
//! rg nodes derive M from the same pre-computed source as the LD scores,
//! rather than approximating it with `COUNT(*)` of the panel table.

use arrow_array::Float64Array;

/// Read per-annotation M_5_50 values from the Iceberg companion table
/// `iceberg.ld_score.{ld_table}_m`.
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
    ld_table: &str,
    n_annot: usize,
) -> Result<Vec<f64>, LdscCommonError> {
    let sql = format!(r#"SELECT "m_5_50" FROM iceberg.ld_score."{ld_table}_m""#);
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
            "M table '{ld_table}_m' has {} rows but expected {n_annot} annotation(s)",
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
