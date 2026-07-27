//! LD Score Regression (LDSC) transform node.
//!
//! Takes a single upstream GWAS summary statistics `DataFrame` (with Z-scores,
//! sample sizes, and rsid), queries the Iceberg data lake for LD score panel
//! data under `genetics.ld_score`, joins on rsid, and runs LD Score Regression
//! via [`ldsc::hsq::estimate_h2`]. Outputs a single-row summary `DataFrame`
//! with h², intercept, ratio, and per-annotation coefficients.

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory, new_isolated_ctx},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum LdscNodeError {
    #[error("LDSC computation failed: {0}")]
    Ldsc(#[from] ldsc::LdscError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("datalake error: {0}")]
    Datalake(String),
}

impl From<LdscNodeError> for DagError {
    fn from(e: LdscNodeError) -> Self {
        DagError::NodeError {
            node_type: "ldsc".to_string(),
            msg: e.to_string(),
        }
    }
}

impl From<datalake::error::Error> for LdscNodeError {
    fn from(e: datalake::error::Error) -> Self {
        LdscNodeError::Datalake(e.to_string())
    }
}

// =====================================================================
// Output DataFrame construction
// =====================================================================

/// The fixed output schema of the LDSC h² summary `DataFrame`.
///
/// Columns: `h2`, `h2_se`, `intercept`, `intercept_se`, `ratio`,
/// `ratio_se`, `mean_chisq`, `lambda_gc`, `n_snp` (Float64),
/// `coef`, `coef_se` (Utf8 — JSON arrays).
///
/// This is the single source of truth shared by the node's declared output
/// port (so the DAG can validate downstream edges) and [`build_result_batch`].
///
/// Reused by downstream transform nodes (e.g. [`super::liability`]) as their
/// typed input port schema, so only an `ldsc` h² summary can feed them.
pub fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("h2", DataType::Float64, false),
        Field::new("h2_se", DataType::Float64, false),
        Field::new("intercept", DataType::Float64, true),
        Field::new("intercept_se", DataType::Float64, true),
        Field::new("ratio", DataType::Float64, true),
        Field::new("ratio_se", DataType::Float64, true),
        Field::new("mean_chisq", DataType::Float64, false),
        Field::new("lambda_gc", DataType::Float64, false),
        Field::new("n_snp", DataType::Float64, false),
        Field::new("coef", DataType::Utf8, false),
        Field::new("coef_se", DataType::Utf8, false),
    ]))
}

/// The fixed input column names for the upstream GWAS sumstats `DataFrame`.
/// All upstream data must use these exact column names; they are enforced by
/// [`input_schema`] so the DAG rejects misshaped edges at `add_edge` time.
const INPUT_Z_COL: &str = "z";
const INPUT_N_COL: &str = "n";
const INPUT_RSID_COL: &str = "rsid";

/// Build the input port schema with the fixed upstream sumstats column names:
/// per-SNP Z-score (`z`, Float64), sample size (`n`, Float64), and rsid join
/// key (`rsid`, Utf8). These are exactly the columns the internal SQL join
/// reads, so typing the input port lets the DAG reject misshaped upstream
/// edges at `add_edge` time.
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// Build a single-row summary `RecordBatch` from the LDSC result.
fn build_result_batch(r: &ldsc::hsq::HsqResult) -> Result<RecordBatch, LdscNodeError> {
    let schema = output_schema();

    let coef_json = serde_json::to_string(&r.coef)
        .map_err(|e| LdscNodeError::Ldsc(ldsc::LdscError::InvalidInput(e.to_string())))?;
    let coef_se_json = serde_json::to_string(&r.coef_se)
        .map_err(|e| LdscNodeError::Ldsc(ldsc::LdscError::InvalidInput(e.to_string())))?;

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![r.h2])),
            Arc::new(Float64Array::from(vec![r.h2_se])),
            Arc::new(Float64Array::from(vec![r.intercept])),
            Arc::new(Float64Array::from(vec![r.intercept_se])),
            Arc::new(Float64Array::from(vec![r.ratio])),
            Arc::new(Float64Array::from(vec![r.ratio_se])),
            Arc::new(Float64Array::from(vec![r.mean_chisq])),
            Arc::new(Float64Array::from(vec![r.lambda_gc])),
            Arc::new(Float64Array::from(vec![r.n_snp as f64])),
            Arc::new(StringArray::from(vec![coef_json])),
            Arc::new(StringArray::from(vec![coef_se_json])),
        ],
    )?;

    Ok(batch)
}

