//! Latent Causal Variable (LCV) node — genetic causality proportion (gcp).
//!
//! Takes **two** upstream GWAS summary-statistics `DataFrame`s (trait 1 and
//! trait 2, each with Z-scores, sample sizes, and rsid), queries the Iceberg
//! data lake for the LD score panel under `iceberg.ld_score.*`, inner-joins all
//! three on rsid so only SNPs shared by *both* traits and the panel survive,
//! and runs LCV via [`lcv::model::run_lcv`]. Outputs a single-row summary
//! `DataFrame` with the posterior gcp, its z-score and p-value, the genetic
//! correlation, and per-trait h² z-scores.
//!
//! This mirrors [`super::ldsc_rg::LdscRgNode`]; the structural difference is
//! the algorithm called after the 3-way join (LCV vs LDSC bivariate).

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum LcvNodeError {
    #[error("LCV computation failed: {0}")]
    Lcv(#[from] lcv::LcvError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("datalake error: {0}")]
    Datalake(String),
}

impl ::dag_core::dag::NodeError for LcvNodeError {
    fn node_type(&self) -> &str {
        "lcv"
    }
}


// =====================================================================
// Schemas
// =====================================================================

/// The fixed output schema of the LCV summary `DataFrame`.
///
/// Columns: `gcp_pm`, `gcp_pse`, `zscore`, `pval_gcpzero`, `pval_causal_1to2`,
/// `pval_causal_2to1`, `rho_est`, `rho_err`, `h2_zscore_1`, `h2_zscore_2`,
/// `n_snp` (all Float64).
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("gcp_pm", DataType::Float64, false),
        Field::new("gcp_pse", DataType::Float64, false),
        Field::new("zscore", DataType::Float64, false),
        Field::new("pval_gcpzero", DataType::Float64, false),
        Field::new("pval_causal_1to2", DataType::Float64, false),
        Field::new("pval_causal_2to1", DataType::Float64, false),
        Field::new("rho_est", DataType::Float64, false),
        Field::new("rho_err", DataType::Float64, false),
        Field::new("h2_zscore_1", DataType::Float64, false),
        Field::new("h2_zscore_2", DataType::Float64, false),
        Field::new("n_snp", DataType::Float64, false),
    ]))
}

/// Input column names each upstream GWAS sumstats `DataFrame` must expose.
const INPUT_Z_COL: &str = "z";
const INPUT_N_COL: &str = "n";
const INPUT_RSID_COL: &str = "rsid";

/// Input port schema (shared by both ports): per-SNP Z-score (`z`, Float64),
/// sample size (`n`, Float64), rsid join key (`rsid`, Utf8).
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// Build a single-row LCV summary `RecordBatch` from [`lcv::model::LcvOutput`].
fn build_result_batch(
    out: &lcv::model::LcvOutput,
    n_snp: usize,
) -> Result<RecordBatch, LcvNodeError> {
    let schema = output_schema();
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![out.gcp_pm])),
            Arc::new(Float64Array::from(vec![out.gcp_pse])),
            Arc::new(Float64Array::from(vec![out.zscore])),
            Arc::new(Float64Array::from(vec![out.pval_gcpzero_2tailed])),
            Arc::new(Float64Array::from(vec![out.pval_fullycausal[0]])),
            Arc::new(Float64Array::from(vec![out.pval_fullycausal[1]])),
            Arc::new(Float64Array::from(vec![out.rho_est])),
            Arc::new(Float64Array::from(vec![out.rho_err])),
            Arc::new(Float64Array::from(vec![out.h2_zscore[0]])),
            Arc::new(Float64Array::from(vec![out.h2_zscore[1]])),
            Arc::new(Float64Array::from(vec![n_snp as f64])),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Config
// =====================================================================

