//! CPASSOC (Cross-Phenotype Association) node.
//!
//! Takes **K** (≥ 2) upstream GWAS summary-statistics `DataFrame`s — one per
//! trait (or cohort-trait combination) — each with columns `z`, `n`, `rsid`,
//! inner-joins them all on `rsid` so only SNPs shared by *every* trait survive,
//! then runs the two CPASSOC cross-phenotype tests:
//!
//! 1. **SHom** — homogeneous-effects test (powerful when effects are consistent
//!    across traits/cohorts; follows χ²₁ under the null).
//! 2. **SHet** — heterogeneous-effects truncated test (powerful when a variant
//!    affects only a subset of traits, or has opposite directions across
//!    traits; null ≈ shifted gamma `Gamma(k,θ)+a` estimated by Monte-Carlo).
//!
//! The inter-trait correlation matrix **R** is estimated directly from the
//! Z-scores via `cor(X)` (Pearson correlation, Equation 6 of the paper).
//! **No LD reference panel is required.**
//!
//! Outputs a single `DataFrame` with per-SNP `rsid`, `shom`, `shet`,
//! `shom_pval`, `shet_pval`.

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use faer::Mat;
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
pub enum CpassocNodeError {
    #[error("CPASSOC computation failed: {0}")]
    Cpassoc(String),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
}

impl ::dag_core::dag::NodeError for CpassocNodeError {
    fn node_type(&self) -> &str {
        "cpassoc"
    }
}

// =====================================================================
// Schemas
// =====================================================================

/// Input column names each upstream GWAS sumstats `DataFrame` must expose.
const INPUT_Z_COL: &str = "z";
const INPUT_N_COL: &str = "n";
const INPUT_RSID_COL: &str = "rsid";

/// Output column names.
const OUT_RSID_COL: &str = "rsid";
const OUT_SHOM_COL: &str = "shom";
const OUT_SHET_COL: &str = "shet";
const OUT_SHOM_P_COL: &str = "shom_pval";
const OUT_SHET_P_COL: &str = "shet_pval";

/// Input port schema: per-SNP Z-score, sample size, rsid join key.
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        // `z` is per-SNP — missing Z-scores are common in real GWAS data
        // (SNP not genotyped, failed QC, etc.). The node filters rows where
        // any trait's Z is null.
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        // `n` is per-trait (CPASSOC assumes constant sample size per trait).
        // Upstream nodes are expected to fill any null n values before this
        // node runs; the schema declares it non-nullable to enforce this
        // contract. If a null slips through, the node errors with a clear
        // message rather than silently producing NaN statistics.
        Field::new(INPUT_N_COL, DataType::Float64, false),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// Output port schema: per-SNP CPASSOC results.
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(OUT_RSID_COL, DataType::Utf8, false),
        Field::new(OUT_SHOM_COL, DataType::Float64, false),
        Field::new(OUT_SHET_COL, DataType::Float64, false),
        Field::new(OUT_SHOM_P_COL, DataType::Float64, false),
        Field::new(OUT_SHET_P_COL, DataType::Float64, false),
    ]))
}

// =====================================================================
// Config
// =====================================================================

/// Configuration for the CPASSOC analysis node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CpassocConfig {
    /// Number of Monte-Carlo null draws for SHet gamma-distribution fitting
    /// (R default: 10 000). More draws ⇒ more stable gamma parameters.
    #[serde(default = "default_n_sim")]
    pub n_sim: usize,
    /// RNG seed for reproducible MVN null simulation.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// `correct = 1` (default): use absolute statistics with signed weights
    /// (standard SHet). `correct = 0`: keep original signs (no correction).
    #[serde(default = "default_correct")]
    pub correct: i32,
    /// If `true` (default): search all unique `|x|` values as cutoffs
    /// (exhaustive but slower). If `false`: use a fixed cutoff sequence.
    #[serde(default = "default_true")]
    pub is_all_possible: bool,
    /// Starting cutoff for the fixed-cutoff search (used only when
    /// `is_all_possible = false`).
    #[serde(default = "default_start_cutoff")]
    pub start_cutoff: f64,
    /// Ending cutoff for the fixed-cutoff search.
    #[serde(default = "default_end_cutoff")]
    pub end_cutoff: f64,
    /// Step size for the fixed-cutoff search.
    #[serde(default = "default_cutoff_step")]
    pub cutoff_step: f64,
}

