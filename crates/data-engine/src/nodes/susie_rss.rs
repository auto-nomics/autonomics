//! SuSiE-RSS DAG node — Bayesian fine-mappinging from GWAS summary statistics.
//!
//! Consumes GWAS z-scores (or bhat/shat) from the upstream port, reads the LD
//! correlation matrix from the Iceberg `ld_matrix.eur_chr{N}` panel tables,
//! runs [`susie::susie_rss`], and emits per-variant posterior inclusion
//! probabilities (PIPs), credible-set membership, and posterior moments.
//!
//! ```text
//! GWAS sumstats ──▶ susie_rss ──▶ fine-mappinging results
//!   (snp, z, n, chrom)
//! ```
//!
//! The LD matrix in Iceberg stores `unphased_r2` (squared correlations).
//! Signed correlations are reconstructed as `sign(z_j × z_k) × √(r²_jk)`,
//! the standard approximation when only r² is available from PLINK `--r2`.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::dag::runtime::RuntimeStatus;
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
};

// ─── errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum SusieNodeError {
    #[error("SuSiE computation failed: {0}")]
    Susie(String),
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("datafusion error: {0}")]
    Df(#[from] datafusion::error::DataFusionError),
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
    #[error("no SNPs overlapped between sumstats and LD panel")]
    NoLdOverlap,
}

impl From<SusieNodeError> for DagError {
    fn from(e: SusieNodeError) -> Self {
        DagError::NodeError {
            node_type: "susie_rss".into(),
            msg: e.to_string(),
        }
    }
}

fn missing(name: &str) -> SusieNodeError {
    SusieNodeError::MissingColumn { name: name.into() }
}

// ─── input / output schemas ──────────────────────────────────────────────────

/// Input: GWAS summary statistics.
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("snp", DataType::Utf8, false),
        Field::new("chrom", DataType::Int64, true),
        Field::new("z", DataType::Float64, true),
        Field::new("n", DataType::Float64, true),
        Field::new("a1", DataType::Utf8, true),
        Field::new("a2", DataType::Utf8, true),
    ]))
}

/// Output: fine-mappinging results, one row per variant.
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("snp", DataType::Utf8, false),
        Field::new("pip", DataType::Float64, false),
        Field::new("cs", DataType::Int64, false),  // 0 = not in CS, 1-based CS index
        Field::new("alpha", DataType::Float64, true),  // max alpha across effects
        Field::new("mu", DataType::Float64, true),
        Field::new("mu2", DataType::Float64, true),
        Field::new("lbf", DataType::Float64, true),  // per-variant log Bayes factor (top effect)
    ]))
}

// ─── spec ────────────────────────────────────────────────────────────────────

/// Spec for [`SusieRssNode`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SusieRssSpec {
    /// Maximum number of non-zero effects (SuSiE L parameter). Default 10.
    #[serde(default = "default_l")]
    pub l: usize,
    /// Prior-variance optimization method: "optim" (Brent), "EM", or "simple".
    #[serde(default = "default_method")]
    pub estimate_prior_method: String,
    /// Estimate residual variance each IBSS iteration? Default false.
    #[serde(default = "default_false")]
    pub estimate_residual_variance: bool,
    /// Estimate prior variance per effect? Default true.
    #[serde(default = "default_true")]
    pub estimate_prior_variance: bool,
    /// Coverage for credible sets. Default 0.95.
    #[serde(default = "default_coverage")]
    pub coverage: f64,
    /// Min |corr| purity threshold for credible sets. Default 0.5.
    #[serde(default = "default_min_abs_corr")]
    pub min_abs_corr: f64,
    /// Scaled prior variance (initial value if estimate_prior_variance=true).
    #[serde(default = "default_spv")]
    pub scaled_prior_variance: f64,
    /// z-score method: "wald" (PVE-adjusted) or "score" (already on σ²=1 scale).
    #[serde(default = "default_z_method")]
    pub z_method: String,
    /// Minimum r² threshold for LD pairs from the Iceberg panel. Default 0.0
    /// (all pairs used). Set higher (e.g. 0.05) to sparsify large regions.
    #[serde(default = "default_r2_min")]
    pub r2_min: f64,
    /// Sample size override. If None, reads `n` from the input column.
    #[serde(default)]
    pub n: Option<f64>,
    /// Check-null threshold: if loglik(0) + this >= loglik(V̂), set V=0.
    #[serde(default = "default_check_null_threshold")]
    pub check_null_threshold: f64,
    /// Max IBSS iterations. Default 100.
    #[serde(default = "default_max_iter")]
    pub max_iter: usize,
}