/// Configuration for the LCV node.
///
/// Mirrors the switches of R's `RunLCV` (intercept estimation, jackknife
/// blocks, significance threshold). Sample sizes are derived at execution time
/// from the joined per-SNP `n` columns.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LcvConfig {
    /// Number of jackknife blocks (R default 100).
    pub no_blocks: usize,
    /// If `true`, estimate univariate LDSC intercepts from the data
    /// (R `ldsc.intercept = 1`); if `false`, fix them at `1/N`.
    /// Default: `true` (recommended for real data).
    #[serde(default = "default_true")]
    pub ldsc_intercept: bool,
    /// If `true`, estimate the cross-trait LDSC intercept from the data
    /// (R `crosstrait.intercept = 1`); if `false`, fix it at 0 (disjoint
    /// cohorts). Default: `true`.
    #[serde(default = "default_true")]
    pub crosstrait_intercept: bool,
    /// Chisq significance threshold for excluding GWS SNPs when computing
    /// intercepts (R `sig.threshold`). `None` = no filtering (∞); a typical
    /// real-data value is 30.
    pub sig_threshold: Option<f64>,
    /// Fixed cross-trait intercept, used only when `crosstrait_intercept =
    /// false`. Should be 0 for non-overlapping cohorts.
    #[serde(default)]
    pub intercept12: f64,
}

fn default_true() -> bool {
    true
}

impl Default for LcvConfig {
    fn default() -> Self {
        Self {
            no_blocks: 100,
            ldsc_intercept: true,
            crosstrait_intercept: true,
            sig_threshold: None,
            intercept12: 0.0,
        }
    }
}

// =====================================================================
// Node
// =====================================================================

const LCV_NODE_KIND: &str = "lcv";

/// A transform node that runs LCV (Latent Causal Variable) for genetic
/// causality between two GWAS traits.
///
/// Accepts two upstream `DataFrame`s (trait 1 on port 0, trait 2 on port 1),
/// queries the Iceberg data lake for the LD score panel, inner-joins all three
/// on rsid, and runs LCV. Each upstream `DataFrame` must have columns `z`
/// (Float64), `n` (Float64), and `rsid` (Utf8).
#[derive(Clone)]
pub struct LcvNode {
    meta: NodePorts,
    config: LcvConfig,
}

pub struct LcvNodeFactory {}

impl NodeFactory for LcvNodeFactory {
    fn kind(&self) -> &'static str {
        LCV_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Latent Causal Variable model for genetic causality (gcp) between two GWAS traits."
    }

    fn doc(&self) -> &'static str {
        "Latent Causal Variable (LCV) transform node for inferring the genetic \
        causality proportion (gcp) between two GWAS traits. Takes two upstream \
        summary statistics DataFrames (trait 1 on port 0, trait 2 on port 1, each \
        with z, n, rsid), queries the Iceberg data lake for the LD score panel, \
        3-way joins on rsid, and runs LCV. Outputs a single-row summary with \
        posterior gcp mean/SE, partial-causality z-score and p-value, fully-causal \
        p-values, genetic correlation, and per-trait h² z-scores.\n\n\
        gcp=+1 implies trait 1 → trait 2 (fully causal); gcp=-1 implies trait 2 → \
        trait 1; gcp=0 implies no genetic causality (pleiotropy only).\n\n\
        IMPORTANT — you MUST perform the following quality control on the \
        input GWAS summary statistics BEFORE feeding them into this node:\n\
        1. Remove palindromic SNPs (alleles that are complementary on the two \
        strands, e.g. A/T or C/G) to avoid strand-alignment ambiguity.\n\
        2. Remove duplicate SNPs so each rsid appears at most once.\n\
        3. Keep only biallelic SNPs (exactly two alleles per variant).\n\
        4. Remove SNPs on sex chromosomes (X, Y, MT) — retain autosomal SNPs \
        only.\n\
        5. Filter by minor allele frequency: keep only SNPs with MAF > 0.01 \
        (1%).\n\
        6. Remove the MHC region.\n\
        7. Sort SNPs by genomic position (LCV uses a block jackknife; \
        non-contiguous ordering underestimates standard errors)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LcvConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: LcvConfig = serde_json::from_value(spec)?;
        let node = LcvNode::new(config);
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<LcvConfig>(spec, "lcv")?;
        let out = ctx.output_var.to_string();
        let input1 = ctx
            .input_vars
            .first()
            .cloned()
            .unwrap_or_else(|| "__missing_input_0".into());
        let input2 = ctx
            .input_vars
            .get(1)
            .cloned()
            .unwrap_or_else(|| "__missing_input_1".into());
        let ldsc_fit = ctx.fresh_var("ldsc_fit");
        let gcp = ctx.fresh_var("gcp");
        let code = vec![
            format!("# Latent Causal Variable (LCV) analysis"),
            format!("# Input 1: {input1}, Input 2: {input2}"),
            format!("# NOTE: LCV requires LDSC estimates of h2 and genetic covariance first"),
            format!(
                "{ldsc_fit} <- ldsc::estimate_rg({input1}, {input2}, n_blocks = {}{})",
                cfg.no_blocks,
                if cfg.ldsc_intercept {
                    ""
                } else {
                    ", intercept = FALSE"
                }
            ),
            format!("# Estimate genetic causality proportion (gcp)"),
            format!(
                "{gcp} <- lcv::estimate_gcp({ldsc_fit}, intercept12 = {})",
                cfg.intercept12
            ),
            format!("{out} <- {gcp}"),
            format!("cat(\"GCP:\", {out}$gcp, \"p-value:\", {out}$p_value, \"\\n\")"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["LDSC".into()]
    }
}