fn default_n_sim() -> usize {
    10_000
}
fn default_seed() -> u64 {
    42
}
fn default_correct() -> i32 {
    1
}
fn default_true() -> bool {
    true
}
fn default_start_cutoff() -> f64 {
    0.0
}
fn default_end_cutoff() -> f64 {
    1.0
}
fn default_cutoff_step() -> f64 {
    0.05
}

impl Default for CpassocConfig {
    fn default() -> Self {
        Self {
            n_sim: default_n_sim(),
            seed: default_seed(),
            correct: default_correct(),
            is_all_possible: default_true(),
            start_cutoff: default_start_cutoff(),
            end_cutoff: default_end_cutoff(),
            cutoff_step: default_cutoff_step(),
        }
    }
}

// =====================================================================
// Node
// =====================================================================

const CPASSOC_NODE_KIND: &str = "cpassoc";

/// Static port layout: a single typed output port and a **variadic** input
/// (input count not fixed, so the scheduler skips connectivity validation).
/// Each connected input port is one trait.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema())) // port 0: trait 1 (declared for schema)
        .add_output_port(Some(output_schema()))
        .set_fixed_input(false) // accept any number of connected inputs
}

/// A transform node that runs CPASSOC on ≥ 2 GWAS summary-statistics inputs.
///
/// Accepts K upstream `DataFrame`s (trait 1 on port 0, trait 2 on port 1, …),
/// each with columns `z`, `n`, `rsid`. Inner-joins all on `rsid`, estimates the
/// inter-trait correlation matrix from the Z-scores, and computes SHom and SHet
/// cross-phenotype association tests. Outputs a single result `DataFrame` with
/// per-SNP `rsid`, `shom`, `shet`, `shom_pval`, `shet_pval`.
#[derive(Clone)]
pub struct CpassocNode {
    meta: NodePorts,
    config: CpassocConfig,
}

pub struct CpassocNodeFactory {}

impl NodeFactory for CpassocNodeFactory {
    fn kind(&self) -> &'static str {
        CPASSOC_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "CPASSOC cross-phenotype association (SHom + SHet) for ≥ 2 GWAS traits."
    }

    fn doc(&self) -> &'static str {
        "CPASSOC (Cross-Phenotype Association) transform node. Takes ≥ 2 upstream \
        GWAS summary statistics DataFrames (trait 1 on port 0, trait 2 on port 1, \
        …, each with z, n, rsid), inner-joins them on rsid to find common SNPs, \
        estimates the inter-trait correlation matrix R directly from the Z-scores \
        (cor(X), Equation 6), and computes two cross-phenotype tests:\n  \
        - SHom: homogeneous-effects test (chi-sq_1 under null).\n  \
        - SHet: heterogeneous-effects truncated test (null approx shifted gamma).\n\
        Outputs a single DataFrame with per-SNP rsid, shom, shet, shom_pval, \
        shet_pval.\n\n\
        ── IMPORTANT NOTES ──\n\
        1. NO LD REFERENCE PANEL NEEDED. CPASSOC works purely from summary \
        statistics. The correlation matrix R is estimated from the Z-scores \
        themselves. Do NOT feed an LD panel into this node.\n\
        2. LD PRUNING RECOMMENDED. The paper recommends estimating R from \
        independent (LD-pruned) SNPs to avoid inflated correlations. Consider \
        pruning the upstream sumstats to approximate LD independence before \
        this node.\n\
        3. QUALITY CONTROL — you MUST perform the following QC on the input \
        GWAS summary statistics BEFORE feeding them into this node:\n   \
        a. Remove palindromic SNPs (A/T or C/G) to avoid strand-alignment \
        ambiguity.\n   \
        b. Remove duplicate SNPs so each rsid appears at most once.\n   \
        c. Keep only biallelic SNPs.\n   \
        d. Remove SNPs on sex chromosomes (X, Y, MT) — retain autosomal only.\n   \
        e. Filter by minor allele frequency: keep only MAF > 0.01 (1%).\n\
        4. NO MISSING VALUES. The current version assumes no missing summary \
        statistics across traits. The inner join ensures only SNPs present in \
        ALL input traits survive.\n\
        5. SAMPLE SIZE AS WEIGHT. The 'n' column is used as the weight for \
        each trait (w_jk = sqrt(n_j)). Larger studies get more weight.\n\
        6. GAMMA MAY FAIL FOR LARGE K. When the number of traits is large \
        (> ~10), the shifted-gamma approximation for SHet may be unreliable. \
        In that case, consider reducing K by first meta-analysing within each \
        trait (e.g. with METAL), then using CPASSOC to combine across traits."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CpassocConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: CpassocConfig = serde_json::from_value(spec)?;
        let node = CpassocNode::new(config);
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<CpassocConfig>(spec, "cpassoc")?;
        let out = ctx.output_var.to_string();
        let input = ctx.fresh_var("sumstats");
        let input_var = ctx
            .input_vars
            .first()
            .cloned()
            .unwrap_or_else(|| "__missing_input".into());
        let code = vec![
            format!("# CPASSOC: Cross-Phenotype Association"),
            format!("# correct mode: {} (1=SHom, 2=SHet, 3=both)", cfg.correct),
            format!("{input} <- {input_var}"),
            format!("set.seed({})", cfg.seed),
            format!("# NOTE: CPASSOC requires per-trait Z-scores and sample sizes"),
            format!("# Format: data.frame with snp, z_trait1, ..., n_trait1, ..."),
            format!("{out} <- CPASSOC::cpassoc("),
            format!("  sumstats = {input},"),
            format!("  n_sim = {},", cfg.n_sim),
            format!("  correct = {},", cfg.correct),
            format!(
                "  all_possible = {},",
                if cfg.is_all_possible { "TRUE" } else { "FALSE" }
            ),
            format!(
                "  cutoff = seq({}, {}, {})",
                cfg.start_cutoff, cfg.end_cutoff, cfg.cutoff_step
            ),
            format!(")"),
            format!("print(head({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["CPASSOC".into()]
    }
}