fn default_l() -> usize { 10 }
fn default_method() -> String { "optim".into() }
fn default_false() -> bool { false }
fn default_true() -> bool { true }
fn default_coverage() -> f64 { 0.95 }
fn default_min_abs_corr() -> f64 { 0.5 }
fn default_spv() -> f64 { 0.2 }
fn default_z_method() -> String { "wald".into() }
fn default_r2_min() -> f64 { 0.0 }
fn default_check_null_threshold() -> f64 { 0.0 }
fn default_max_iter() -> usize { 100 }

// ─── node ────────────────────────────────────────────────────────────────────

const NODE_KIND: &str = "susie_rss";

#[derive(Clone)]
pub struct SusieRssNode {
    meta: NodePorts,
    spec: SusieRssSpec,
}

impl SusieRssNode {
    pub fn new(spec: SusieRssSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(Some(input_schema()))
                .add_output_port(Some(output_schema())),
            spec,
        }
    }
}

pub struct SusieRssNodeFactory {}
impl NodeFactory for SusieRssNodeFactory {
    fn kind(&self) -> &'static str {
        NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "SuSiE-RSS: Bayesian fine-mappinging from GWAS z-scores + Iceberg LD panel."
    }
    fn doc(&self) -> &'static str {
        "Reads LD correlations from iceberg.ld_matrix.eur_chr{N}, runs susie_rss, \
         and outputs PIPs, credible-set membership, and posterior moments."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SusieRssSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(Some(input_schema()))
            .add_output_port(Some(output_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(SusieRssNode::new(serde_json::from_value(spec)?)))
    }
}

// ─── column extractors ───────────────────────────────────────────────────────

fn col_str(batches: &[RecordBatch], name: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        for v in super::meta::string_opt_values(col.as_ref())? {
            out.push(v.unwrap_or_default());
        }
    }
    Some(out)
}

fn col_f64(batches: &[RecordBatch], name: &str) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        for i in 0..col.len() {
            out.push(if col.is_null(i) {
                f64::NAN
            } else {
                arr_f64(col.as_ref(), i)
            });
        }
    }
    Some(out)
}

fn col_i64(batches: &[RecordBatch], name: &str) -> Option<Vec<i64>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        for i in 0..col.len() {
            out.push(if col.is_null(i) { 0 } else { arr_i64(col.as_ref(), i) });
        }
    }
    Some(out)
}

fn arr_f64(arr: &dyn Array, i: usize) -> f64 {
    use arrow_array::*;
    if let Some(a) = arr.as_any().downcast_ref::<Float64Array>() {
        return a.value(i);
    }
    if let Some(a) = arr.as_any().downcast_ref::<Float32Array>() {
        return a.value(i) as f64;
    }
    if let Some(a) = arr.as_any().downcast_ref::<Int64Array>() {
        return a.value(i) as f64;
    }
    if let Some(a) = arr.as_any().downcast_ref::<Int32Array>() {
        return a.value(i) as f64;
    }
    f64::NAN
}

fn arr_i64(arr: &dyn Array, i: usize) -> i64 {
    use arrow_array::*;
    if let Some(a) = arr.as_any().downcast_ref::<Int64Array>() {
        return a.value(i);
    }
    if let Some(a) = arr.as_any().downcast_ref::<Int32Array>() {
        return a.value(i) as i64;
    }
    if let Some(a) = arr.as_any().downcast_ref::<Float64Array>() {
        return a.value(i) as i64;
    }
    0
}

async fn collect_input_batches(input: &NodeInput) -> Result<Vec<RecordBatch>, DagError> {
    let batches: Vec<RecordBatch> = input.data.clone().collect().await.map_err(|e| {
        DagError::NodeError {
            node_type: NODE_KIND.into(),
            msg: format!("collect failed: {e}"),
        }
    })?;
    if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
        return Err(SusieNodeError::EmptyInput.into());
    }
    Ok(batches)
}

// ─── LD matrix loading from Iceberg ──────────────────────────────────────────