impl LcvNode {
    pub fn new(config: LcvConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

/// Static port layout: two typed inputs (trait 1, trait 2) and one typed output.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

/// Internal join output column aliases.
const LD_Z1_COL: &str = "z1";
const LD_Z2_COL: &str = "z2";
const LD_N1_COL: &str = "n1";
const LD_N2_COL: &str = "n2";
const LD_ELL_COL: &str = "ell";

#[async_trait]
impl DagNode for LcvNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        LCV_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input1 = inputs.iter().find(|i| i.port == 0).ok_or_else(|| {
            LcvNodeError::Lcv(lcv::LcvError::Input(
                "missing trait-1 input DataFrame (port 0)".into(),
            ))
        })?;
        let input2 = inputs.iter().find(|i| i.port == 1).ok_or_else(|| {
            LcvNodeError::Lcv(lcv::LcvError::Input(
                "missing trait-2 input DataFrame (port 1)".into(),
            ))
        })?;

        let ctx = node_ctx.session();
        crate::ldsc_common::register_listing_table(&ctx, "ld_panel", crate::ldsc_common::VFS_LDSCORE_1000G_EUR)
        .await
        .map_err(|e| LcvNodeError::Datalake(e.to_string()))?;
        let (out, n_snp) = Self::run_with_ctx(
            &ctx,
            &input1.data,
            &input2.data,
            "ld_panel",
            &self.config,
        )
        .await?;

        let batch = build_result_batch(&out, n_snp)?;
        let df = ctx.read_batch(batch).map_err(LcvNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

impl LcvNode {
    /// The catalog-independent LCV pipeline.
    ///
    /// Registers both upstream sumstats `DataFrame`s, runs the 3-way inner join
    /// on rsid with the LD score panel, collects aligned vectors, and calls
    /// [`lcv::model::run_lcv`].
    async fn run_with_ctx(
        ctx: &datafusion::prelude::SessionContext,
        input1: &datafusion::prelude::DataFrame,
        input2: &datafusion::prelude::DataFrame,
        panel_table: &str,
        cfg: &LcvConfig,
    ) -> Result<(lcv::model::LcvOutput, usize), DagError> {
        // 1. Register both upstream sumstats DataFrames.
        ctx.register_table("sumstats1", input1.clone().into_view())
            .map_err(LcvNodeError::ReadBatch)?;
        ctx.register_table("sumstats2", input2.clone().into_view())
            .map_err(LcvNodeError::ReadBatch)?;

        // 2. 3-way inner join on rsid.
        let ld_table = crate::ldsc_common::quote_table(panel_table);
        let sql = format!(
            r#"SELECT s1."{z}" AS "{Z1}", s2."{z}" AS "{Z2}",
                      s1."{n}" AS "{N1}", s2."{n}" AS "{N2}",
                      l.ld_score AS "{ELL}"
               FROM sumstats1 AS s1
               INNER JOIN sumstats2 AS s2 ON s1."{rsid}" = s2."{rsid}"
               INNER JOIN {ld_table} AS l ON s1."{rsid}" = l.rsid
               ORDER BY l.locus.position"#,
            z = INPUT_Z_COL,
            n = INPUT_N_COL,
            rsid = INPUT_RSID_COL,
            Z1 = LD_Z1_COL,
            Z2 = LD_Z2_COL,
            N1 = LD_N1_COL,
            N2 = LD_N2_COL,
            ELL = LD_ELL_COL,
        );

        // 3. Execute join and collect.
        let joined_df = ctx.sql(&sql).await.map_err(LcvNodeError::ReadBatch)?;
        let batches = joined_df
            .clone()
            .collect()
            .await
            .map_err(LcvNodeError::ReadBatch)?;
        if batches.is_empty() {
            return Err(LcvNodeError::Lcv(lcv::LcvError::Input(
                "lcv: joined DataFrame is empty (no SNPs shared by both traits and the LD panel)"
                    .into(),
            ))
            .into());
        }

        // 4. Extract aligned vectors.
        let z1 = extract_f64(&batches, LD_Z1_COL)?;
        let z2 = extract_f64(&batches, LD_Z2_COL)?;
        let n1_arr = extract_f64(&batches, LD_N1_COL)?;
        let n2_arr = extract_f64(&batches, LD_N2_COL)?;
        let ell = extract_f64(&batches, LD_ELL_COL)?;
        let n_snp = z1.len();

        if z2.len() != n_snp || n1_arr.len() != n_snp || n2_arr.len() != n_snp || ell.len() != n_snp
        {
            return Err(LcvNodeError::Lcv(lcv::LcvError::Input(format!(
                "lcv: aligned column length mismatch (z1={}, z2={}, n1={}, n2={}, ell={})",
                n_snp,
                z2.len(),
                n1_arr.len(),
                n2_arr.len(),
                ell.len()
            )))
            .into());
        }

        // 5. Filter NaN Z-scores (null → NaN from extract_f64).
        let mut z1f = Vec::new();
        let mut z2f = Vec::new();
        let mut ellf = Vec::new();
        for i in 0..n_snp {
            if z1[i].is_finite() && z2[i].is_finite() && ell[i].is_finite() {
                z1f.push(z1[i]);
                z2f.push(z2[i]);
                ellf.push(ell[i]);
            }
        }
        let (z1, z2, ell) = (z1f, z2f, ellf);
        let n_snp = z1.len();
        if n_snp < cfg.no_blocks {
            return Err(LcvNodeError::Lcv(lcv::LcvError::Input(format!(
                "lcv: need at least {} SNPs for {} jackknife blocks, got {} after NaN filtering",
                cfg.no_blocks, cfg.no_blocks, n_snp
            )))
            .into());
        }

        // 6. Regression weights: 1/max(1, ell) (R default).
        let weights: Vec<f64> = ell.iter().map(|&e| 1.0 / e.max(1.0)).collect();

        // 7. Derive representative sample sizes (mean of per-SNP n).
        let n1 = n1_arr.iter().filter(|v| v.is_finite()).sum::<f64>()
            / n1_arr.iter().filter(|v| v.is_finite()).count().max(1) as f64;
        let n2 = n2_arr.iter().filter(|v| v.is_finite()).sum::<f64>()
            / n2_arr.iter().filter(|v| v.is_finite()).count().max(1) as f64;

        // 8. Build K4Config and run LCV.
        let k4_cfg = lcv::moments::K4Config {
            crosstrait_intercept: cfg.crosstrait_intercept,
            ldsc_intercept: cfg.ldsc_intercept,
            sig_threshold: cfg.sig_threshold.unwrap_or(f64::INFINITY),
            n1,
            n2,
            intercept12: cfg.intercept12,
        };

        let out = lcv::model::run_lcv(&ell, &z1, &z2, &weights, cfg.no_blocks, &k4_cfg)
            .map_err(LcvNodeError::from)?;

        Ok((out, n_snp))
    }
}

// =====================================================================
// Column extraction
// =====================================================================

/// Extract a named numeric column from record batches into `Vec<f64>`,
/// casting nulls to NaN.
fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, LcvNodeError> {
    let schema = batches
        .first()
        .ok_or_else(|| LcvNodeError::Lcv(lcv::LcvError::Input("extract_f64: no batches".into())))?
        .schema();
    let idx = schema
        .index_of(name)
        .map_err(|_| LcvNodeError::Lcv(lcv::LcvError::Input(format!("missing column '{name}'"))))?;
    let dtype = schema.field(idx).data_type().clone();
    if !matches!(
        dtype,
        DataType::Float32
            | DataType::Float64
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    ) {
        return Err(LcvNodeError::Lcv(lcv::LcvError::Input(format!(
            "column '{name}' is not numeric (got {dtype})"
        ))));
    }

    let mut out = Vec::new();
    for batch in batches {
        push_numeric(batch.column(idx), &mut out);
    }
    Ok(out)
}

fn push_numeric(col: &dyn Array, out: &mut Vec<f64>) {
    macro_rules! cast {
        ($arr:expr, $t:ty) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$t>() {
                for v in a.iter() {
                    out.push(match v {
                        Some(val) => val as f64,
                        None => f64::NAN,
                    });
                }
                return;
            }
        };
    }
    cast!(col, Int8Array);
    cast!(col, Int16Array);
    cast!(col, Int32Array);
    cast!(col, Int64Array);
    cast!(col, UInt8Array);
    cast!(col, UInt16Array);
    cast!(col, UInt32Array);
    cast!(col, UInt64Array);
    cast!(col, Float32Array);
    cast!(col, Float64Array);
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            opendal: None,
            global_sem: None,
        }
    }
    use super::*;
    use arrow_array::{Array, Int64Array, StringArray, StructArray};
    use datafusion::catalog::{
        CatalogProvider, MemTable, MemoryCatalogProvider, MemorySchemaProvider, SchemaProvider,
    };
    use datafusion::prelude::SessionContext;