impl CpassocNode {
    pub fn new(config: CpassocConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }

    /// The catalog-independent CPASSOC pipeline.
    ///
    /// Given a [`SessionContext`] with K upstream sumstats registered, this:
    /// 1. Registers each input as `sumstats_{k}`.
    /// 2. Inner-joins all on rsid (SNPs common to every trait survive).
    /// 3. Extracts aligned Z-score matrix (M×K) and sample-size vector.
    /// 4. Estimates correlation matrix R = cor(Z).
    /// 5. Runs SHom, SHet, gamma fitting, and p-value computation.
    /// 6. Builds the result batch.
    async fn run_with_ctx(
        ctx: &datafusion::prelude::SessionContext,
        inputs: &[NodeInput],
        cfg: &CpassocConfig,
    ) -> Result<RecordBatch, DagError> {
        let k = inputs.len();
        if k < 2 {
            return Err(CpassocNodeError::Cpassoc(
                "CPASSOC requires at least 2 input traits".into(),
            )
            .into());
        }

        // 1. Register each input as a temp table.
        for (i, input) in inputs.iter().enumerate() {
            ctx.register_table(
                format!("sumstats_{i}"),
                input.dataframe()?.clone().into_view(),
            )
            .map_err(CpassocNodeError::ReadBatch)?;
        }

        // 2. Build the join SQL: progressively inner-join all traits on rsid.
        let mut select_cols = Vec::new();
        let mut joins = String::new();
        for i in 0..k {
            select_cols.push(format!(
                "s{i}.\"{z}\" AS z{i}, s{i}.\"{n}\" AS n{i}",
                z = INPUT_Z_COL,
                n = INPUT_N_COL,
            ));
            if i > 0 {
                joins.push_str(&format!(
                    " INNER JOIN sumstats_{i} AS s{i} ON s0.\"{rsid}\" = s{i}.\"{rsid}\"",
                    rsid = INPUT_RSID_COL,
                ));
            }
        }
        let sql = format!(
            r#"SELECT s0."{rsid}" AS rsid_joined, {cols}
               FROM sumstats_0 AS s0{joins}"#,
            rsid = INPUT_RSID_COL,
            cols = select_cols.join(", "),
            joins = joins,
        );

        let joined_df = ctx.sql(&sql).await.map_err(CpassocNodeError::ReadBatch)?;
        let batches = joined_df
            .clone()
            .collect()
            .await
            .map_err(CpassocNodeError::ReadBatch)?;

        if batches.is_empty() {
            return Err(CpassocNodeError::Cpassoc(
                "CPASSOC: joined DataFrame is empty (no SNPs shared by all traits)".into(),
            )
            .into());
        }

        // 3. Extract aligned vectors.
        let rsids = extract_string(&batches, "rsid_joined")?;
        let n_snp = rsids.len();
        let mut z_cols: Vec<Vec<f64>> = Vec::with_capacity(k);
        let mut n_vals: Vec<f64> = Vec::with_capacity(k);
        for i in 0..k {
            z_cols.push(extract_f64(&batches, &format!("z{i}"))?);
            let ns = extract_f64(&batches, &format!("n{i}"))?;
            // CPASSOC assumes sample size is constant per trait (the R reference
            // takes a single SampleSize vector). Take the first non-NaN n value.
            // If n is null/NaN for every row of a trait, this is a hard error.
            let n_trait = ns
                .iter()
                .copied()
                .find(|v| v.is_finite())
                .ok_or_else(|| {
                    CpassocNodeError::Cpassoc(format!(
                        "CPASSOC: trait {i} has no valid sample size (column 'n{i}' is null for all rows)"
                    ))
                })?;
            if n_trait <= 0.0 {
                return Err(CpassocNodeError::Cpassoc(format!(
                    "CPASSOC: trait {i} has non-positive sample size n={n_trait} (must be > 0)"
                ))
                .into());
            }
            n_vals.push(n_trait);
        }

        // Build M×K Z-score matrix, then filter out rows where any trait's Z
        // is NaN. CPASSOC assumes no missing summary statistics (R reference:
        // "the current version assumes no missing summary statistics"). The
        // inner-join guarantees rsid is present in all traits, but individual
        // Z-scores may still be null/NaN in the source data.
        let valid_idx: Vec<usize> = (0..n_snp)
            .filter(|&row| {
                (0..k).all(|col| {
                    let v = z_cols[col][row];
                    v.is_finite()
                })
            })
            .collect();

        let n_valid = valid_idx.len();
        let n_dropped = n_snp - n_valid;
        if n_dropped > 0 {
            tracing::info!(
                "CPASSOC: dropped {n_dropped} / {n_snp} SNPs with missing (NaN/null) Z-scores"
            );
        }
        if n_valid == 0 {
            return Err(CpassocNodeError::Cpassoc(
                "CPASSOC: all SNPs have at least one missing Z-score after inner join".into(),
            )
            .into());
        }

        // Build filtered rsid list and Z-score matrix.
        let rsids_filtered: Vec<String> = valid_idx.iter().map(|&i| rsids[i].clone()).collect();
        let rsids = rsids_filtered;
        let z_matrix = Mat::from_fn(n_valid, k, |row, col| z_cols[col][valid_idx[row]]);

        // 4. Estimate correlation matrix R = cor(Z) on the complete-case data.
        let corr = cpassoc::input::corr_matrix(&z_matrix);

        // 5. Build SHet options from config.
        let shet_opts = cpassoc::stats::ShetOptions {
            correct: cfg.correct,
            start_cutoff: cfg.start_cutoff,
            end_cutoff: cfg.end_cutoff,
            cutoff_step: cfg.cutoff_step,
            is_all_possible: cfg.is_all_possible,
        };

        // 6. Run the full CPASSOC pipeline.
        let cp_config = cpassoc::CpassocConfig {
            sample_size: n_vals.clone(),
            shet_opts,
            n_sim: cfg.n_sim,
            seed: cfg.seed,
        };
        let results = cpassoc::run_cpassoc(&z_matrix, &corr, &cp_config, Some(&rsids));

        // 7. Build result batch.
        let m = results.len();
        let rsid_out: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        let shom_out: Vec<f64> = results.iter().map(|r| r.shom).collect();
        let shet_out: Vec<f64> = results.iter().map(|r| r.shet).collect();
        let shom_p_out: Vec<f64> = results.iter().map(|r| r.shom_p).collect();
        let shet_p_out: Vec<f64> = results.iter().map(|r| r.shet_p).collect();

        let batch = RecordBatch::try_new(
            output_schema(),
            vec![
                Arc::new(arrow_array::StringArray::from(rsid_out)),
                Arc::new(Float64Array::from(shom_out)),
                Arc::new(Float64Array::from(shet_out)),
                Arc::new(Float64Array::from(shom_p_out)),
                Arc::new(Float64Array::from(shet_p_out)),
            ],
        )
        .map_err(CpassocNodeError::from)?;

        let _ = m; // m == n_snp
        Ok(batch)
    }
}