/// LD pair from the Iceberg `ld_matrix` table.
struct LdPair {
    id_a: String,
    id_b: String,
    r2: f64,
}

/// Query the Iceberg LD matrix for one chromosome and return all pairs with
/// r² ≥ `r2_min` that involve at least one SNP in `snp_set`.
async fn load_ld_pairs(
    ctx: &datafusion::prelude::SessionContext,
    chrom: i64,
    r2_min: f64,
    snp_set: &std::collections::HashSet<String>,
) -> Result<Vec<LdPair>, SusieNodeError> {
    let sql = format!(
        "SELECT id_a, id_b, unphased_r2 \
         FROM iceberg.ld_matrix.eur_chr{chrom} \
         WHERE unphased_r2 >= {r2_min}"
    );
    let df = ctx.sql(&sql).await.map_err(SusieNodeError::Df)?;
    let batches = df.collect().await.map_err(SusieNodeError::Df)?;

    let mut pairs = Vec::new();
    for batch in &batches {
        let a_ids = batch
            .column_by_name("id_a")
            .ok_or_else(|| missing("id_a"))?;
        let b_ids = batch
            .column_by_name("id_b")
            .ok_or_else(|| missing("id_b"))?;
        let r2s = batch
            .column_by_name("unphased_r2")
            .ok_or_else(|| missing("unphased_r2"))?;

        for i in 0..batch.num_rows() {
            if r2s.is_null(i) {
                continue;
            }
            let r2 = arr_f64(r2s.as_ref(), i);
            let a = super::meta::string_opt_values(a_ids.as_ref())
                .and_then(|v| v.get(i).cloned().flatten())
                .unwrap_or_default();
            let b = super::meta::string_opt_values(b_ids.as_ref())
                .and_then(|v| v.get(i).cloned().flatten())
                .unwrap_or_default();
            // Only keep pairs where at least one endpoint is in our SNP set.
            // The other endpoint may or may not be in the set.
            if snp_set.contains(&a) || snp_set.contains(&b) {
                pairs.push(LdPair { id_a: a, id_b: b, r2 });
            }
        }
    }
    Ok(pairs)
}

/// Build a p×p correlation matrix R from LD r² pairs and z-scores.
///
/// R[j,j] = 1.0 for all variants.
/// R[j,k] = sign(z_j × z_k) × √(r²_jk) for off-diagonal entries.
///
/// The sign approximation uses z-score concordance: if two variants have the
/// same z-score sign, they are positively correlated; opposite signs imply
/// negative correlation.
fn build_corr_matrix(
    snps: &[String],
    z: &[f64],
    pairs: &[LdPair],
) -> Mat<f64> {
    let p = snps.len();
    let mut r = Mat::zeros(p, p);

    // rsid → index map
    let mut idx: HashMap<&str, usize> = HashMap::with_capacity(p);
    for (i, s) in snps.iter().enumerate() {
        idx.insert(s.as_str(), i);
        r[(i, i)] = 1.0; // diagonal = 1
    }

    for pair in pairs {
        let (Some(&a), Some(&b)) = (idx.get(pair.id_a.as_str()), idx.get(pair.id_b.as_str()))
        else {
            continue;
        };
        if a == b {
            continue;
        }
        let abs_r = pair.r2.sqrt();
        // Sign from z-score concordance
        let za = z[a];
        let zb = z[b];
        let sign = if za == 0.0 || zb == 0.0 {
            1.0 // neutral → positive
        } else if (za > 0.0) == (zb > 0.0) {
            1.0
        } else {
            -1.0
        };
        let val = sign * abs_r;
        r[(a, b)] = val;
        r[(b, a)] = val;
    }

    r
}

// ─── execute ─────────────────────────────────────────────────────────────────

