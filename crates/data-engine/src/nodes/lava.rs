//! LAVA DAG node — Local genetic correlation analysis (univariate h² + bivariate
//! local r_g) over user-defined genomic loci, wrapping the `lava` bio crate.
//!
//! Input port 0 carries GWAS summary statistics (one row per SNP × phenotype):
//! `snp, phenotype, a1, a2, stat, n`. The LD reference is a PLINK prefix
//! (`ref_prefix`). Output is one row per (locus, phenotype / phenotype-pair).
//!
//! The validated `lava` library loads PLINK genotypes and runs the full
//! `process.input → process.locus → run.univ / run.bivar` pipeline. (A
//! production variant that streams the LD matrix from an Iceberg table is a
//! drop-in: the library separates LD loading from analysis.)

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
};

const LAVA_NODE_KIND: &str = "lava";

#[derive(Debug, Error)]
pub enum LavaNodeError {
    #[error("LAVA computation failed: {0}")]
    Lava(String),
    #[error("failed to build/read result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read input: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
}

impl From<LavaNodeError> for DagError {
    fn from(e: LavaNodeError) -> Self {
        DagError::NodeError { node_type: LAVA_NODE_KIND.into(), msg: e.to_string() }
    }
}

// ----------------------------- input schema -----------------------------

const IN_SNP: &str = "snp";
const IN_PHENO: &str = "phenotype";
const IN_A1: &str = "a1";
const IN_A2: &str = "a2";
const IN_STAT: &str = "stat";
const IN_N: &str = "n";

fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(IN_SNP, DataType::Utf8, false),
        Field::new(IN_PHENO, DataType::Utf8, false),
        Field::new(IN_A1, DataType::Utf8, true),
        Field::new(IN_A2, DataType::Utf8, true),
        Field::new(IN_STAT, DataType::Float64, true),
        Field::new(IN_N, DataType::Float64, true),
    ]))
}

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("locus", DataType::Utf8, false),
        Field::new("chr", DataType::Int64, true),
        Field::new("start", DataType::Int64, true),
        Field::new("stop", DataType::Int64, true),
        Field::new("n_snps", DataType::Int64, false),
        Field::new("n_pcs", DataType::Int64, false),
        Field::new("analysis", DataType::Utf8, false),
        Field::new("phen1", DataType::Utf8, true),
        Field::new("phen2", DataType::Utf8, true),
        Field::new("estimate", DataType::Float64, true),
        Field::new("ci_lower", DataType::Float64, true),
        Field::new("ci_upper", DataType::Float64, true),
        Field::new("r2", DataType::Float64, true),
        Field::new("p", DataType::Float64, true),
    ]))
}

// ----------------------------- spec -----------------------------

/// A locus definition (coordinate form).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LavaLocus {
    pub loc: String,
    pub chr: i64,
    pub start: i64,
    pub stop: i64,
}

/// Per-phenotype case/control info. For continuous phenotypes leave unset
/// (treated as continuous). For binary phenotypes set `prop_cases` (sample case
/// proportion) and optionally `prevalence` (population prevalence for liability
/// transformation).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct LavaPhenoMeta {
    pub prop_cases: Option<f64>,
    pub prevalence: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LavaNodeSpec {
    /// PLINK reference prefix (`.bed/.bim/.fam`).
    #[serde(default)]
    pub ref_prefix: String,
    /// Loci to analyse (coordinate form).
    #[serde(default)]
    pub loci: Vec<LavaLocus>,
    /// Per-phenotype metadata (binary prop_cases / prevalence). Phenotypes not
    /// listed are treated as continuous.
    #[serde(default)]
    pub pheno_meta: HashMap<String, LavaPhenoMeta>,
    /// Sample-overlap correlation matrix (row/col order = phenotypes in input
    /// order). Omit if no overlap.
    #[serde(default)]
    pub sample_overlap: Option<Vec<Vec<f64>>>,
    /// Analyses to run.
    #[serde(default = "default_analyses")]
    pub analyses: Vec<String>,
    /// PC-variance pruning threshold (percent). Default 99.
    #[serde(default = "default_prune")]
    pub prune_thresh: f64,
    /// Cap on K as a proportion of min sample size. Default 0.75.
    #[serde(default = "default_max_prop_k")]
    pub max_prop_k: f64,
    /// Univariate p-value threshold for the bivariate pre-filter.
    #[serde(default = "default_univ_thresh")]
    pub univ_thresh: f64,
    /// RNG seed for the Monte-Carlo p-values / CIs.
    #[serde(default = "default_seed")]
    pub rng_seed: u64,
    /// Compute simulation p-values (slower).
    #[serde(default = "default_true")]
    pub p_values: bool,
    /// Compute 95% confidence intervals.
    #[serde(default = "default_true")]
    pub cis: bool,
}