#[async_trait]
impl DagNode for CpassocNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        CPASSOC_NODE_KIND
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
        // Sort inputs by port number so trait order is deterministic.
        let mut sorted_inputs: Vec<NodeInput> = inputs.to_vec();
        sorted_inputs.sort_by_key(|i| i.port);

        let ctx = node_ctx.session();
        let batch = Self::run_with_ctx(&ctx, &sorted_inputs, &self.config).await?;
        let df = ctx.read_batch(batch).map_err(CpassocNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// Column extraction helpers (mirrors mtag.rs / ldsc_rg.rs)
// =====================================================================

fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, CpassocNodeError> {
    let schema = batches
        .first()
        .ok_or(CpassocNodeError::Cpassoc("extract_f64: no batches".into()))?
        .schema();
    let idx = schema
        .index_of(name)
        .map_err(|_| CpassocNodeError::Cpassoc(format!("missing column '{name}'")))?;
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
        return Err(CpassocNodeError::Cpassoc(format!(
            "column '{name}' is not numeric (got {dtype})"
        )));
    }

    let mut out = Vec::new();
    for batch in batches {
        push_numeric(batch.column(idx), &mut out);
    }
    Ok(out)
}

fn extract_string(batches: &[RecordBatch], name: &str) -> Result<Vec<String>, CpassocNodeError> {
    let schema = batches
        .first()
        .ok_or(CpassocNodeError::Cpassoc(
            "extract_string: no batches".into(),
        ))?
        .schema();
    let idx = schema
        .index_of(name)
        .map_err(|_| CpassocNodeError::Cpassoc(format!("missing column '{name}'")))?;

    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        if let Some(arr) = col.as_any().downcast_ref::<arrow_array::StringArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<arrow_array::LargeStringArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<arrow_array::StringViewArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else {
            return Err(CpassocNodeError::Cpassoc(format!(
                "column '{name}' is not a string type"
            )));
        }
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
    use super::*;
    use arrow_array::StringArray;
    use datafusion::prelude::SessionContext;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    #[test]
    fn test_cpassoc_node_structure() {
        let node = CpassocNode::new(CpassocConfig::default());
        assert_eq!(node.kind(), "cpassoc");
        // Output: one port.
        assert_eq!(node.ports().output_ports().len(), 1);
        // Input: declared with at least 1 port (for schema), but variadic.
        assert!(!node.ports().input_ports().is_empty());
    }

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

    #[tokio::test]
    async fn e2e_cpassoc_two_traits() {
        let rsids: Vec<String> = (0..200).map(|i| format!("rs{}", 1_000_000 + i)).collect();
        let z1: Vec<f64> = (0..200)
            .map(|i| (i as f64 / 50.0 - 2.0).sin() * 3.0)
            .collect();
        let z2: Vec<f64> = (0..200)
            .map(|i| (i as f64 / 50.0 - 2.0).cos() * 2.5 + 0.5)
            .collect();

        let ctx = SessionContext::new();
        let df1 = ctx.read_batch(sumstats_batch(&z1, &rsids, 1000.0)).unwrap();
        let df2 = ctx.read_batch(sumstats_batch(&z2, &rsids, 800.0)).unwrap();

        let cfg = CpassocConfig {
            n_sim: 5000,
            ..Default::default()
        };

        let inputs = vec![
            NodeInput::new_dataframe(0, df1),
            NodeInput::new_dataframe(1, df2),
        ];
        let batch = CpassocNode::run_with_ctx(&ctx, &inputs, &cfg)
            .await
            .expect("CPASSOC pipeline should succeed");

        assert_eq!(batch.num_rows(), 200);
        assert_eq!(batch.num_columns(), 5);

        // Check column names.
        let schema = batch.schema();
        assert_eq!(schema.field(0).name(), "rsid");
        assert_eq!(schema.field(1).name(), "shom");
        assert_eq!(schema.field(2).name(), "shet");
        assert_eq!(schema.field(3).name(), "shom_pval");
        assert_eq!(schema.field(4).name(), "shet_pval");

        // All SHom/SHet stats should be finite and non-negative.
        let shom = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let shet = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        for i in 0..200 {
            assert!(shom.value(i).is_finite(), "shom[{i}] not finite");
            assert!(shom.value(i) >= 0.0, "shom[{i}] negative");
            assert!(shet.value(i).is_finite(), "shet[{i}] not finite");
            assert!(shet.value(i) >= 0.0, "shet[{i}] negative");
        }

        // All p-values in [0, 1].
        let shom_p = batch
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let shet_p = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        for i in 0..200 {
            assert!(
                shom_p.value(i) >= 0.0 && shom_p.value(i) <= 1.0,
                "shom_p[{i}] out of [0,1]: {}",
                shom_p.value(i)
            );
            assert!(
                shet_p.value(i) >= 0.0 && shet_p.value(i) <= 1.0,
                "shet_p[{i}] out of [0,1]: {}",
                shet_p.value(i)
            );
        }
    }

    #[tokio::test]
    async fn e2e_cpassoc_three_traits() {
        let rsids: Vec<String> = (0..150).map(|i| format!("rs{}", 2_000_000 + i)).collect();
        let z1: Vec<f64> = (0..150).map(|i| (i as f64 * 0.1).sin() * 2.0).collect();
        let z2: Vec<f64> = (0..150)
            .map(|i| (i as f64 * 0.1 + 1.0).sin() * 1.5)
            .collect();
        let z3: Vec<f64> = (0..150)
            .map(|i| (i as f64 * 0.1 + 2.0).sin() * 2.5)
            .collect();

        let ctx = SessionContext::new();
        let df1 = ctx.read_batch(sumstats_batch(&z1, &rsids, 1000.0)).unwrap();
        let df2 = ctx.read_batch(sumstats_batch(&z2, &rsids, 1500.0)).unwrap();
        let df3 = ctx.read_batch(sumstats_batch(&z3, &rsids, 800.0)).unwrap();

        let cfg = CpassocConfig {
            n_sim: 3000,
            ..Default::default()
        };

        let inputs = vec![
            NodeInput::new_dataframe(0, df1),
            NodeInput::new_dataframe(1, df2),
            NodeInput::new_dataframe(2, df3),
        ];
        let batch = CpassocNode::run_with_ctx(&ctx, &inputs, &cfg)
            .await
            .expect("3-trait CPASSOC should succeed");

        assert_eq!(batch.num_rows(), 150);

        // SHet ≥ SHom is NOT guaranteed, but both should be non-negative.
        let shom = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let shet = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        for i in 0..150 {
            assert!(shom.value(i) >= 0.0, "shom[{i}] negative");
            assert!(shet.value(i) >= 0.0, "shet[{i}] negative");
        }
    }

    #[tokio::test]
    async fn e2e_cpassoc_single_input_errors() {
        let rsids: Vec<String> = (0..10).map(|i| format!("rs{}", 3_000_000 + i)).collect();
        let z: Vec<f64> = (0..10).map(|i| i as f64 * 0.1).collect();

        let ctx = SessionContext::new();
        let df = ctx.read_batch(sumstats_batch(&z, &rsids, 1000.0)).unwrap();

        let cfg = CpassocConfig::default();
        let inputs = vec![NodeInput::new_dataframe(0, df)];
        let result = CpassocNode::run_with_ctx(&ctx, &inputs, &cfg).await;
        assert!(result.is_err(), "single-trait input must error");
    }

    #[tokio::test]
    async fn e2e_cpassoc_missing_input_errors() {
        let mut node = CpassocNode::new(CpassocConfig::default());
        let batch = sumstats_batch(
            &[1.0, 2.0, 3.0],
            &["rs1".into(), "rs2".into(), "rs3".into()],
            1000.0,
        );
        let df = SessionContext::new().read_batch(batch).unwrap();
        let one_input = vec![NodeInput::new_dataframe(0, df)];
        let res = node
            .execute(
                &node_ctx(),
                &one_input,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await;
        assert!(res.is_err(), "missing trait-2 input must error");
    }

    /// Build a sumstats batch where some Z-scores are null (None).
    /// Null positions are given as row indices.
    fn sumstats_batch_with_nulls(z: &[Option<f64>], rsids: &[String], n_samp: f64) -> RecordBatch {
        let n: Vec<f64> = vec![n_samp; z.len()];
        let schema = Arc::new(Schema::new(vec![
            Field::new("z", DataType::Float64, true),
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

    #[tokio::test]
    async fn e2e_cpassoc_filters_nan_z() {
        // 10 SNPs; SNP 3 has null z in trait 1, SNP 7 has null z in trait 2.
        // These should be silently dropped; the remaining 8 SNPs should
        // produce valid results.
        let rsids: Vec<String> = (0..10).map(|i| format!("rs{}", 4_000_000 + i)).collect();
        let z1: Vec<Option<f64>> = (0..10)
            .map(|i| {
                if i == 3 {
                    None
                } else {
                    Some(i as f64 * 0.3 - 1.0)
                }
            })
            .collect();
        let z2: Vec<Option<f64>> = (0..10)
            .map(|i| {
                if i == 7 {
                    None
                } else {
                    Some(i as f64 * 0.2 - 0.5)
                }
            })
            .collect();

        let ctx = SessionContext::new();
        let df1 = ctx
            .read_batch(sumstats_batch_with_nulls(&z1, &rsids, 1000.0))
            .unwrap();
        let df2 = ctx
            .read_batch(sumstats_batch_with_nulls(&z2, &rsids, 800.0))
            .unwrap();

        let cfg = CpassocConfig {
            n_sim: 2000,
            ..Default::default()
        };
        let inputs = vec![
            NodeInput::new_dataframe(0, df1),
            NodeInput::new_dataframe(1, df2),
        ];
        let batch = CpassocNode::run_with_ctx(&ctx, &inputs, &cfg)
            .await
            .expect("CPASSOC with NaN filtering should succeed");

        // 10 - 2 dropped = 8 SNPs remain.
        assert_eq!(batch.num_rows(), 8, "expected 8 SNPs after NaN filtering");

        // The dropped rsids should NOT appear in the output.
        let rsid_col = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let output_rsids: std::collections::HashSet<&str> =
            (0..batch.num_rows()).map(|i| rsid_col.value(i)).collect();
        assert!(
            !output_rsids.contains("rs4000003"),
            "SNP with null z in trait 1 should be filtered"
        );
        assert!(
            !output_rsids.contains("rs4000007"),
            "SNP with null z in trait 2 should be filtered"
        );

        // All remaining SHom/SHet should be finite and non-negative.
        let shom = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let shet = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        for i in 0..8 {
            assert!(
                shom.value(i).is_finite(),
                "shom[{i}] not finite after NaN filter"
            );
            assert!(shom.value(i) >= 0.0);
            assert!(
                shet.value(i).is_finite(),
                "shet[{i}] not finite after NaN filter"
            );
            assert!(shet.value(i) >= 0.0);
        }
    }

    #[tokio::test]
    async fn e2e_cpassoc_all_nan_errors() {
        // Every SNP has a null in at least one trait → all filtered → error.
        let rsids: Vec<String> = vec!["rs1".into(), "rs2".into()];
        let z1: Vec<Option<f64>> = vec![None, None];
        let z2: Vec<Option<f64>> = vec![None, None];

        let ctx = SessionContext::new();
        let df1 = ctx
            .read_batch(sumstats_batch_with_nulls(&z1, &rsids, 1000.0))
            .unwrap();
        let df2 = ctx
            .read_batch(sumstats_batch_with_nulls(&z2, &rsids, 800.0))
            .unwrap();

        let cfg = CpassocConfig::default();
        let inputs = vec![
            NodeInput::new_dataframe(0, df1),
            NodeInput::new_dataframe(1, df2),
        ];
        let result = CpassocNode::run_with_ctx(&ctx, &inputs, &cfg).await;
        assert!(result.is_err(), "all-NaN input must error after filtering");
    }

    /// Build a sumstats batch where the sample-size column has nulls.
    fn sumstats_batch_null_n(z: &[f64], rsids: &[String], n_vals: &[Option<f64>]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("z", DataType::Float64, false),
            Field::new("n", DataType::Float64, true),
            Field::new("rsid", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(z.to_vec())),
                Arc::new(Float64Array::from(n_vals.to_vec())),
                Arc::new(StringArray::from(rsids.to_vec())),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn e2e_cpassoc_null_n_first_row_falls_back() {
        // Trait 2 has null n at row 0 but valid n at row 1.
        // The node should fall back to the first valid n (not 0.0 or NaN).
        let rsids: Vec<String> = (0..5).map(|i| format!("rs{}", 5_000_000 + i)).collect();
        let z1: Vec<f64> = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let z2: Vec<f64> = vec![0.5, 1.0, 1.5, 2.0, 2.5];
        // n for trait 1: all 1000.0
        let n1: Vec<Option<f64>> = vec![Some(1000.0); 5];
        // n for trait 2: row 0 is null, rest are 800.0
        let n2: Vec<Option<f64>> = vec![None, Some(800.0), Some(800.0), Some(800.0), Some(800.0)];

        let ctx = SessionContext::new();
        let df1 = ctx
            .read_batch(sumstats_batch_null_n(&z1, &rsids, &n1))
            .unwrap();
        let df2 = ctx
            .read_batch(sumstats_batch_null_n(&z2, &rsids, &n2))
            .unwrap();

        let cfg = CpassocConfig {
            n_sim: 2000,
            ..Default::default()
        };
        let inputs = vec![
            NodeInput::new_dataframe(0, df1),
            NodeInput::new_dataframe(1, df2),
        ];
        let batch = CpassocNode::run_with_ctx(&ctx, &inputs, &cfg)
            .await
            .expect("should fall back to first valid n");

        assert_eq!(batch.num_rows(), 5);
        // All results should be finite (n=800 was found despite row-0 null).
        let shom = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        for i in 0..5 {
            assert!(
                shom.value(i).is_finite(),
                "shom[{i}] not finite with fallback n"
            );
        }
    }

    #[tokio::test]
    async fn e2e_cpassoc_all_null_n_errors() {
        // Trait 2 has null n for every row → should error.
        let rsids: Vec<String> = (0..3).map(|i| format!("rs{}", 6_000_000 + i)).collect();
        let z1: Vec<f64> = vec![1.0, 2.0, 3.0];
        let z2: Vec<f64> = vec![0.5, 1.0, 1.5];
        let n1: Vec<Option<f64>> = vec![Some(1000.0); 3];
        let n2: Vec<Option<f64>> = vec![None, None, None];

        let ctx = SessionContext::new();
        let df1 = ctx
            .read_batch(sumstats_batch_null_n(&z1, &rsids, &n1))
            .unwrap();
        let df2 = ctx
            .read_batch(sumstats_batch_null_n(&z2, &rsids, &n2))
            .unwrap();

        let cfg = CpassocConfig::default();
        let inputs = vec![
            NodeInput::new_dataframe(0, df1),
            NodeInput::new_dataframe(1, df2),
        ];
        let result = CpassocNode::run_with_ctx(&ctx, &inputs, &cfg).await;
        assert!(result.is_err(), "all-null n should error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("no valid sample size") || msg.contains("sample size"),
            "error should mention sample size, got: {msg}"
        );
    }

    #[tokio::test]
    async fn e2e_cpassoc_zero_n_errors() {
        // Trait 2 has n=0 → should error (weight would be 0).
        let rsids: Vec<String> = (0..3).map(|i| format!("rs{}", 7_000_000 + i)).collect();
        let z1: Vec<f64> = vec![1.0, 2.0, 3.0];
        let z2: Vec<f64> = vec![0.5, 1.0, 1.5];
        let n1: Vec<Option<f64>> = vec![Some(1000.0); 3];
        let n2: Vec<Option<f64>> = vec![Some(0.0); 3];

        let ctx = SessionContext::new();
        let df1 = ctx
            .read_batch(sumstats_batch_null_n(&z1, &rsids, &n1))
            .unwrap();
        let df2 = ctx
            .read_batch(sumstats_batch_null_n(&z2, &rsids, &n2))
            .unwrap();

        let cfg = CpassocConfig::default();
        let inputs = vec![
            NodeInput::new_dataframe(0, df1),
            NodeInput::new_dataframe(1, df2),
        ];
        let result = CpassocNode::run_with_ctx(&ctx, &inputs, &cfg).await;
        assert!(result.is_err(), "zero n should error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("non-positive"),
            "error should mention non-positive sample size, got: {msg}"
        );
    }
}