    // ── Structural tests ──

    #[tokio::test]
    async fn test_lcv_node_structure() {
        let node = LcvNode::new(LcvConfig::default());
        assert_eq!(node.kind(), "lcv");
        assert_eq!(node.ports().input_ports().len(), 2);
        assert_eq!(node.ports().output_ports().len(), 1);
    }

    #[test]
    fn test_output_schema_columns() {
        let schema = output_schema();
        assert_eq!(schema.fields().len(), 11);
        for name in [
            "gcp_pm",
            "gcp_pse",
            "zscore",
            "pval_gcpzero",
            "pval_causal_1to2",
            "pval_causal_2to1",
            "rho_est",
            "rho_err",
            "h2_zscore_1",
            "h2_zscore_2",
            "n_snp",
        ] {
            assert!(schema.field_with_name(name).is_ok(), "missing {name}");
        }
    }

    // ── In-memory catalog harness ──

    const N_SNP: usize = 2000;

    /// Build a synthetic LD-score panel batch.
    fn ld_panel_batch(n: usize) -> RecordBatch {
        let rsids: Vec<String> = (0..n).map(|i| format!("rs{}", 1_000_000 + i)).collect();
        let ld: Vec<f64> = (0..n).map(|i| 1.0 + 0.01 * i as f64).collect();
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

    /// Build synthetic GWAS sumstats batch.
    fn sumstats_batch(z: &[f64], rsids: &[String], n_samp: f64) -> RecordBatch {
        let n: Vec<f64> = vec![n_samp; z.len()];
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

    /// Build a SessionContext with an in-memory `iceberg.ld_score.1000g_eur`.
    fn ctx_with_ld_panel(n: usize) -> SessionContext {
        let ctx = SessionContext::new();
        let batch = ld_panel_batch(n);
        let schema = batch.schema();
        let table = MemTable::try_new(schema, vec![vec![batch]]).unwrap();
        ctx.register_table("1000g_eur", Arc::new(table)).unwrap();
        ctx
    }

    /// Run the full pipeline against the in-memory catalog.
    async fn run_pipeline(
        z1: &[f64],
        z2: &[f64],
        cfg: &LcvConfig,
    ) -> (lcv::model::LcvOutput, usize) {
        let rsids: Vec<String> = (0..z1.len())
            .map(|i| format!("rs{}", 1_000_000 + i))
            .collect();
        let ctx = ctx_with_ld_panel(N_SNP);
        let df1 = ctx.read_batch(sumstats_batch(z1, &rsids, 20000.0)).unwrap();
        let df2 = ctx.read_batch(sumstats_batch(z2, &rsids, 50000.0)).unwrap();
        LcvNode::run_with_ctx(&ctx, &df1, &df2, "1000g_eur", cfg)
            .await
            .expect("LCV pipeline should succeed")
    }

    // ── Known-answer tests ──

    /// When z2 = z1 (perfectly correlated), gcp should be near 0 (no causality
    /// signal in either direction — symmetric mixed 4th moments).
    #[tokio::test]
    async fn e2e_symmetric_signal_gcp_near_zero() {
        let z: Vec<f64> = (0..N_SNP).map(|i| 0.5 * (1.0 + 0.01 * i as f64)).collect();
        let cfg = LcvConfig {
            no_blocks: 20,
            ldsc_intercept: false,
            crosstrait_intercept: false,
            sig_threshold: None,
            intercept12: 0.0,
        };
        let (out, n_snp) = run_pipeline(&z, &z, &cfg).await;
        assert_eq!(n_snp, N_SNP);
        assert!(
            out.gcp_pm.abs() < 0.3,
            "symmetric signal should give gcp near 0, got {}",
            out.gcp_pm
        );
        assert!(
            out.rho_est > 0.0,
            "correlated signal ⇒ positive rho, got {}",
            out.rho_est
        );
    }

    /// When z2 = -z1 (perfectly anticorrelated), gcp should also be near 0
    /// (still symmetric, just negative rho).
    #[tokio::test]
    async fn e2e_anticorrelated_gcp_near_zero() {
        let z1: Vec<f64> = (0..N_SNP).map(|i| 0.5 * (1.0 + 0.01 * i as f64)).collect();
        let z2: Vec<f64> = z1.iter().map(|z| -z).collect();
        let cfg = LcvConfig {
            no_blocks: 20,
            ldsc_intercept: false,
            crosstrait_intercept: false,
            sig_threshold: None,
            intercept12: 0.0,
        };
        let (out, n_snp) = run_pipeline(&z1, &z2, &cfg).await;
        assert_eq!(n_snp, N_SNP);
        assert!(
            out.gcp_pm.abs() < 0.3,
            "symmetric anticorrelated signal should give gcp near 0, got {}",
            out.gcp_pm
        );
        assert!(out.rho_est < 0.0, "anticorrelated ⇒ negative rho");
    }

    /// Result batch has the declared schema.
    #[tokio::test]
    async fn e2e_result_batch_schema() {
        let z: Vec<f64> = (0..N_SNP).map(|i| 0.5 * (1.0 + 0.01 * i as f64)).collect();
        let cfg = LcvConfig {
            no_blocks: 20,
            ldsc_intercept: false,
            crosstrait_intercept: false,
            sig_threshold: None,
            intercept12: 0.0,
        };
        let (out, n_snp) = run_pipeline(&z, &z, &cfg).await;
        let batch = build_result_batch(&out, n_snp).unwrap();
        assert_eq!(batch.schema(), output_schema());
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 11);
    }

    /// Disjoint rsid sets must error, not silently return NaN.
    #[tokio::test]
    async fn e2e_disjoint_rsid_sets_yield_error() {
        let ctx = ctx_with_ld_panel(N_SNP);
        let rs1: Vec<String> = (0..50).map(|i| format!("rs{}", 1_000_000 + i)).collect();
        let rs2: Vec<String> = (0..50).map(|i| format!("rs{}", 9_000_000 + i)).collect();
        let z: Vec<f64> = (0..50).map(|i| 0.1 * i as f64).collect();

        let df1 = ctx.read_batch(sumstats_batch(&z, &rs1, 1000.0)).unwrap();
        let df2 = ctx.read_batch(sumstats_batch(&z, &rs2, 1000.0)).unwrap();

        let res = LcvNode::run_with_ctx(
            &ctx,
            &df1,
            &df2,
            "1000g_eur",
            &LcvConfig {
                no_blocks: 10,
                ..Default::default()
            },
        )
        .await;
        assert!(res.is_err(), "disjoint rsid sets must error");
    }

    /// Missing input port must error.
    #[tokio::test]
    async fn e2e_missing_input_yields_error() {
        let mut node = LcvNode::new(LcvConfig::default());
        let batch = sumstats_batch(
            &[1.0, 2.0, 3.0],
            &["rs1".into(), "rs2".into(), "rs3".into()],
            1000.0,
        );
        let df = SessionContext::new().read_batch(batch).unwrap();
        let one_input = vec![dag_core::node::NodeInput { port: 0, data: df }];
        let res = node
            .execute(
                &node_ctx(),
                &one_input,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await;
        assert!(res.is_err(), "missing trait-2 input must error");
    }
}