fn default_analyses() -> Vec<String> { vec!["univ".into(), "bivar".into()] }
fn default_prune() -> f64 { 99.0 }
fn default_max_prop_k() -> f64 { 0.75 }
fn default_univ_thresh() -> f64 { 0.05 }
fn default_seed() -> u64 { lava::DEFAULT_RNG_SEED }
fn default_true() -> bool { true }

// ----------------------------- node -----------------------------

#[derive(Clone)]
pub struct LavaNode {
    meta: NodePorts,
    spec: LavaNodeSpec,
}

impl LavaNode {
    pub fn new(spec: LavaNodeSpec) -> Self {
        Self { meta: port_layout(), spec }
    }
}

pub struct LavaNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl NodeFactory for LavaNodeFactory {
    fn kind(&self) -> &'static str { LAVA_NODE_KIND }
    fn desc(&self) -> &'static str { "LAVA: local genetic correlation (local h² + bivariate local r_g) across genomic loci." }
    fn doc(&self) -> &'static str {
        "LAVA transform node. Takes SNP×phenotype GWAS summary statistics from \
        its input port, a PLINK LD reference prefix, and a list of loci, then runs \
        univariate (local h²) and bivariate (local genetic correlation) LAVA \
        analysis per locus. Outputs one row per (locus, phenotype / phenotype-pair)."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(LavaNodeSpec) }
    fn ports(&self) -> NodePorts { port_layout() }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let spec: LavaNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LavaNode::new(spec)))
    }
}

fn missing(name: &str) -> LavaNodeError {
    LavaNodeError::MissingColumn { name: name.into() }
}