// =====================================================================
// Node
// =====================================================================

/// A transform node that runs LD Score Regression for SNP-heritability (h²).
///
/// Accepts raw GWAS summary statistics as input, queries the Iceberg data lake
/// for LD score panel data, performs the join internally, and runs LDSC.
///
/// The upstream `DataFrame` must have columns named exactly `z` (Float64),
/// `n` (Float64), and `rsid` (Utf8) — enforced by the node's input port schema.
#[derive(Clone)]
pub struct LdscHsqNode {
    /// DAG node metadata (id, ports).
    meta: NodePorts,
    /// Shared object-store registry, used to build an isolated context for
    /// the join SQL against upstream sumstats.
    runtime_env: Arc<datafusion::execution::runtime_env::RuntimeEnv>,
    /// Optional Iceberg catalog, registered under `"iceberg"` on the
    /// per-execution context so the join SQL resolves
    /// `iceberg.ld_score.*`.
    iceberg_catalog: Option<Arc<dyn datafusion::catalog::CatalogProvider>>,
    /// Configuration for the LDSC h² estimation algorithm itself
    /// (per-annotation M, jackknife blocks, optional fixed intercept).
    /// See [`LdscHsqConfig`].
    ldsc_hsq: LdscHsqConfig,
}

/// Configuration for the LDSC h² estimation algorithm.
///
/// Bundles the parameters that govern the LDSC regression itself.  M (the
/// number of SNPs in the LD score annotation) is derived at execution time by
/// counting the rows in the LD score panel table, so the caller does not need
/// to supply it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LdscHsqConfig {
    /// Number of block-jackknife blocks used by
    /// [`ldsc::hsq::estimate_h2`] to compute standard errors and the
    /// intercept. A typical value is 200.
    pub n_blocks: usize,
    /// Optional fixed intercept for the LDSC regression.
    ///
    /// `None` (the common case) lets [`ldsc::hsq::estimate_h2`] estimate
    /// the intercept from the data, where it absorbs confounding such as
    /// population stratification. `Some(v)` constrains the regression
    /// to pass through `(0, v)`.
    pub intercept: Option<f64>,
}

impl LdscHsqConfig {
    /// Construct an [`LdscHsqConfig`].
    pub fn new(n_blocks: usize, intercept: Option<f64>) -> Self {
        Self {
            n_blocks,
            intercept,
        }
    }
}

pub struct LdscHsqNodeFactory {}

/// Static port layout for every [`LdscHsqNode`]: a single typed input carrying
/// GWAS sumstats (z, n, rsid) and a single typed output with the fixed h²
/// summary schema.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl NodeFactory for LdscHsqNodeFactory {
    fn kind(&self) -> &'static str {
        "ldsc"
    }

    fn desc(&self) -> &'static str {
        "LD Score Regression for SNP-heritability (h²) estimation from GWAS summary statistics."
    }

    fn doc(&self) -> &'static str {
        "LD Score Regression (LDSC) transform node for SNP-heritability (h²) \
        estimation. Takes a single upstream GWAS summary statistics DataFrame \
        (with z, n, rsid columns), queries the Iceberg data lake for LD score \
        panel data, joins on rsid, and runs LDSC via block-jackknife. Outputs a \
        single-row summary with h², intercept, ratio, and per-annotation \
        coefficients."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LdscHsqConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: LdscHsqConfig = serde_json::from_value(spec)?;
        let node = LdscHsqNode::new(node_ctx.runtime_env, node_ctx.iceberg_catalog, config);
        Ok(Box::new(node))
    }
}

