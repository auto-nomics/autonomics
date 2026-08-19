//! SuSiE-RSS DAG node — Bayesian fine-mapping from GWAS summary statistics.
//!
//! Consumes GWAS z-scores (or bhat/shat) from the upstream port, resolves signed
//! LD correlations from a versioned gsa-MiXeR reference panel, runs
//! [`susie::susie_rss`], and emits per-variant posterior inclusion
//! probabilities (PIPs), credible-set membership, and posterior moments.
//!
//! ```text
//! GWAS sumstats ──▶ susie_rss ──▶ fine-mappinging results
//!   (snp, z, n, chrom)
//! ```
//!
//! The reference stores signed Pearson correlations. LD direction is never
//! inferred from GWAS z-score signs.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::dag::runtime::RuntimeStatus;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
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
    #[error("SuSiE reference bundle error: {0}")]
    ReferenceBundle(String),
    #[error("signed LD query failed: {0}")]
    LdQuery(String),
}

impl ::dag_core::dag::NodeError for SusieNodeError {
    fn node_type(&self) -> &str {
        "susie_rss"
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
        Field::new("cs", DataType::Int64, false), // 0 = not in CS, 1-based CS index
        Field::new("alpha", DataType::Float64, true), // max alpha across effects
        Field::new("mu", DataType::Float64, true),
        Field::new("mu2", DataType::Float64, true),
        Field::new("lbf", DataType::Float64, true), // per-variant log Bayes factor (top effect)
    ]))
}

// ─── spec ────────────────────────────────────────────────────────────────────

/// Spec for [`SusieRssNode`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SusieRssSpec {
    /// Deployed signed-LD reference bundle ID. Defaults to `g1000_eur`.
    #[serde(default = "default_reference")]
    pub reference: String,
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
    /// Minimum r² threshold for signed LD pairs. Default 0.0
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

fn default_l() -> usize {
    10
}
fn default_reference() -> String {
    crate::mixer_common::default_reference()
}
fn default_method() -> String {
    "optim".into()
}
fn default_false() -> bool {
    false
}
fn default_true() -> bool {
    true
}
fn default_coverage() -> f64 {
    0.95
}
fn default_min_abs_corr() -> f64 {
    0.5
}
fn default_spv() -> f64 {
    0.2
}
fn default_z_method() -> String {
    "wald".into()
}
fn default_r2_min() -> f64 {
    0.0
}
fn default_check_null_threshold() -> f64 {
    0.0
}
fn default_max_iter() -> usize {
    100
}

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
        "SuSiE-RSS: Bayesian fine-mapping from GWAS z-scores + signed LD panel."
    }
    fn doc(&self) -> &'static str {
        "Resolves a semantic signed-LD reference bundle, runs susie_rss, and \
         outputs PIPs, credible-set membership, and posterior moments."
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
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(SusieRssNode::new(serde_json::from_value(spec)?)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<SusieRssSpec>(spec, "susie_rss")?;
        let out = ctx.output_var.to_string();
        let z = ctx.fresh_var("z_scores");
        let r_mat = ctx.fresh_var("ref_ld");
        let input = input_0(ctx).to_string();
        let n_arg = s.n.map(|v| format!(", n = {v}")).unwrap_or_default();
        let code = vec![
            format!("# SuSiE fine-mapping via summary statistics"),
            format!("{z} <- {input}$z"),
            format!(
                "# {r_mat} <- signed LD from MiXeR reference '{reference}'",
                reference = s.reference
            ),
            format!(
                "{out} <- susie_rss(z = {z}, R = {r_mat}, L = {}, estimate_prior_method = \"{}\", estimate_residual_variance = {}, estimate_prior_variance = {}, coverage = {}, min_abs_corr = {}, scaled_prior_variance = {}, z_method = \"{}\"{n_arg}, max_iter = {})",
                s.l,
                s.estimate_prior_method,
                s.estimate_residual_variance,
                s.estimate_prior_variance,
                s.coverage,
                s.min_abs_corr,
                s.scaled_prior_variance,
                s.z_method,
                s.max_iter
            ),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["susieR".into()]
    }
}

// ─── column extractors ───────────────────────────────────────────────────────

fn col_str(batches: &[RecordBatch], name: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        for v in dag_core::node::string_opt_values(col.as_ref())? {
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
            out.push(if col.is_null(i) {
                0
            } else {
                arr_i64(col.as_ref(), i)
            });
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
    let batches: Vec<RecordBatch> =
        input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;
    if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
        return Err(SusieNodeError::EmptyInput.into());
    }
    Ok(batches)
}

// ─── Signed LD lookup ────────────────────────────────────────────────────────

struct LdPair {
    id_a: String,
    id_b: String,
    r: f64,
}

const SUSIE_LD_QUERY_PY: &str = include_str!("susie_ld_query.py");