#[async_trait]
impl DagNode for LavaNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new((*self).clone()) }
    fn kind(&self) -> &'static str { LAVA_NODE_KIND }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(&mut self, inputs: &[NodeInput]) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(LavaNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> = input.data.clone().collect().await.map_err(|e| {
            DagError::NodeError { node_type: LAVA_NODE_KIND.into(), msg: format!("collect failed: {e}") }
        })?;
        if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            return Err(LavaNodeError::EmptyInput.into());
        }

        let snp = col_string(&batches, IN_SNP).ok_or_else(|| missing(IN_SNP))?;
        let pheno = col_string(&batches, IN_PHENO).ok_or_else(|| missing(IN_PHENO))?;
        let a1 = col_string_opt(&batches, IN_A1);
        let a2 = col_string_opt(&batches, IN_A2);
        let stat = col_f64(&batches, IN_STAT).ok_or_else(|| missing(IN_STAT))?;
        let n = col_f64(&batches, IN_N).ok_or_else(|| missing(IN_N))?;

        // group rows by phenotype (preserve first-seen order)
        let mut order: Vec<String> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut groups: Vec<(Vec<String>, Vec<String>, Vec<String>, Vec<f64>, Vec<f64>)> = Vec::new();
        for i in 0..snp.len() {
            let p = pheno[i].clone();
            if !seen.contains_key(&p) {
                seen.insert(p.clone(), order.len());
                order.push(p.clone());
                groups.push((Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()));
            }
            let g = *seen.get(&p).unwrap();
            groups[g].0.push(snp[i].clone());
            groups[g].1.push(a1.as_ref().and_then(|v| v[i].clone()).unwrap_or_default());
            groups[g].2.push(a2.as_ref().and_then(|v| v[i].clone()).unwrap_or_default());
            groups[g].3.push(stat[i]);
            groups[g].4.push(n[i]);
        }

        // build lava SumStats + PhenoInfo
        let mut sum_stats = Vec::with_capacity(order.len());
        let mut info = Vec::with_capacity(order.len());
        for (ph, (snps, a1v, a2v, stats, nv)) in order.iter().zip(groups.into_iter()) {
            let meta = self.spec.pheno_meta.get(ph);
            let prop_cases = meta.and_then(|m| m.prop_cases);
            let binary = prop_cases.map(|pc| !pc.is_nan() && pc != 1.0).unwrap_or(false);
            sum_stats.push(lava::input::SumStats {
                snp: snps, a1: a1v, a2: a2v, stat: stats, n: nv, gene: None,
            });
            info.push(lava::input::PhenoInfo {
                phenotype: ph.clone(),
                cases: f64::NAN,
                controls: f64::NAN,
                filename: String::new(),
                n: f64::NAN,
                prop_cases: prop_cases.unwrap_or(f64::NAN),
                binary,
                prevalence: meta.and_then(|m| m.prevalence),
            });
        }

        let sample_overlap = self.spec.sample_overlap.as_ref().map(|m| {
            let rows = m.len();
            let cols = m.first().map(|r| r.len()).unwrap_or(0);
            let mut mat = faer::Mat::zeros(rows, cols);
            for i in 0..rows {
                for j in 0..cols {
                    mat[(i, j)] = m[i][j];
                }
            }
            lava::stats::cov2cor(&mat)
        });

        let input = lava::input::finish_input(
            info,
            order.clone(),
            sum_stats,
            sample_overlap,
            std::path::Path::new(&self.spec.ref_prefix),
        )
        .map_err(|e| LavaNodeError::Lava(e.to_string()))?;

        let opts = lava::locus::LocusOptions {
            min_k: 2,
            prune_thresh: self.spec.prune_thresh,
            max_prop_k: Some(self.spec.max_prop_k),
            drop_failed: true,
            max_block_size: 3000,
            cap_estimates: true,
        };
        let mut rng = lava::rng(self.spec.rng_seed);
        let do_univ = self.spec.analyses.iter().any(|a| a == "univ");
        let do_bivar = self.spec.analyses.iter().any(|a| a == "bivar");

        let mut rows: Vec<OutRow> = Vec::new();
        for ld in &self.spec.loci {
            let locus_def = lava::input::LocusDef {
                loc: ld.loc.clone(),
                chr: Some(ld.chr),
                start: Some(ld.start),
                stop: Some(ld.stop),
                snps: None,
            };
            let locus = match lava::locus::process_locus(&locus_def, &input, None, &opts) {
                Ok(Some(l)) => l,
                _ => continue,
            };
            if do_univ {
                let univ = lava::analysis::run_univ(&locus, None, false, true);
                for u in &univ {
                    rows.push(OutRow {
                        locus: ld.loc.clone(), chr: ld.chr, start: ld.start, stop: ld.stop,
                        n_snps: locus.n_snps as i64, n_pcs: locus.k as i64,
                        analysis: "univ".into(), phen1: u.phen.clone(), phen2: String::new(),
                        estimate: u.h2_obs, ci_lower: f64::NAN, ci_upper: f64::NAN,
                        r2: f64::NAN, p: u.p,
                    });
                }
            }
            if do_bivar {
                let bivar = lava::analysis::run_bivar(
                    &locus, None, None, None, self.spec.p_values, self.spec.cis, 1.25, true, &mut rng,
                );
                for b in &bivar {
                    rows.push(OutRow {
                        locus: ld.loc.clone(), chr: ld.chr, start: ld.start, stop: ld.stop,
                        n_snps: locus.n_snps as i64, n_pcs: locus.k as i64,
                        analysis: "bivar".into(), phen1: b.phen1.clone(), phen2: b.phen2.clone(),
                        estimate: b.rho, ci_lower: b.rho_lower, ci_upper: b.rho_upper,
                        r2: b.r2, p: b.p,
                    });
                }
            }
        }

        let batch = build_result_batch(&rows)?;
        let ctx = datafusion::prelude::SessionContext::new();
        let df = ctx.read_batch(batch).map_err(LavaNodeError::from)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

struct OutRow {
    locus: String, chr: i64, start: i64, stop: i64, n_snps: i64, n_pcs: i64,
    analysis: String, phen1: String, phen2: String,
    estimate: f64, ci_lower: f64, ci_upper: f64, r2: f64, p: f64,
}