impl LdscHsqNode {
    /// Construct an [`LdscHsqNode`].
    ///
    /// # Arguments
    ///
    /// * `runtime_env` — shared object-store registry, used to build the
    ///   per-execution context for the join SQL.
    /// * `iceberg_catalog` — Iceberg catalog, registered under `"iceberg"`
    ///   so the join SQL resolves `iceberg.ld_score.*`.
    /// * `ldsc_hsq` — algorithm configuration; see [`LdscHsqConfig`].
    ///
    /// The upstream `DataFrame` must expose columns `z` (Float64), `n`
    /// (Float64), and `rsid` (Utf8) — enforced by the input port schema.
    pub fn new(
        runtime_env: Arc<datafusion::execution::runtime_env::RuntimeEnv>,
        iceberg_catalog: Option<Arc<dyn datafusion::catalog::CatalogProvider>>,
        ldsc_hsq: LdscHsqConfig,
    ) -> Self {
        // Fixed, typed ports: a single input carrying GWAS sumstats (z, n,
        // rsid) and a single output with the fixed h² summary schema.
        // Declaring the schemas lets the DAG validate edge compatibility
        // at `add_edge`/`validate` time.
        Self {
            meta: port_layout(),
            runtime_env,
            iceberg_catalog,
            ldsc_hsq,
        }
    }
}

/// The fixed column names used in the internal SQL join for passing to
/// [`ldsc::hsq::HsqColumns`]. The SQL aliases output columns to these names
/// so the downstream LDSC computation is independent of the user-facing
/// column names.
const LD_Z_COL: &str = "z";
const LD_N_COL: &str = "n";
const LD_REF_COL: &str = "l2_0";
const LD_WLD_COL: &str = "wld";