fn allele_from_variant_id(snp: &str, allele_index: usize) -> Option<&str> {
    let fields = snp.split(':').collect::<Vec<_>>();
    if fields.len() < 4 {
        return None;
    }
    fields.iter().rev().nth(1 - allele_index).copied()
}

/// Query signed Pearson-r pairs from the gsa-MiXeR engine.
async fn load_signed_ld_pairs(
    bundle: &crate::mixer_common::MixerReferenceBundle,
    chrom: i64,
    r2_min: f64,
    snps: &[String],
    z: &[f64],
    a1: Option<&[String]>,
    a2: Option<&[String]>,
    n: Option<f64>,
) -> Result<Vec<LdPair>, SusieNodeError> {
    let work = tempfile::tempdir()
        .map_err(|e| SusieNodeError::LdQuery(format!("create LD query directory: {e}")))?;
    let script_path = work.path().join("susie_ld_query.py");
    let query_path = work.path().join("locus.sumstats");
    std::fs::write(&script_path, SUSIE_LD_QUERY_PY)
        .map_err(|e| SusieNodeError::LdQuery(format!("write embedded LD query: {e}")))?;

    let mut query = String::from("SNP\tA1\tA2\tN\tZ\n");
    for (i, (snp, z_value)) in snps.iter().zip(z).enumerate() {
        let sample_n = n.unwrap_or(500.0);
        let allele1 = a1
            .and_then(|values| values.get(i))
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| allele_from_variant_id(snp, 0));
        let allele2 = a2
            .and_then(|values| values.get(i))
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| allele_from_variant_id(snp, 1));
        let (Some(allele1), Some(allele2)) = (allele1, allele2) else {
            return Err(SusieNodeError::LdQuery(format!(
                "missing a1/a2 for SNP '{snp}'; provide allele columns or a chr:pos:a1:a2 ID"
            )));
        };
        query.push_str(&format!(
            "{snp}\t{allele1}\t{allele2}\t{sample_n:.9}\t{z_value:.17}\n"
        ));
    }
    std::fs::write(&query_path, query)
        .map_err(|e| SusieNodeError::LdQuery(format!("write locus query: {e}")))?;

    let mut cmd =
        std::process::Command::new(crate::mixer_common::python_executable(&bundle.mixer_home));
    cmd.arg(&script_path)
        .arg("--engine-home")
        .arg(&bundle.mixer_home)
        .arg("--lib")
        .arg(bundle.mixer_home.join("libbgmg.so"))
        .arg("--bim-file")
        .arg(&bundle.bim_template)
        .arg("--ld-file")
        .arg(&bundle.ld_template)
        .arg("--chrom")
        .arg(chrom.to_string())
        .arg("--trait1-file")
        .arg(&query_path)
        .arg("--r2-min")
        .arg(r2_min.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = tokio::task::spawn_blocking(move || cmd.output())
        .await
        .map_err(|e| SusieNodeError::LdQuery(format!("join LD query: {e}")))?
        .map_err(|e| SusieNodeError::LdQuery(format!("run LD query: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SusieNodeError::LdQuery(format!(
            "exit {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim_end()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut pairs = Vec::new();
    let mut matched = std::collections::HashSet::new();
    for line in stdout.lines() {
        if let Some(id) = line.strip_prefix("#matched\t") {
            matched.insert(id.to_string());
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(SusieNodeError::LdQuery(format!(
                "invalid LD query row: {line}"
            )));
        }
        let r = fields[2]
            .parse::<f64>()
            .map_err(|e| SusieNodeError::LdQuery(format!("parse LD r '{line}': {e}")))?;
        pairs.push(LdPair {
            id_a: fields[0].to_string(),
            id_b: fields[1].to_string(),
            r,
        });
    }
    let missing: Vec<&String> = snps.iter().filter(|snp| !matched.contains(*snp)).collect();
    if !missing.is_empty() {
        return Err(SusieNodeError::LdQuery(format!(
            "{} input SNP(s) did not align to the signed-LD reference (first: {}); \
             check SNP ID and A1/A2 orientation",
            missing.len(),
            missing.first().map(|value| value.as_str()).unwrap_or("")
        )));
    }
    Ok(pairs)
}

/// Build a p×p correlation matrix R from signed LD pairs.
///
/// R[j,j] = 1.0 for all variants.
/// R[j,k] is the signed Pearson correlation stored by the reference.
fn build_corr_matrix(snps: &[String], pairs: &[LdPair]) -> Mat<f64> {
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
        let val = pair.r;
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
        reporter: &dag_core::dag::node_event::NodeReporter,
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
        let a1_col = col_str(&batches, "a1");
        let a2_col = col_str(&batches, "a2");
        let n_col = col_f64(&batches, "n");
        let chrom_col = col_i64(&batches, "chrom");

        // Determine sample size
        let n = self.spec.n.or_else(|| {
            n_col.as_ref().and_then(|col| {
                // Use the first non-NAN n value
                col.iter().find(|&&v| !v.is_nan() && v > 1.0).copied()
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
        let unique_snps = snps_filt.iter().collect::<std::collections::HashSet<_>>();
        if unique_snps.len() != p {
            return Err(SusieNodeError::Susie(format!(
                "duplicate SNP IDs in input ({unique_snps_len} unique of {p})",
                unique_snps_len = unique_snps.len()
            ))
            .into());
        }
        let a1_filt = a1_col
            .as_ref()
            .map(|values| keep.iter().map(|&i| values[i].clone()).collect::<Vec<_>>());
        let a2_filt = a2_col
            .as_ref()
            .map(|values| keep.iter().map(|&i| values[i].clone()).collect::<Vec<_>>());
        reporter.info(format!("susie_rss: {p} variants with valid z-scores"));

        // ── load signed LD from the reference bundle ──
        let ctx = node_ctx.session();
        let bundle = crate::mixer_common::resolve_reference(&self.spec.reference)
            .map_err(SusieNodeError::ReferenceBundle)?;
        reporter.info(format!(
            "susie_rss: querying signed LD reference '{}' for chromosome {chrom} (r² ≥ {})…",
            self.spec.reference, self.spec.r2_min
        ));
        let ld_pairs = load_signed_ld_pairs(
            &bundle,
            chrom,
            self.spec.r2_min,
            &snps_filt,
            &z_filt,
            a1_filt.as_deref(),
            a2_filt.as_deref(),
            n,
        )
        .await?;
        reporter.info(format!("susie_rss: loaded {} LD pairs", ld_pairs.len()));

        let r = build_corr_matrix(&snps_filt, &ld_pairs);

        // Check LD overlap
        let n_offdiag: usize = (0..p)
            .flat_map(|i| (0..p).map(move |j| (i, j)))
            .filter(|&(i, j)| i != j && r[(i, j)] != 0.0)
            .count();
        if p > 1 && n_offdiag == 0 {
            return Err(SusieNodeError::NoLdOverlap.into());
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

        let df = ctx.read_batch(batch).map_err(|e| {
            SusieNodeError::Df(datafusion::error::DataFusionError::External(
                e.to_string().into(),
            ))
        })?;
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
            reference: default_reference(),
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
        let pairs = vec![
            LdPair {
                id_a: "rs1".into(),
                id_b: "rs2".into(),
                r: 0.9,
            },
            LdPair {
                id_a: "rs1".into(),
                id_b: "rs3".into(),
                r: -0.8,
            },
        ];
        let r = build_corr_matrix(&snps, &pairs);
        // Signed reference correlations are preserved exactly.
        assert!((r[(0, 1)] - 0.9).abs() < 1e-10);
        assert!((r[(0, 2)] - (-0.8)).abs() < 1e-10);
        // diagonal = 1
        assert!((r[(1, 1)] - 1.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn runs_against_signed_reference_bundle() {
        let bundle = match crate::mixer_common::resolve_reference(&default_reference()) {
            Ok(bundle) => bundle,
            Err(_) => return,
        };
        let bim_path = bundle.bim_template.replace('@', "21");
        let content = std::fs::read_to_string(&bim_path).unwrap();
        let variants = content
            .lines()
            .take(50)
            .map(|line| line.split_whitespace().nth(1).unwrap().to_string())
            .collect::<Vec<_>>();
        let z = variants
            .iter()
            .enumerate()
            .map(|(i, _)| (i as f64 / 7.0).sin() * 4.0)
            .collect::<Vec<_>>();

        let schema = Arc::new(Schema::new(vec![
            Field::new("snp", DataType::Utf8, false),
            Field::new("chrom", DataType::Int64, true),
            Field::new("z", DataType::Float64, true),
            Field::new("n", DataType::Float64, true),
        ]));
        let chrom = vec![21_i64; variants.len()];
        let sample_n = vec![503.0; variants.len()];
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(variants)),
                Arc::new(Int64Array::from(chrom)),
                Arc::new(Float64Array::from(z)),
                Arc::new(Float64Array::from(sample_n)),
            ],
        )
        .unwrap();
        let ctx = datafusion::prelude::SessionContext::new();
        let input = ctx.read_batch(batch).unwrap();

        let mut node = SusieRssNode::new(SusieRssSpec {
            reference: default_reference(),
            l: 5,
            estimate_prior_method: "optim".into(),
            estimate_residual_variance: false,
            estimate_prior_variance: true,
            coverage: 0.95,
            min_abs_corr: 0.5,
            scaled_prior_variance: 0.2,
            z_method: "wald".into(),
            r2_min: 0.01,
            n: Some(503.0),
            check_null_threshold: 0.0,
            max_iter: 20,
        });
        let outputs = node
            .execute(
                &NodeCtx::new(ctx.runtime_env(), None),
                &[NodeInput::new_dataframe(0, input)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            50
        );
    }
}