fn build_result_batch(rows: &[OutRow]) -> Result<RecordBatch, LavaNodeError> {
    let schema = output_schema();
    let locus = StringArray::from(rows.iter().map(|r| r.locus.as_str()).collect::<Vec<_>>());
    let chr = Int64Array::from(rows.iter().map(|r| Some(r.chr)).collect::<Vec<_>>());
    let start = Int64Array::from(rows.iter().map(|r| Some(r.start)).collect::<Vec<_>>());
    let stop = Int64Array::from(rows.iter().map(|r| Some(r.stop)).collect::<Vec<_>>());
    let n_snps = Int64Array::from(rows.iter().map(|r| r.n_snps).collect::<Vec<_>>());
    let n_pcs = Int64Array::from(rows.iter().map(|r| r.n_pcs).collect::<Vec<_>>());
    let analysis = StringArray::from(rows.iter().map(|r| r.analysis.as_str()).collect::<Vec<_>>());
    let phen1 = StringArray::from(rows.iter().map(|r| r.phen1.as_str()).collect::<Vec<_>>());
    let phen2 = StringArray::from(rows.iter().map(|r| r.phen2.as_str()).collect::<Vec<_>>());
    let estimate = Float64Array::from(rows.iter().map(|r| Some(r.estimate)).collect::<Vec<_>>());
    let ci_lower = Float64Array::from(rows.iter().map(|r| Some(r.ci_lower)).collect::<Vec<_>>());
    let ci_upper = Float64Array::from(rows.iter().map(|r| Some(r.ci_upper)).collect::<Vec<_>>());
    let r2 = Float64Array::from(rows.iter().map(|r| Some(r.r2)).collect::<Vec<_>>());
    let p = Float64Array::from(rows.iter().map(|r| Some(r.p)).collect::<Vec<_>>());
    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(locus), Arc::new(chr), Arc::new(start), Arc::new(stop),
            Arc::new(n_snps), Arc::new(n_pcs), Arc::new(analysis), Arc::new(phen1),
            Arc::new(phen2), Arc::new(estimate), Arc::new(ci_lower), Arc::new(ci_upper),
            Arc::new(r2), Arc::new(p),
        ],
    )?)
}

// ----------------------------- column extractors -----------------------------

fn col_string(batches: &[RecordBatch], name: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        let arr = col.as_any().downcast_ref::<arrow_array::StringArray>()?;
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) { String::new() } else { arr.value(i).to_string() });
        }
    }
    Some(out)
}

fn col_string_opt(batches: &[RecordBatch], name: &str) -> Option<Vec<Option<String>>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        let arr = col.as_any().downcast_ref::<arrow_array::StringArray>()?;
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) { None } else { Some(arr.value(i).to_string()) });
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
                arrow_f64_value(col.as_ref(), i)
            });
        }
    }
    Some(out)
}

fn arrow_f64_value(arr: &dyn Array, i: usize) -> f64 {
    use arrow_array::*;
    if let Some(a) = arr.as_any().downcast_ref::<Float64Array>() { return a.value(i); }
    if let Some(a) = arr.as_any().downcast_ref::<Float32Array>() { return a.value(i) as f64; }
    if let Some(a) = arr.as_any().downcast_ref::<Int64Array>() { return a.value(i) as f64; }
    if let Some(a) = arr.as_any().downcast_ref::<Int32Array>() { return a.value(i) as f64; }
    f64::NAN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_metadata() {
        let f = LavaNodeFactory {};
        assert_eq!(f.kind(), "lava");
        let _ = f.spec_schema();
        let _ = f.ports();
    }

    #[test]
    fn spec_round_trips() {
        let spec = serde_json::json!({
            "ref_prefix": "ref/g1000",
            "loci": [{"loc": "1", "chr": 1, "start": 10, "stop": 20}],
            "analyses": ["univ", "bivar"],
            "rng_seed": 42,
        });
        let s: LavaNodeSpec = serde_json::from_value(spec).unwrap();
        assert_eq!(s.loci.len(), 1);
        assert_eq!(s.analyses, vec!["univ".to_string(), "bivar".to_string()]);
        assert_eq!(s.rng_seed, 42);
        assert!((s.prune_thresh - 99.0).abs() < 1e-12); // default applied
    }
}