#[async_trait]
impl DagNode for LdscHsqNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "ldsc"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(LdscNodeError::Ldsc(ldsc::LdscError::InvalidInput(
                "no input DataFrame".into(),
            )))?;

        // 1. Build an isolated DataFusion context with the Iceberg catalog
        //    registered (under "iceberg"), then delegate to the
        //    catalog-independent pipeline. Splitting here lets the pipeline
        //    be exercised end-to-end against an in-memory catalog (see
        //    `tests`).
        let ctx = new_isolated_ctx(self.runtime_env.clone(), self.iceberg_catalog.clone());

        // TODO: Auto select LD score panel table by population
        let result = Self::run_with_ctx(&ctx, &input.data, "ukbb_eur", &self.ldsc_hsq).await?;

        // 2. Build a single-row summary RecordBatch and return.
        let batch = build_result_batch(&result)?;
        let df = ctx.read_batch(batch).map_err(LdscNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

impl LdscHsqNode {
    /// The catalog-independent h² pipeline.
    ///
    /// Given a [`SessionContext`] in which `iceberg.ld_score.{ld_table}` resolves
    /// to an LD-score panel, this registers the upstream sumstats `DataFrame` as
    /// `sumstats`, runs the inner join on rsid, and fits
    /// [`ldsc::hsq::estimate_h2`].
    ///
    /// Extracted from [`DagNode::execute`](LdscHsqNode::execute) so the full
    /// pipeline can be tested against an in-memory catalog without a live
    /// Iceberg REST server.
    async fn run_with_ctx(
        ctx: &datafusion::prelude::SessionContext,
        input: &datafusion::prelude::DataFrame,
        ld_table: &str,
        cfg: &LdscHsqConfig,
    ) -> Result<ldsc::hsq::HsqResult, DagError> {
        // 1. Register the upstream sumstats DataFrame as a temporary table.
        ctx.register_table("sumstats", input.clone().into_view())
            .map_err(LdscNodeError::ReadBatch)?;

        // 2. Count the total SNPs in the LD score panel to derive M — the
        //    normalising constant in the LDSC regression.
        //
        // NOTE: Rare SNP need to be filtered out (MAF < 0.05)
        let count_sql = format!(
            r#"SELECT COUNT(*) AS "n" FROM iceberg.ld_score.{ld_table} WHERE "AF" BETWEEN 0.05 AND 0.95"#
        );
        let count_df = ctx
            .sql(&count_sql)
            .await
            .map_err(LdscNodeError::ReadBatch)?;
        let count_batches = count_df.collect().await.map_err(LdscNodeError::ReadBatch)?;
        let panel_count = extract_scalar_u64(&count_batches, "n")?;
        let m = vec![panel_count as f64];

        // 3. Build SQL: join sumstats with LD score panel on rsid.
        //    ld_score is used for both ref_ld and w_ld (single-annotation baseline).
        let sql = format!(
            r#"SELECT s."{z}" AS "{Z}", s."{n}" AS "{N}", l.ld_score AS "{REF}", l.ld_score AS "{WLD}"
               FROM sumstats AS s
               INNER JOIN iceberg.ld_score.{table} AS l
               ON s."{rsid}" = l.rsid
               ORDER BY l.locus.position"#,
            z = INPUT_Z_COL,
            n = INPUT_N_COL,
            rsid = INPUT_RSID_COL,
            table = ld_table,
            Z = LD_Z_COL,
            N = LD_N_COL,
            REF = LD_REF_COL,
            WLD = LD_WLD_COL,
        );

        // 4. Execute the join.
        let joined_df = ctx.sql(&sql).await.map_err(LdscNodeError::ReadBatch)?;

        // 5. Build the LDSC column-name descriptor and run LDSC.
        let cols = ldsc::hsq::HsqColumns {
            snp: "", // not consumed by the computation
            z: LD_Z_COL,
            n: LD_N_COL,
            ref_ld: vec![LD_REF_COL],
            w_ld: LD_WLD_COL,
        };
        let LdscHsqConfig {
            n_blocks,
            intercept,
        } = cfg;
        let result = ldsc::hsq::estimate_h2(joined_df, cols, &m, *n_blocks, *intercept)
            .await
            .map_err(LdscNodeError::from)?;

        Ok(result)
    }
}

/// Extract a single u64 scalar from a one-row, one-column `RecordBatch` result.
fn extract_scalar_u64(batches: &[RecordBatch], col: &str) -> Result<u64, LdscNodeError> {
    let batch = batches
        .first()
        .ok_or(LdscNodeError::Ldsc(ldsc::LdscError::InvalidInput(
            "extract_scalar_u64: no batches".into(),
        )))?;
    let idx = batch.schema().index_of(col).map_err(|_| {
        LdscNodeError::Ldsc(ldsc::LdscError::InvalidInput(format!(
            "missing column '{col}'"
        )))
    })?;
    let arr = batch.column(idx);
    if arr.is_null(0) {
        return Err(LdscNodeError::Ldsc(ldsc::LdscError::InvalidInput(
            "extract_scalar_u64: null value".into(),
        )));
    }
    let dtype = arr.data_type();
    match dtype {
        DataType::UInt64 => Ok(arr
            .as_any()
            .downcast_ref::<arrow_array::UInt64Array>()
            .unwrap()
            .value(0)),
        DataType::Int64 => Ok(arr
            .as_any()
            .downcast_ref::<arrow_array::Int64Array>()
            .unwrap()
            .value(0) as u64),
        DataType::UInt32 => Ok(arr
            .as_any()
            .downcast_ref::<arrow_array::UInt32Array>()
            .unwrap()
            .value(0) as u64),
        DataType::Int32 => Ok(arr
            .as_any()
            .downcast_ref::<arrow_array::Int32Array>()
            .unwrap()
            .value(0) as u64),
        _ => Err(LdscNodeError::Ldsc(ldsc::LdscError::InvalidInput(format!(
            "extract_scalar_u64: unsupported dtype {dtype} for column '{col}'"
        )))),
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, Float64Array, Int64Array, StringArray, StructArray};
    use datafusion::catalog::{
        CatalogProvider, MemTable, MemoryCatalogProvider, MemorySchemaProvider, SchemaProvider,
    };
    use datafusion::prelude::SessionContext;

    // -----------------------------------------------------------------
    // Structural checks (no catalog, no execution)
    // -----------------------------------------------------------------

    /// Construct the node and assert its kind and single-in/single-out topology.
    #[tokio::test]
    async fn test_ldsc_hsq_node_structure() {
        let node = LdscHsqNode::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
            LdscHsqConfig::new(5, None),
        );
        assert_eq!(node.kind(), "ldsc");
        assert_eq!(node.ports().input_ports().len(), 1);
        assert_eq!(node.ports().output_ports().len(), 1);
    }

    /// The declared output schema has exactly the h² summary columns.
    #[test]
    fn test_output_schema_has_all_columns() {
        let schema = output_schema();
        for name in [
            "h2",
            "h2_se",
            "intercept",
            "intercept_se",
            "ratio",
            "ratio_se",
            "mean_chisq",
            "lambda_gc",
            "n_snp",
            "coef",
            "coef_se",
        ] {
            assert!(schema.field_with_name(name).is_ok(), "missing {name}");
        }
    }

    // -----------------------------------------------------------------
    // In-memory catalog harness
    // -----------------------------------------------------------------
    //
    // Same approach as `ldsc_rg::tests`: register an in-memory
    // `MemoryCatalogProvider` under the production `iceberg` name with a
    // `ld_score.ukbb_eur` `MemTable`, so the node's SQL resolves identically
    // to production and the full pipeline (join → estimate_h2 → batch) runs
    // deterministically with no external service.

    /// LD-panel row count used by the synthetic fixtures.
    const N_SNP: usize = 200;
    /// Per-SNP sample size used by the fixtures.
    const N_SAMP: f64 = 1000.0;

    /// Synthetic LD-score panel `RecordBatch` (`rsid`, `ld_score`,
    /// `locus<position>`), matching the node SQL's `l.rsid`, `l.ld_score`,
    /// `l.locus.position`. `ld_score` strictly increases; `position` tracks it
    /// so `ORDER BY l.locus.position` preserves LD order.
    fn ld_panel_batch(n: usize) -> RecordBatch {
        let rsids: Vec<String> = (0..n).map(|i| format!("rs{}", 1_000_000 + i)).collect();
        let ld: Vec<f64> = (0..n).map(|i| 1.0 + 0.1 * i as f64).collect();
        let pos: Vec<i64> = (0..n).map(|i| i as i64).collect();

        let position_field = Arc::new(Field::new("position", DataType::Int64, false));
        let locus = StructArray::new(
            vec![position_field].into(),
            vec![Arc::new(Int64Array::from(pos)) as Arc<dyn Array>],
            None,
        );
        let schema = Arc::new(Schema::new(vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("ld_score", DataType::Float64, false),
            Field::new(
                "locus",
                DataType::Struct(
                    vec![Arc::new(Field::new("position", DataType::Int64, false))].into(),
                ),
                false,
            ),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(rsids)),
                Arc::new(Float64Array::from(ld)),
                Arc::new(locus) as Arc<dyn Array>,
            ],
        )
        .unwrap()
    }

    /// Synthetic single-trait GWAS sumstats `RecordBatch` (`z`, `n`, `rsid`).
    fn sumstats_batch(z: &[f64], rsids: &[String]) -> RecordBatch {
        let n: Vec<f64> = vec![N_SAMP; z.len()];
        let schema = Arc::new(Schema::new(vec![
            Field::new("z", DataType::Float64, false),
            Field::new("n", DataType::Float64, false),
            Field::new("rsid", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(z.to_vec())),
                Arc::new(Float64Array::from(n)),
                Arc::new(StringArray::from(rsids.to_vec())),
            ],
        )
        .unwrap()
    }

    /// `SessionContext` with an in-memory `iceberg.ld_score.ukbb_eur` table.
    fn ctx_with_ld_panel(n: usize) -> SessionContext {
        let ctx = SessionContext::new();
        let batch = ld_panel_batch(n);
        let table = MemTable::try_new(batch.schema(), vec![vec![batch]]).unwrap();
        let ld_schema = MemorySchemaProvider::new();
        ld_schema
            .register_table("ukbb_eur".to_string(), Arc::new(table))
            .unwrap();
        let catalog = MemoryCatalogProvider::new();
        catalog
            .register_schema("ld_score", Arc::new(ld_schema))
            .unwrap();
        ctx.register_catalog("iceberg", Arc::new(catalog));
        ctx
    }

    /// Build Z-scores whose χ² = Z² follows the LDSC mean model exactly:
    /// `χ² = 1 + (N/M)·h2·ld`. With a constrained intercept the regression
    /// recovers `h2` to machine precision.
    fn z_from_linear_model(ld: &[f64], h2: f64, m: f64) -> Vec<f64> {
        let slope = (N_SAMP / m) * h2;
        ld.iter().map(|l| (1.0 + slope * l).sqrt()).collect()
    }

    /// Run the full pipeline against the in-memory catalog for one trait.
    async fn run_pipeline(z: &[f64], cfg: &LdscHsqConfig) -> ldsc::hsq::HsqResult {
        let rsids: Vec<String> = (0..z.len())
            .map(|i| format!("rs{}", 1_000_000 + i))
            .collect();
        let ctx = ctx_with_ld_panel(N_SNP);
        let df = ctx.read_batch(sumstats_batch(z, &rsids)).unwrap();
        LdscHsqNode::run_with_ctx(&ctx, &df, "ukbb_eur", cfg)
            .await
            .expect("hsq pipeline should succeed")
    }

    // -----------------------------------------------------------------
    // Known-answer end-to-end tests
    // -----------------------------------------------------------------

    /// With a constrained intercept (`intercept = Some(1)`) and a noiseless
    /// linear χ²-vs-LD signal, h² is recovered analytically: the design column
    /// is `ld` (N constant), χ² = 1 + (N/M)·h²·ld ⇒ slope = (N/M)·h² ⇒
    /// h² = M·slope/N exactly. `mean_chisq` is a plain sample mean ⇒ exact.
    #[tokio::test]
    async fn e2e_constrained_intercept_recovers_h2_exactly() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        let target_h2 = 0.5;
        let m = N_SNP as f64;
        let z = z_from_linear_model(&ld, target_h2, m);
        let cfg = LdscHsqConfig::new(20, Some(1.0));

        let r = run_pipeline(&z, &cfg).await;

        assert_eq!(r.n_snp, N_SNP, "all SNPs must survive the join");
        assert!(
            (r.h2 - target_h2).abs() < 1e-4,
            "h2 should be ≈ {target_h2}, got {}",
            r.h2
        );
        // Constrained intercept ⇒ the result reports the fixed value (1.0).
        let intercept = r
            .intercept
            .expect("constrained intercept should report the fixed value");
        assert!(
            (intercept - 1.0).abs() < 1e-9,
            "constrained intercept should be 1.0, got {}",
            intercept
        );
        // mean χ² is a deterministic sample mean of the fixture.
        let expected_mean_chisq: f64 = 1.0 + (N_SAMP / m) * target_h2 * mean_ld();
        assert!(
            (r.mean_chisq - expected_mean_chisq).abs() < 1e-9,
            "mean_chisq should be ≈ {expected_mean_chisq}, got {}",
            r.mean_chisq
        );
        // λ_GC is the median χ² / 0.4549; with a linear, increasing χ² the
        // median sits at the middle LD score — just assert finiteness & > 0.
        assert!(
            r.lambda_gc.is_finite() && r.lambda_gc > 0.0,
            "lambda_gc={}",
            r.lambda_gc
        );
    }

    /// The production default config (free intercept + two-step cutoff 30) on a
    /// signal whose χ² stays below the cutoff must also recover h² and the
    /// free intercept (≈1 here, since the model intercept is 1) closely.
    #[tokio::test]
    async fn e2e_default_config_recovers_h2_and_intercept() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        // slope = 1.0 ⇒ max χ² = 1 + 20.9 = 21.9 < 30 (two-step keeps all),
        // target h² = M·slope/N = 200/1000 = 0.2.
        let target_h2 = 0.2;
        let m = N_SNP as f64;
        // Build χ² = 1 + 1.0·ld directly (slope 1 ⇒ h² = M·1/N = 0.2).
        let slope = 1.0_f64;
        let z: Vec<f64> = ld.iter().map(|l| (1.0 + slope * l).sqrt()).collect();
        assert!(
            (target_h2 - m * slope / N_SAMP).abs() < 1e-12,
            "fixture self-check"
        );

        let cfg = LdscHsqConfig::new(20, None);
        let r = run_pipeline(&z, &cfg).await;

        assert_eq!(r.n_snp, N_SNP);
        assert!(
            (r.h2 - target_h2).abs() < 1e-3,
            "h2 should be ≈ {target_h2}, got {}",
            r.h2
        );
        let intercept = r
            .intercept
            .expect("free-intercept fit should report an intercept estimate");
        assert!(
            (intercept - 1.0).abs() < 1e-3,
            "intercept should be ≈ 1.0, got {}",
            intercept
        );
    }

    /// `build_result_batch` wires the fitted `HsqResult` into exactly the
    /// declared output schema, including the JSON-encoded coef arrays.
    #[tokio::test]
    async fn e2e_result_batch_has_declared_schema() {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        let z = z_from_linear_model(&ld, 0.5, N_SNP as f64);
        let r = run_pipeline(&z, &LdscHsqConfig::new(20, Some(1.0))).await;

        let batch = build_result_batch(&r).unwrap();
        assert_eq!(batch.schema(), output_schema());
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 11);
        // n_snp column carries the joined SNP count.
        let n_snp_col = batch
            .column(8)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(n_snp_col.value(0) as usize, N_SNP);
        // coef / coef_se are JSON arrays (single annotation ⇒ length-1 arrays).
        let coef_json = batch
            .column(9)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(0);
        let coef: Vec<f64> = serde_json::from_str(coef_json).unwrap();
        assert_eq!(coef.len(), 1);
    }

    // -----------------------------------------------------------------
    // Error-path end-to-end tests
    // -----------------------------------------------------------------

    /// When the sumstats rsids are absent from the LD panel, the inner join is
    /// empty and the pipeline must error rather than silently return NaNs.
    #[tokio::test]
    async fn e2e_no_overlap_with_panel_yields_error() {
        let ctx = ctx_with_ld_panel(N_SNP);
        // rsids the panel does not contain.
        let rsids: Vec<String> = (0..50).map(|i| format!("rs{}", 9_000_000 + i)).collect();
        let z: Vec<f64> = (0..50).map(|i| (i as f64) * 0.1).collect();
        let df = ctx.read_batch(sumstats_batch(&z, &rsids)).unwrap();

        let res =
            LdscHsqNode::run_with_ctx(&ctx, &df, "ukbb_eur", &LdscHsqConfig::new(20, None)).await;
        assert!(
            res.is_err(),
            "no rsid overlap must error, not silently return NaN"
        );
    }

    /// A missing input must surface a clear error before any catalog work.
    #[tokio::test]
    async fn e2e_missing_input_yields_error() {
        let mut node = LdscHsqNode::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
            LdscHsqConfig::new(5, None),
        );
        let res = node
            .execute(&[], &crate::dag::node_event::NodeReporter::noop())
            .await;
        assert!(res.is_err(), "missing input must error");
    }

    // -----------------------------------------------------------------
    // Small numeric helpers
    // -----------------------------------------------------------------

    /// Mean of the fixture's LD scores (arithmetic series 1.0 .. 20.9).
    fn mean_ld() -> f64 {
        let ld: Vec<f64> = (0..N_SNP).map(|i| 1.0 + 0.1 * i as f64).collect();
        ld.iter().sum::<f64>() / ld.len() as f64
    }

    // #[test]
    // fn test_factory_spec_schema() {
    //     let schema = LdscHsqNodeFactory {}.spec_schema();
    //     dbg!(schema);
    //     panic!()
    // }
}