#[async_trait]
impl DagNode for SusieRssNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        NODE_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        reporter.status(RuntimeStatus::Running);

        // ── collect input sumstats ──
        let input = inputs.first().ok_or_else(|| {
            reporter.error("susie_rss: abort — no input port connected");
            SusieNodeError::EmptyInput
        })?;
        let batches = collect_input_batches(input).await?;
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        reporter.info(format!("susie_rss: collected {total_rows} sumstat rows"));

        let snp = col_str(&batches, "snp").ok_or_else(|| {
            reporter.error("susie_rss: abort — missing 'snp' column");
            missing("snp")
        })?;
        let z = col_f64(&batches, "z").ok_or_else(|| {
            reporter.error("susie_rss: abort — missing 'z' column");
            missing("z")
        })?;
        let n_col = col_f64(&batches, "n");
        let chrom_col = col_i64(&batches, "chrom");

        // Determine sample size
        let n = self.spec.n.or_else(|| {
            n_col.as_ref().and_then(|col| {
                // Use the first non-NAN n value
                col.iter()
                    .find(|&&v| !v.is_nan() && v > 1.0)
                    .copied()
            })
        });

        // ── determine chromosome for LD lookup ──
        let chrom = chrom_col
            .as_ref()
            .and_then(|col| col.iter().find(|&&c| c > 0).copied())
            .unwrap_or(0);

        if chrom == 0 {
            return Err(SusieNodeError::Susie(
                "cannot determine chromosome from input (missing or zero 'chrom' column)".into(),
            )
            .into());
        }
        reporter.info(format!(
            "susie_rss: chromosome {chrom}, {} variants, n={:?}",
            snp.len(),
            n
        ));

        // ── filter out NaN z-scores ──
        let mut keep: Vec<usize> = Vec::with_capacity(snp.len());
        for i in 0..snp.len() {
            if z[i].is_finite() {
                keep.push(i);
            }
        }
        if keep.is_empty() {
            return Err(SusieNodeError::EmptyInput.into());
        }

        let p = keep.len();
        let snps_filt: Vec<String> = keep.iter().map(|&i| snp[i].clone()).collect();
        let z_filt: Vec<f64> = keep.iter().map(|&i| z[i]).collect();
        reporter.info(format!("susie_rss: {p} variants with valid z-scores"));

        // ── load LD from Iceberg and build correlation matrix ──
        let ctx = node_ctx.session();
        let snp_set: std::collections::HashSet<String> = snps_filt.iter().cloned().collect();
        reporter.info(format!(
            "susie_rss: querying LD matrix iceberg.ld_matrix.eur_chr{chrom} (r² ≥ {})…",
            self.spec.r2_min
        ));
        let ld_pairs = load_ld_pairs(&ctx, chrom, self.spec.r2_min, &snp_set).await?;
        reporter.info(format!("susie_rss: loaded {} LD pairs", ld_pairs.len()));

        let r = build_corr_matrix(&snps_filt, &z_filt, &ld_pairs);

        // Check LD overlap
        let n_offdiag: usize = (0..p)
            .flat_map(|i| (0..p).map(move |j| (i, j)))
            .filter(|&(i, j)| i != j && r[(i, j)] != 0.0)
            .count();
        if p > 1 && n_offdiag == 0 {
            reporter.warn("susie_rss: no LD pairs overlapped with input SNPs — R is diagonal");
        }

        // ── build susie input and run ──
        let prior_method = match self.spec.estimate_prior_method.as_str() {
            "EM" | "em" => susie::PriorMethod::Em,
            "simple" => susie::PriorMethod::Simple,
            _ => susie::PriorMethod::Optim,
        };
        let z_method = if self.spec.z_method == "score" {
            susie::ZMethod::Score
        } else {
            susie::ZMethod::Wald
        };

        let susie_input = susie::RssInput {
            z: Some(z_filt),
            r,
            n,
            l: self.spec.l,
            scaled_prior_variance: self.spec.scaled_prior_variance,
            estimate_residual_variance: self.spec.estimate_residual_variance,
            estimate_prior_variance: self.spec.estimate_prior_variance,
            estimate_prior_method: prior_method,
            z_method,
            coverage: self.spec.coverage,
            min_abs_corr: Some(self.spec.min_abs_corr),
            check_null_threshold: self.spec.check_null_threshold,
            max_iter: self.spec.max_iter,
            ..Default::default()
        };

        reporter.info(format!(
            "susie_rss: running susie_rss (L={}, method={:?}, estimate_residual_variance={}, …)",
            self.spec.l, prior_method, self.spec.estimate_residual_variance
        ));

        let fit = susie::susie_rss(&susie_input).map_err(|e| {
            let msg = e.to_string();
            reporter.error(format!("susie_rss: abort — {msg}"));
            DagError::NodeError {
                node_type: NODE_KIND.into(),
                msg,
            }
        })?;

        reporter.info(format!(
            "susie_rss: done — niter={}, converged={}, {} credible sets, {} CSs after purity",
            fit.niter,
            fit.converged,
            fit.sets.sets.len(),
            fit.sets.sets.len(),
        ));

        // ── build output: per-variant results ──
        // Determine CS membership: for each CS (1-based), mark its variables.
        let mut cs_membership: Vec<i64> = vec![0; p]; // 0 = not in any CS
        for (cs_idx, cs) in fit.sets.sets.iter().enumerate() {
            for &var_idx in &cs.variables {
                if var_idx < p {
                    cs_membership[var_idx] = (cs_idx + 1) as i64;
                }
            }
        }

        // For each variant, find the max alpha across effects (and corresponding mu/mu2)
        let l = fit.alpha.len();
        let mut max_alpha: Vec<Option<f64>> = vec![None; p];
        let mut best_mu: Vec<Option<f64>> = vec![None; p];
        let mut best_mu2: Vec<Option<f64>> = vec![None; p];
        let mut best_lbf_var: Vec<Option<f64>> = vec![None; p];
        for j in 0..p {
            let mut best_l = 0usize;
            let mut best_a = -1.0_f64;
            for ll in 0..l {
                if fit.alpha[ll][j] > best_a {
                    best_a = fit.alpha[ll][j];
                    best_l = ll;
                }
            }
            if best_a > 0.0 {
                max_alpha[j] = Some(best_a);
                best_mu[j] = Some(fit.mu[best_l][j]);
                best_mu2[j] = Some(fit.mu2[best_l][j]);
                if best_l < fit.lbf_variable.len() {
                    best_lbf_var[j] = Some(fit.lbf_variable[best_l][j]);
                }
            }
        }

        let batch = RecordBatch::try_new(
            output_schema(),
            vec![
                Arc::new(StringArray::from(snps_filt)),
                Arc::new(Float64Array::from(fit.pip)),
                Arc::new(Int64Array::from(cs_membership)),
                Arc::new(Float64Array::from(max_alpha)),
                Arc::new(Float64Array::from(best_mu)),
                Arc::new(Float64Array::from(best_mu2)),
                Arc::new(Float64Array::from(best_lbf_var)),
            ],
        )
        .map_err(SusieNodeError::Arrow)?;

        let df = ctx.read_batch(batch).map_err(SusieNodeError::Df)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_metadata() {
        let f = SusieRssNodeFactory {};
        assert_eq!(f.kind(), NODE_KIND);
        let _ = f.spec_schema();
        let _ = f.ports();
        let _ = f.desc();
        let _ = f.doc();
    }

    #[test]
    fn node_constructs() {
        let spec = SusieRssSpec {
            l: 10,
            estimate_prior_method: "optim".into(),
            estimate_residual_variance: false,
            estimate_prior_variance: true,
            coverage: 0.95,
            min_abs_corr: 0.5,
            scaled_prior_variance: 0.2,
            z_method: "wald".into(),
            r2_min: 0.0,
            n: None,
            check_null_threshold: 0.0,
            max_iter: 100,
        };
        let node = SusieRssNode::new(spec);
        assert_eq!(node.kind(), NODE_KIND);
        assert_eq!(node.ports().input_ports().len(), 1);
        assert_eq!(node.ports().output_ports().len(), 1);
    }

    #[test]
    fn build_corr_matrix_signs() {
        let snps = vec!["rs1".into(), "rs2".into(), "rs3".into()];
        let z = vec![5.0, 4.0, -3.0];
        let pairs = vec![
            LdPair { id_a: "rs1".into(), id_b: "rs2".into(), r2: 0.81 },
            LdPair { id_a: "rs1".into(), id_b: "rs3".into(), r2: 0.64 },
        ];
        let r = build_corr_matrix(&snps, &z, &pairs);
        // rs1-rs2: same sign → positive correlation
        assert!((r[(0, 1)] - 0.9).abs() < 1e-10);
        // rs1-rs3: opposite sign → negative correlation
        assert!((r[(0, 2)] - (-0.8)).abs() < 1e-10);
        // diagonal = 1
        assert!((r[(1, 1)] - 1.0).abs() < 1e-10);
    }
}
