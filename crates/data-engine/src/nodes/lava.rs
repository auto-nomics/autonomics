//! LAVA DAG nodes — local genetic correlation analysis.
//!
//! Split into a **shared locus-construction node** plus four **lightweight
//! analysis nodes**, so the expensive `process.input → process.locus` runs once
//! and feeds all analyses:
//!
//! ```text
//! GWAS sumstats ──▶ lava_locus ──┬──▶ lava_univ    ──▶ univ results
//!   (process.input +             ├──▶ lava_bivar   ──▶ bivar results
//!    per-locus process.locus)    ├──▶ lava_pcor    ──▶ pcor results
//!                                └──▶ lava_multireg──▶ multireg results
//! ```
//!
//! `lava_locus` emits a typed "locus parameters" table (the upper-triangular
//! `omega`/`sigma` cells + per-phenotype scalars — everything the analyses need,
//! without the K-dim `delta`). The analysis nodes consume that table, rebuild
//! the matrices, and run their method.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Array, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::dag::runtime::RuntimeStatus;
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum LavaNodeError {
    #[error("LAVA computation failed: {0}")]
    Lava(String),
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("datafusion error: {0}")]
    Df(#[from] datafusion::error::DataFusionError),
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
}

impl From<LavaNodeError> for DagError {
    fn from(e: LavaNodeError) -> Self {
        DagError::NodeError {
            node_type: "lava".into(),
            msg: e.to_string(),
        }
    }
}

fn missing(name: &str) -> LavaNodeError {
    LavaNodeError::MissingColumn { name: name.into() }
}

// ============================ shared schemas ============================

const IN_SNP: &str = "snp";
const IN_PHENO: &str = "phenotype";
const IN_A1: &str = "a1";
const IN_A2: &str = "a2";
const IN_STAT: &str = "stat";
const IN_N: &str = "n";

fn gwas_input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(IN_SNP, DataType::Utf8, false),
        Field::new(IN_PHENO, DataType::Utf8, false),
        Field::new(IN_A1, DataType::Utf8, true),
        Field::new(IN_A2, DataType::Utf8, true),
        Field::new(IN_STAT, DataType::Float64, true),
        Field::new(IN_N, DataType::Float64, true),
    ]))
}

const P_LOCUS: &str = "locus";
const P_CHR: &str = "chr";
const P_START: &str = "start";
const P_STOP: &str = "stop";
const P_NSNPS: &str = "n_snps";
const P_K: &str = "k";
const P_NREF: &str = "nref_scale";
const P_I: &str = "i";
const P_J: &str = "j";
const P_PHI: &str = "pheno_i";
const P_PHJ: &str = "pheno_j";
const P_OMEGA: &str = "omega";
const P_SIGMA: &str = "sigma";
const P_NI: &str = "n_i";
const P_BINI: &str = "binary_i";
const P_H2O: &str = "h2_obs_i";
const P_H2L: &str = "h2_latent_i";
const P_ASC: &str = "ascertained_i";

/// The "locus parameters" table — the contract between `lava_locus` and the
/// analysis nodes. One row per (locus, i, j) with i ≤ j.
fn locus_params_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(P_LOCUS, DataType::Utf8, false),
        Field::new(P_CHR, DataType::Int64, true),
        Field::new(P_START, DataType::Int64, true),
        Field::new(P_STOP, DataType::Int64, true),
        Field::new(P_NSNPS, DataType::Int64, false),
        Field::new(P_K, DataType::Int64, false),
        Field::new(P_NREF, DataType::Float64, false),
        Field::new(P_I, DataType::Int64, false),
        Field::new(P_J, DataType::Int64, false),
        Field::new(P_PHI, DataType::Utf8, false),
        Field::new(P_PHJ, DataType::Utf8, false),
        Field::new(P_OMEGA, DataType::Float64, false),
        Field::new(P_SIGMA, DataType::Float64, false),
        Field::new(P_NI, DataType::Float64, true),
        Field::new(P_BINI, DataType::Boolean, true),
        Field::new(P_H2O, DataType::Float64, true),
        Field::new(P_H2L, DataType::Float64, true),
        Field::new(P_ASC, DataType::Boolean, true),
    ]))
}

// ============================ shared spec bits ============================

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LavaLocus {
    pub loc: String,
    pub chr: i64,
    pub start: i64,
    pub stop: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct LavaPhenoMeta {
    pub prop_cases: Option<f64>,
    pub prevalence: Option<f64>,
}

// ============================ column extractors ============================

fn col_str(batches: &[RecordBatch], name: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        let arr = col.as_any().downcast_ref::<StringArray>()?;
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) {
                String::new()
            } else {
                arr.value(i).to_string()
            });
        }
    }
    Some(out)
}

fn col_opt_str(batches: &[RecordBatch], name: &str) -> Option<Vec<Option<String>>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        let arr = col.as_any().downcast_ref::<StringArray>()?;
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) {
                None
            } else {
                Some(arr.value(i).to_string())
            });
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

fn col_opt_f64(batches: &[RecordBatch], name: &str) -> Option<Vec<Option<f64>>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        for i in 0..col.len() {
            out.push(if col.is_null(i) {
                None
            } else {
                Some(arr_f64(col.as_ref(), i))
            });
        }
    }
    Some(out)
}

fn col_opt_bool(batches: &[RecordBatch], name: &str) -> Option<Vec<Option<bool>>> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        let arr = col.as_any().downcast_ref::<BooleanArray>()?;
        for i in 0..arr.len() {
            out.push(if arr.is_null(i) {
                None
            } else {
                Some(arr.value(i))
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

/// Read the shared "locus parameters" table into reconstructed [`LocusParams`].
fn parse_locus_params(
    batches: &[RecordBatch],
) -> Result<Vec<lava::locus::LocusParams>, LavaNodeError> {
    let locus = col_str(batches, P_LOCUS).ok_or_else(|| missing(P_LOCUS))?;
    let n = locus.len();
    macro_rules! reqi {
        ($col:ident, $name:expr) => {
            let $col = col_i64(batches, $name).ok_or_else(|| missing($name))?;
        };
    }
    let chr = col_i64(batches, P_CHR).ok_or_else(|| missing(P_CHR))?;
    let start = col_i64(batches, P_START).ok_or_else(|| missing(P_START))?;
    let stop = col_i64(batches, P_STOP).ok_or_else(|| missing(P_STOP))?;
    reqi!(n_snps, P_NSNPS);
    reqi!(k, P_K);
    let nref = col_f64(batches, P_NREF).ok_or_else(|| missing(P_NREF))?;
    reqi!(ii, P_I);
    reqi!(jj, P_J);
    let phi = col_str(batches, P_PHI).ok_or_else(|| missing(P_PHI))?;
    let phj = col_str(batches, P_PHJ).ok_or_else(|| missing(P_PHJ))?;
    let omega = col_f64(batches, P_OMEGA).ok_or_else(|| missing(P_OMEGA))?;
    let sigma = col_f64(batches, P_SIGMA).ok_or_else(|| missing(P_SIGMA))?;
    let n_i = col_opt_f64(batches, P_NI).ok_or_else(|| missing(P_NI))?;
    let bin_i = col_opt_bool(batches, P_BINI).ok_or_else(|| missing(P_BINI))?;
    let h2o = col_opt_f64(batches, P_H2O).ok_or_else(|| missing(P_H2O))?;
    let h2l = col_opt_f64(batches, P_H2L).ok_or_else(|| missing(P_H2L))?;
    let asc = col_opt_bool(batches, P_ASC).ok_or_else(|| missing(P_ASC))?;

    let rows: Vec<lava::locus::LocusParamRow> = (0..n)
        .map(|r| lava::locus::LocusParamRow {
            locus: locus[r].clone(),
            chr: Some(chr[r]),
            start: Some(start[r]),
            stop: Some(stop[r]),
            n_snps: n_snps[r] as usize,
            k: k[r] as usize,
            nref_scale: nref[r],
            i: ii[r] as usize,
            j: jj[r] as usize,
            pheno_i: phi[r].clone(),
            pheno_j: phj[r].clone(),
            omega: omega[r],
            sigma: sigma[r],
            n_i: n_i[r],
            binary_i: bin_i[r],
            h2_obs_i: h2o[r],
            h2_latent_i: h2l[r],
            ascertained_i: asc[r],
        })
        .collect();
    Ok(lava::locus::locus_params_from_rows(&rows))
}

/// Build the shared "locus parameters" `RecordBatch` from reconstructed params.
fn build_locus_params_batch(
    loci: &[lava::locus::LocusParams],
) -> Result<RecordBatch, LavaNodeError> {
    let rows: Vec<lava::locus::LocusParamRow> =
        loci.iter().flat_map(|lp| lp.to_param_rows()).collect();
    let locus = StringArray::from(rows.iter().map(|r| r.locus.as_str()).collect::<Vec<_>>());
    let chr = Int64Array::from(rows.iter().map(|r| r.chr).collect::<Vec<_>>());
    let start = Int64Array::from(rows.iter().map(|r| r.start).collect::<Vec<_>>());
    let stop = Int64Array::from(rows.iter().map(|r| r.stop).collect::<Vec<_>>());
    let n_snps = Int64Array::from(rows.iter().map(|r| r.n_snps as i64).collect::<Vec<_>>());
    let k = Int64Array::from(rows.iter().map(|r| r.k as i64).collect::<Vec<_>>());
    let nref = Float64Array::from(rows.iter().map(|r| r.nref_scale).collect::<Vec<_>>());
    let ii = Int64Array::from(rows.iter().map(|r| r.i as i64).collect::<Vec<_>>());
    let jj = Int64Array::from(rows.iter().map(|r| r.j as i64).collect::<Vec<_>>());
    let phi = StringArray::from(rows.iter().map(|r| r.pheno_i.as_str()).collect::<Vec<_>>());
    let phj = StringArray::from(rows.iter().map(|r| r.pheno_j.as_str()).collect::<Vec<_>>());
    let omega = Float64Array::from(rows.iter().map(|r| r.omega).collect::<Vec<_>>());
    let sigma = Float64Array::from(rows.iter().map(|r| r.sigma).collect::<Vec<_>>());
    let n_i = Float64Array::from(rows.iter().map(|r| r.n_i).collect::<Vec<_>>());
    let bin_i = BooleanArray::from(rows.iter().map(|r| r.binary_i).collect::<Vec<_>>());
    let h2o = Float64Array::from(rows.iter().map(|r| r.h2_obs_i).collect::<Vec<_>>());
    let h2l = Float64Array::from(rows.iter().map(|r| r.h2_latent_i).collect::<Vec<_>>());
    let asc = BooleanArray::from(rows.iter().map(|r| r.ascertained_i).collect::<Vec<_>>());
    RecordBatch::try_new(
        locus_params_schema(),
        vec![
            Arc::new(locus),
            Arc::new(chr),
            Arc::new(start),
            Arc::new(stop),
            Arc::new(n_snps),
            Arc::new(k),
            Arc::new(nref),
            Arc::new(ii),
            Arc::new(jj),
            Arc::new(phi),
            Arc::new(phj),
            Arc::new(omega),
            Arc::new(sigma),
            Arc::new(n_i),
            Arc::new(bin_i),
            Arc::new(h2o),
            Arc::new(h2l),
            Arc::new(asc),
        ],
    )
    .map_err(LavaNodeError::Arrow)
}

async fn collect_input_batches(
    input: &NodeInput,
    kind: &str,
) -> Result<Vec<RecordBatch>, DagError> {
    let batches: Vec<RecordBatch> =
        input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: kind.into(),
                msg: format!("collect failed: {e}"),
            })?;
    if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
        return Err(LavaNodeError::EmptyInput.into());
    }
    Ok(batches)
}

fn read_batch(
    node_ctx: &crate::node_registry::registry::NodeCtx,
    batch: RecordBatch,
) -> Result<PortOutputs, LavaNodeError> {
    let ctx = node_ctx.session();
    let df = ctx.read_batch(batch).map_err(LavaNodeError::Df)?;
    let mut res: PortOutputs = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

// ============================ lava_locus node ============================

/// Spec for [`LavaLocusNode`]: shared locus construction.
///
/// The PLINK LD reference is **not** a spec parameter: it is hardcoded as
/// [`REF_PREFIX_TEMPLATE`] (the EUR per-chromosome panel) for now. Only the
/// loci, phenotype metadata, sample-overlap, and decomposition tuning are spec-
/// configurable.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LavaLocusSpec {
    #[serde(default)]
    pub loci: Vec<LavaLocus>,
    #[serde(default)]
    pub pheno_meta: HashMap<String, LavaPhenoMeta>,
    #[serde(default)]
    pub sample_overlap: Option<Vec<Vec<f64>>>,
    #[serde(default = "d_prune")]
    pub prune_thresh: f64,
    #[serde(default = "d_max_prop_k")]
    pub max_prop_k: f64,
    #[serde(default = "d_min_k")]
    pub min_k: usize,
}
fn d_prune() -> f64 {
    99.0
}
fn d_max_prop_k() -> f64 {
    0.75
}
fn d_min_k() -> usize {
    2
}

const LOCUS_KIND: &str = "lava_locus";

/// Hardcoded per-chromosome PLINK reference prefix (EUR 1000G, one `.bed/.bim/.fam`
/// per chromosome). `{N}` is resolved to each locus's chromosome at execution time.
/// Temporary: until the reference is parameterized again via the spec / a registry.
const REF_PREFIX_TEMPLATE: &str = "/mnt/disk2/dataset/1000g_plink/eur/chr{N}/1000G.EUR.chr{N}.qc";

#[derive(Clone)]
pub struct LavaLocusNode {
    meta: NodePorts,
    spec: LavaLocusSpec,
}
impl LavaLocusNode {
    pub fn new(spec: LavaLocusSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(Some(gwas_input_schema()))
                .add_output_port(Some(locus_params_schema())),
            spec,
        }
    }
}

pub struct LavaLocusNodeFactory {}
impl NodeFactory for LavaLocusNodeFactory {
    fn kind(&self) -> &'static str {
        LOCUS_KIND
    }
    fn desc(&self) -> &'static str {
        "LAVA locus construction: process.input + per-locus process.locus → omega/sigma table."
    }
    fn doc(&self) -> &'static str {
        "Builds the per-locus LAVA parameters (omega/sigma/h²) from GWAS sumstats + a PLINK LD reference. Feed its output into lava_univ/lava_bivar/lava_pcor/lava_multireg."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LavaLocusSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(Some(gwas_input_schema()))
            .add_output_port(Some(locus_params_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(LavaLocusNode::new(serde_json::from_value(spec)?)))
    }
}

#[async_trait]
impl DagNode for LavaLocusNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        LOCUS_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        reporter.status(RuntimeStatus::Running);
        reporter.info(format!(
            "locus: start (loci={}, prune_thresh={}, max_prop_k={}, min_k={}, \
             sample_overlap={})",
            self.spec.loci.len(),
            self.spec.prune_thresh,
            self.spec.max_prop_k,
            self.spec.min_k,
            self.spec.sample_overlap.is_some(),
        ));

        let input = inputs.first().ok_or_else(|| {
            reporter.error("locus: abort — no input port connected");
            LavaNodeError::EmptyInput
        })?;
        let batches = match collect_input_batches(input, LOCUS_KIND).await {
            Ok(b) => b,
            Err(e) => {
                reporter.error(format!("locus: abort — input collection failed: {e}"));
                return Err(e);
            }
        };
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        reporter.info(format!(
            "locus: collected {total_rows} sumstat rows across {} record batches",
            batches.len(),
        ));

        let snp = col_str(&batches, IN_SNP).ok_or_else(|| {
            reporter.error(format!("locus: abort — missing required column '{IN_SNP}'"));
            missing(IN_SNP)
        })?;
        let pheno = col_str(&batches, IN_PHENO).ok_or_else(|| {
            reporter.error(format!(
                "locus: abort — missing required column '{IN_PHENO}'"
            ));
            missing(IN_PHENO)
        })?;
        let a1 = col_opt_str(&batches, IN_A1);
        let a2 = col_opt_str(&batches, IN_A2);
        let stat = col_f64(&batches, IN_STAT).ok_or_else(|| {
            reporter.error(format!(
                "locus: abort — missing required column '{IN_STAT}'"
            ));
            missing(IN_STAT)
        })?;
        let n = col_f64(&batches, IN_N).ok_or_else(|| {
            reporter.error(format!("locus: abort — missing required column '{IN_N}'"));
            missing(IN_N)
        })?;

        // group by phenotype (preserve first-seen order)
        let mut order: Vec<String> = Vec::new();
        let mut idx: HashMap<String, usize> = HashMap::new();
        let mut groups: Vec<(Vec<String>, Vec<String>, Vec<f64>, Vec<f64>)> = Vec::new();
        for i in 0..snp.len() {
            let p = pheno[i].clone();
            if !idx.contains_key(&p) {
                idx.insert(p.clone(), order.len());
                order.push(p.clone());
                groups.push((Vec::new(), Vec::new(), Vec::new(), Vec::new()));
            }
            let g = *idx.get(&p).unwrap();
            groups[g]
                .0
                .push(a1.as_ref().and_then(|v| v[i].clone()).unwrap_or_default());
            groups[g]
                .1
                .push(a2.as_ref().and_then(|v| v[i].clone()).unwrap_or_default());
            groups[g].2.push(stat[i]);
            groups[g].3.push(n[i]);
        }
        let mut sum_stats = Vec::with_capacity(order.len());
        let mut info = Vec::with_capacity(order.len());
        for (ph, (a1v, a2v, stv, nv)) in order.iter().zip(groups) {
            let meta = self.spec.pheno_meta.get(ph);
            let prop_cases = meta.and_then(|m| m.prop_cases);
            let binary = prop_cases
                .map(|pc| !pc.is_nan() && pc != 1.0)
                .unwrap_or(false);
            // snp ids per phenotype: the snp column for that pheno's rows
            let snps: Vec<String> = snp
                .iter()
                .enumerate()
                .filter(|(i, _)| pheno[*i] == *ph)
                .map(|(_, s)| s.clone())
                .collect();
            sum_stats.push(lava::input::SumStats {
                snp: snps,
                a1: a1v,
                a2: a2v,
                stat: stv,
                n: nv,
                gene: None,
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
        reporter.info(format!(
            "locus: grouped into {} phenotype(s): [{}]",
            order.len(),
            order.join(", "),
        ));

        let sample_overlap = self.spec.sample_overlap.as_ref().map(|m| {
            let mut mat = faer::Mat::zeros(m.len(), m.first().map(|r| r.len()).unwrap_or(0));
            for i in 0..m.len() {
                for j in 0..m[i].len() {
                    mat[(i, j)] = m[i][j];
                }
            }
            lava::stats::cov2cor(&mat)
        });

        // Build the PLINK reference from the hardcoded per-chromosome template,
        // loading only the chromosomes that appear in `loci`.
        let mut chroms: Vec<i64> = self.spec.loci.iter().map(|l| l.chr).collect();
        chroms.sort_unstable();
        chroms.dedup();
        reporter.info(format!(
            "locus: loading PLINK LD reference for chromosomes {:?}",
            chroms,
        ));
        let reference = match lava::plink::load_reference_template(REF_PREFIX_TEMPLATE, &chroms) {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                reporter.error(format!("locus: abort — LD reference load failed: {msg}"));
                return Err(LavaNodeError::Lava(msg).into());
            }
        };
        reporter.info("locus: LD reference loaded — assembling input object");
        let input_obj = match lava::input::finish_input_with_ref(
            info,
            order.clone(),
            sum_stats,
            sample_overlap,
            reference,
        ) {
            Ok(o) => o,
            Err(e) => {
                let msg = e.to_string();
                reporter.error(format!("locus: abort — input assembly failed: {msg}"));
                return Err(LavaNodeError::Lava(msg).into());
            }
        };
        let opts = lava::locus::LocusOptions {
            min_k: self.spec.min_k.max(2),
            prune_thresh: self.spec.prune_thresh,
            max_prop_k: Some(self.spec.max_prop_k),
            drop_failed: true,
            max_block_size: 3000,
            cap_estimates: true,
        };

        let n_loci = self.spec.loci.len();
        let mut params: Vec<lava::locus::LocusParams> = Vec::new();
        let mut failed = 0usize;
        for (idx, ld) in self.spec.loci.iter().enumerate() {
            reporter.progress(idx as u64, n_loci as u64);
            reporter.info(format!(
                "locus: [{}/{}] {} chr{}:{}-{}",
                idx + 1,
                n_loci,
                ld.loc,
                ld.chr,
                ld.start,
                ld.stop,
            ));
            let locus_def = lava::input::LocusDef {
                loc: ld.loc.clone(),
                chr: Some(ld.chr),
                start: Some(ld.start),
                stop: Some(ld.stop),
                snps: None,
            };
            match lava::locus::process_locus(&locus_def, &input_obj, None, &opts) {
                Ok(Some(loc)) => {
                    let p = loc.params();
                    reporter.info(format!(
                        "locus:     {} ok — k={}, n_snps={}",
                        ld.loc, p.k, p.n_snps,
                    ));
                    params.push(p);
                }
                Ok(None) => {
                    failed += 1;
                    reporter.warn(format!(
                        "locus:     {} dropped — failed QC/ pruning (no usable SNPs)",
                        ld.loc,
                    ));
                }
                Err(e) => {
                    failed += 1;
                    reporter.warn(format!("locus:     {} failed — {e}", ld.loc));
                }
            }
        }
        reporter.progress(n_loci as u64, n_loci as u64);
        let n_rows: usize = params.iter().map(|p| p.to_param_rows().len()).sum();
        reporter.info(format!(
            "locus: done — {} locus param table(s) ({} total rows, {} dropped/failed)",
            params.len(),
            n_rows,
            failed,
        ));
        read_batch(node_ctx, build_locus_params_batch(&params)?).map_err(Into::into)
    }
}

// ============================ lava_univ node ============================

const UNIV_KIND: &str = "lava_univ";
fn univ_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("locus", DataType::Utf8, false),
        Field::new("phen", DataType::Utf8, false),
        Field::new("h2_obs", DataType::Float64, true),
        Field::new("h2_latent", DataType::Float64, true),
        Field::new("ascertained", DataType::Boolean, true),
        Field::new("p", DataType::Float64, true),
    ]))
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct LavaUnivSpec {
    #[serde(default)]
    pub phenos: Option<Vec<String>>,
    #[serde(default = "d_true")]
    pub cap_estimates: bool,
}
fn d_true() -> bool {
    true
}

#[derive(Clone)]
pub struct LavaUnivNode {
    meta: NodePorts,
    spec: LavaUnivSpec,
}
impl LavaUnivNode {
    pub fn new(spec: LavaUnivSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(Some(locus_params_schema()))
                .add_output_port(Some(univ_schema())),
            spec,
        }
    }
}
pub struct LavaUnivNodeFactory {}
impl NodeFactory for LavaUnivNodeFactory {
    fn kind(&self) -> &'static str {
        UNIV_KIND
    }
    fn desc(&self) -> &'static str {
        "LAVA univariate local h² test."
    }
    fn doc(&self) -> &'static str {
        "Consumes the lava_locus parameters table; tests local heritability per phenotype (F-test continuous / χ² binary)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LavaUnivSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(Some(locus_params_schema()))
            .add_output_port(Some(univ_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(LavaUnivNode::new(serde_json::from_value(spec)?)))
    }
}
#[async_trait]
impl DagNode for LavaUnivNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        UNIV_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        reporter.status(RuntimeStatus::Running);
        let batches = collect_input_batches(
            inputs.first().ok_or_else(|| {
                reporter.error("univ: abort — no input port connected");
                LavaNodeError::EmptyInput
            })?,
            UNIV_KIND,
        )
        .await?;
        let params = parse_locus_params(&batches)?;
        let phenos = self.spec.phenos.clone();
        reporter.info(format!(
            "univ: start ({} locus table(s), phenos={:?}, cap_estimates={})",
            params.len(),
            phenos.as_deref().unwrap_or(&[]),
            self.spec.cap_estimates,
        ));
        let n_loci = params.len();
        let mut locus_v = Vec::new();
        let mut phen_v = Vec::new();
        let mut h2o_v = Vec::new();
        let mut h2l_v = Vec::new();
        let mut asc_v = Vec::new();
        let mut p_v = Vec::new();
        for (idx, lp) in params.iter().enumerate() {
            reporter.progress(idx as u64, n_loci as u64);
            for u in lava::analysis::run_univ(lp, phenos.as_deref(), false, self.spec.cap_estimates)
            {
                locus_v.push(lp.id.clone());
                phen_v.push(u.phen.clone());
                h2o_v.push(Some(u.h2_obs));
                h2l_v.push(u.h2_latent);
                asc_v.push(u.ascertained);
                p_v.push(Some(u.p));
            }
        }
        reporter.progress(n_loci as u64, n_loci as u64);
        reporter.info(format!("univ: done — {} univariate tests", phen_v.len()));
        let batch = RecordBatch::try_new(
            univ_schema(),
            vec![
                Arc::new(StringArray::from(locus_v)),
                Arc::new(StringArray::from(phen_v)),
                Arc::new(Float64Array::from(h2o_v)),
                Arc::new(Float64Array::from(h2l_v)),
                Arc::new(BooleanArray::from(asc_v)),
                Arc::new(Float64Array::from(p_v)),
            ],
        )
        .map_err(LavaNodeError::Arrow)?;
        read_batch(node_ctx, batch).map_err(Into::into)
    }
}

// ============================ lava_bivar node ============================

const BIVAR_KIND: &str = "lava_bivar";
fn bivar_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("locus", DataType::Utf8, false),
        Field::new("phen1", DataType::Utf8, false),
        Field::new("phen2", DataType::Utf8, false),
        Field::new("rho", DataType::Float64, true),
        Field::new("rho_lower", DataType::Float64, true),
        Field::new("rho_upper", DataType::Float64, true),
        Field::new("r2", DataType::Float64, true),
        Field::new("r2_lower", DataType::Float64, true),
        Field::new("r2_upper", DataType::Float64, true),
        Field::new("p", DataType::Float64, true),
    ]))
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LavaBivarSpec {
    #[serde(default)]
    pub phenos: Option<Vec<String>>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default = "d_true")]
    pub p_values: bool,
    #[serde(default = "d_true")]
    pub cis: bool,
    #[serde(default = "d_param_lim")]
    pub param_lim: f64,
    #[serde(default = "d_seed")]
    pub rng_seed: u64,
}
fn d_param_lim() -> f64 {
    1.25
}
fn d_seed() -> u64 {
    lava::DEFAULT_RNG_SEED
}

#[derive(Clone)]
pub struct LavaBivarNode {
    meta: NodePorts,
    spec: LavaBivarSpec,
}
impl LavaBivarNode {
    pub fn new(spec: LavaBivarSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(Some(locus_params_schema()))
                .add_output_port(Some(bivar_schema())),
            spec,
        }
    }
}
pub struct LavaBivarNodeFactory {}
impl NodeFactory for LavaBivarNodeFactory {
    fn kind(&self) -> &'static str {
        BIVAR_KIND
    }
    fn desc(&self) -> &'static str {
        "LAVA bivariate local genetic correlation (r_g)."
    }
    fn doc(&self) -> &'static str {
        "Consumes the lava_locus parameters table; computes pairwise local genetic correlations rho, r², CIs and simulation p-values."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LavaBivarSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(Some(locus_params_schema()))
            .add_output_port(Some(bivar_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(LavaBivarNode::new(serde_json::from_value(spec)?)))
    }
}
#[async_trait]
impl DagNode for LavaBivarNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        BIVAR_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        reporter.status(RuntimeStatus::Running);
        let batches = collect_input_batches(
            inputs.first().ok_or_else(|| {
                reporter.error("bivar: abort — no input port connected");
                LavaNodeError::EmptyInput
            })?,
            BIVAR_KIND,
        )
        .await?;
        let params = parse_locus_params(&batches)?;
        let mut rng = lava::rng(self.spec.rng_seed);
        reporter.info(format!(
            "bivar: start ({} locus table(s), phenos={:?}, target={:?}, p_values={}, \
             cis={}, param_lim={}, rng_seed={})",
            params.len(),
            self.spec.phenos.as_deref().unwrap_or(&[]),
            self.spec.target,
            self.spec.p_values,
            self.spec.cis,
            self.spec.param_lim,
            self.spec.rng_seed,
        ));
        let n_loci = params.len();
        let mut locus_v = Vec::new();
        let mut p1 = Vec::new();
        let mut p2 = Vec::new();
        let mut rho = Vec::new();
        let mut rhol = Vec::new();
        let mut rhoh = Vec::new();
        let mut r2 = Vec::new();
        let mut r2l = Vec::new();
        let mut r2h = Vec::new();
        let mut pv = Vec::new();
        for (idx, lp) in params.iter().enumerate() {
            reporter.progress(idx as u64, n_loci as u64);
            for b in lava::analysis::run_bivar(
                lp,
                self.spec.phenos.as_deref(),
                self.spec.target.as_deref(),
                None,
                self.spec.p_values,
                self.spec.cis,
                self.spec.param_lim,
                true,
                &mut rng,
            ) {
                locus_v.push(lp.id.clone());
                p1.push(b.phen1);
                p2.push(b.phen2);
                rho.push(Some(b.rho));
                rhol.push(Some(b.rho_lower));
                rhoh.push(Some(b.rho_upper));
                r2.push(Some(b.r2));
                r2l.push(Some(b.r2_lower));
                r2h.push(Some(b.r2_upper));
                pv.push(Some(b.p));
            }
        }
        reporter.progress(n_loci as u64, n_loci as u64);
        reporter.info(format!("bivar: done — {} bivariate pairs", p1.len()));
        let batch = RecordBatch::try_new(
            bivar_schema(),
            vec![
                Arc::new(StringArray::from(locus_v)),
                Arc::new(StringArray::from(p1)),
                Arc::new(StringArray::from(p2)),
                Arc::new(Float64Array::from(rho)),
                Arc::new(Float64Array::from(rhol)),
                Arc::new(Float64Array::from(rhoh)),
                Arc::new(Float64Array::from(r2)),
                Arc::new(Float64Array::from(r2l)),
                Arc::new(Float64Array::from(r2h)),
                Arc::new(Float64Array::from(pv)),
            ],
        )
        .map_err(LavaNodeError::Arrow)?;
        read_batch(node_ctx, batch).map_err(Into::into)
    }
}

// ============================ lava_pcor node ============================

const PCOR_KIND: &str = "lava_pcor";
fn pcor_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("locus", DataType::Utf8, false),
        Field::new("phen1", DataType::Utf8, false),
        Field::new("phen2", DataType::Utf8, false),
        Field::new("z", DataType::Utf8, true),
        Field::new("r2_phen1_z", DataType::Float64, true),
        Field::new("r2_phen2_z", DataType::Float64, true),
        Field::new("pcor", DataType::Float64, true),
        Field::new("ci_lower", DataType::Float64, true),
        Field::new("ci_upper", DataType::Float64, true),
        Field::new("p", DataType::Float64, true),
    ]))
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LavaPcorSpec {
    /// The two target phenotypes whose partial correlation is computed.
    pub target: [String; 2],
    /// Conditioner phenotypes (Z). Defaults to all phenotypes except the targets.
    #[serde(default)]
    pub phenos: Option<Vec<String>>,
    #[serde(default = "d_true")]
    pub p_values: bool,
    #[serde(default = "d_true")]
    pub cis: bool,
    #[serde(default = "d_max_r2")]
    pub max_r2: f64,
    #[serde(default = "d_param_lim")]
    pub param_lim: f64,
    #[serde(default = "d_seed")]
    pub rng_seed: u64,
}
fn d_max_r2() -> f64 {
    0.95
}

#[derive(Clone)]
pub struct LavaPcorNode {
    meta: NodePorts,
    spec: LavaPcorSpec,
}
impl LavaPcorNode {
    pub fn new(spec: LavaPcorSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(Some(locus_params_schema()))
                .add_output_port(Some(pcor_schema())),
            spec,
        }
    }
}
pub struct LavaPcorNodeFactory {}
impl NodeFactory for LavaPcorNodeFactory {
    fn kind(&self) -> &'static str {
        PCOR_KIND
    }
    fn desc(&self) -> &'static str {
        "LAVA partial genetic correlation."
    }
    fn doc(&self) -> &'static str {
        "Consumes the lava_locus parameters table; computes the partial local genetic correlation between two target phenotypes conditioned on Z."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LavaPcorSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(Some(locus_params_schema()))
            .add_output_port(Some(pcor_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(LavaPcorNode::new(serde_json::from_value(spec)?)))
    }
}
#[async_trait]
impl DagNode for LavaPcorNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        PCOR_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        reporter.status(RuntimeStatus::Running);
        let batches = collect_input_batches(
            inputs.first().ok_or_else(|| {
                reporter.error("pcor: abort — no input port connected");
                LavaNodeError::EmptyInput
            })?,
            PCOR_KIND,
        )
        .await?;
        let params = parse_locus_params(&batches)?;
        let mut rng = lava::rng(self.spec.rng_seed);
        reporter.info(format!(
            "pcor: start ({} locus table(s), targets=[{}, {}], phenos={:?}, p_values={}, \
             cis={}, max_r2={}, param_lim={}, rng_seed={})",
            params.len(),
            self.spec.target[0],
            self.spec.target[1],
            self.spec.phenos.as_deref().unwrap_or(&[]),
            self.spec.p_values,
            self.spec.cis,
            self.spec.max_r2,
            self.spec.param_lim,
            self.spec.rng_seed,
        ));
        let n_loci = params.len();
        let mut locus_v = Vec::new();
        let mut p1 = Vec::new();
        let mut p2 = Vec::new();
        let mut z = Vec::new();
        let mut r2x = Vec::new();
        let mut r2y = Vec::new();
        let mut pcor = Vec::new();
        let mut cil = Vec::new();
        let mut cih = Vec::new();
        let mut pv = Vec::new();
        for (idx, lp) in params.iter().enumerate() {
            reporter.progress(idx as u64, n_loci as u64);
            let pc = lava::analysis::run_pcor(
                lp,
                (&self.spec.target[0], &self.spec.target[1]),
                self.spec.phenos.as_deref(),
                None,
                self.spec.p_values,
                self.spec.cis,
                self.spec.max_r2,
                self.spec.param_lim,
                &mut rng,
            );
            locus_v.push(lp.id.clone());
            p1.push(pc.phen1);
            p2.push(pc.phen2);
            z.push(pc.z);
            r2x.push(Some(pc.r2_phen1_z));
            r2y.push(Some(pc.r2_phen2_z));
            pcor.push(Some(pc.pcor));
            cil.push(Some(pc.ci_lower));
            cih.push(Some(pc.ci_upper));
            pv.push(Some(pc.p));
        }
        reporter.progress(n_loci as u64, n_loci as u64);
        reporter.info(format!("pcor: done — {} partial-corr rows", p1.len()));
        let batch = RecordBatch::try_new(
            pcor_schema(),
            vec![
                Arc::new(StringArray::from(locus_v)),
                Arc::new(StringArray::from(p1)),
                Arc::new(StringArray::from(p2)),
                Arc::new(StringArray::from(z)),
                Arc::new(Float64Array::from(r2x)),
                Arc::new(Float64Array::from(r2y)),
                Arc::new(Float64Array::from(pcor)),
                Arc::new(Float64Array::from(cil)),
                Arc::new(Float64Array::from(cih)),
                Arc::new(Float64Array::from(pv)),
            ],
        )
        .map_err(LavaNodeError::Arrow)?;
        read_batch(node_ctx, batch).map_err(Into::into)
    }
}

// ============================ lava_multireg node ============================

const MREG_KIND: &str = "lava_multireg";
fn mreg_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("locus", DataType::Utf8, false),
        Field::new("predictors", DataType::Utf8, false),
        Field::new("outcome", DataType::Utf8, false),
        Field::new("gamma", DataType::Float64, true),
        Field::new("gamma_lower", DataType::Float64, true),
        Field::new("gamma_upper", DataType::Float64, true),
        Field::new("r2", DataType::Float64, true),
        Field::new("r2_lower", DataType::Float64, true),
        Field::new("r2_upper", DataType::Float64, true),
        Field::new("p", DataType::Float64, true),
    ]))
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LavaMultiregSpec {
    /// Outcome phenotype.
    pub target: String,
    /// Predictor phenotypes (optional; defaults to all except target).
    #[serde(default)]
    pub phenos: Option<Vec<String>>,
    #[serde(default)]
    pub only_full_model: bool,
    #[serde(default = "d_true")]
    pub p_values: bool,
    #[serde(default = "d_true")]
    pub cis: bool,
    #[serde(default = "d_param_lim_mreg")]
    pub param_lim: f64,
    #[serde(default = "d_seed")]
    pub rng_seed: u64,
}
fn d_param_lim_mreg() -> f64 {
    1.5
}

#[derive(Clone)]
pub struct LavaMultiregNode {
    meta: NodePorts,
    spec: LavaMultiregSpec,
}
impl LavaMultiregNode {
    pub fn new(spec: LavaMultiregSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(Some(locus_params_schema()))
                .add_output_port(Some(mreg_schema())),
            spec,
        }
    }
}
pub struct LavaMultiregNodeFactory {}
impl NodeFactory for LavaMultiregNodeFactory {
    fn kind(&self) -> &'static str {
        MREG_KIND
    }
    fn desc(&self) -> &'static str {
        "LAVA multiple regression of local genetic signal."
    }
    fn doc(&self) -> &'static str {
        "Consumes the lava_locus parameters table; regresses one outcome phenotype's genetic signal on predictor phenotypes (standardized gamma, R²)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LavaMultiregSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(Some(locus_params_schema()))
            .add_output_port(Some(mreg_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(LavaMultiregNode::new(serde_json::from_value(
            spec,
        )?)))
    }
}
#[async_trait]
impl DagNode for LavaMultiregNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        MREG_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        reporter.status(RuntimeStatus::Running);
        let batches = collect_input_batches(
            inputs.first().ok_or_else(|| {
                reporter.error("multireg: abort — no input port connected");
                LavaNodeError::EmptyInput
            })?,
            MREG_KIND,
        )
        .await?;
        let params = parse_locus_params(&batches)?;
        let mut rng = lava::rng(self.spec.rng_seed);
        reporter.info(format!(
            "multireg: start ({} locus table(s), outcome='{}', phenos={:?}, \
             only_full_model={}, p_values={}, cis={}, param_lim={}, rng_seed={})",
            params.len(),
            self.spec.target,
            self.spec.phenos.as_deref().unwrap_or(&[]),
            self.spec.only_full_model,
            self.spec.p_values,
            self.spec.cis,
            self.spec.param_lim,
            self.spec.rng_seed,
        ));
        let n_loci = params.len();
        let mut locus_v = Vec::new();
        let mut pred = Vec::new();
        let mut outc = Vec::new();
        let mut gamma = Vec::new();
        let mut gl = Vec::new();
        let mut gh = Vec::new();
        let mut r2 = Vec::new();
        let mut r2l = Vec::new();
        let mut r2h = Vec::new();
        let mut pv = Vec::new();
        for (idx, lp) in params.iter().enumerate() {
            reporter.progress(idx as u64, n_loci as u64);
            let models = lava::analysis::run_multireg(
                lp,
                &self.spec.target,
                self.spec.phenos.as_deref(),
                None,
                self.spec.only_full_model,
                self.spec.p_values,
                self.spec.cis,
                self.spec.param_lim,
                &mut rng,
            );
            for rows in &models {
                for r in rows {
                    locus_v.push(lp.id.clone());
                    pred.push(r.predictors.clone());
                    outc.push(r.outcome.clone());
                    gamma.push(Some(r.gamma));
                    gl.push(Some(r.gamma_lower));
                    gh.push(Some(r.gamma_upper));
                    r2.push(Some(r.r2));
                    r2l.push(Some(r.r2_lower));
                    r2h.push(Some(r.r2_upper));
                    pv.push(Some(r.p));
                }
            }
        }
        reporter.progress(n_loci as u64, n_loci as u64);
        reporter.info(format!("multireg: done — {} regression rows", pred.len()));
        let batch = RecordBatch::try_new(
            mreg_schema(),
            vec![
                Arc::new(StringArray::from(locus_v)),
                Arc::new(StringArray::from(pred)),
                Arc::new(StringArray::from(outc)),
                Arc::new(Float64Array::from(gamma)),
                Arc::new(Float64Array::from(gl)),
                Arc::new(Float64Array::from(gh)),
                Arc::new(Float64Array::from(r2)),
                Arc::new(Float64Array::from(r2l)),
                Arc::new(Float64Array::from(r2h)),
                Arc::new(Float64Array::from(pv)),
            ],
        )
        .map_err(LavaNodeError::Arrow)?;
        read_batch(node_ctx, batch).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factories_metadata() {
        for (f, kind) in [
            (&LavaLocusNodeFactory {} as &dyn NodeFactory, LOCUS_KIND),
            (&LavaUnivNodeFactory {}, UNIV_KIND),
            (&LavaBivarNodeFactory {}, BIVAR_KIND),
            (&LavaPcorNodeFactory {}, PCOR_KIND),
            (&LavaMultiregNodeFactory {}, MREG_KIND),
        ] {
            assert_eq!(f.kind(), kind);
            let _ = f.spec_schema();
            let _ = f.ports();
            let _ = f.desc();
            let _ = f.doc();
        }
    }

    #[test]
    fn locus_params_roundtrip() {
        // build a 2-pheno LocusParams, flatten to rows, reconstruct, compare.
        let mut omega = faer::Mat::zeros(2, 2);
        omega[(0, 0)] = 1.0;
        omega[(1, 1)] = 2.0;
        omega[(0, 1)] = 0.5;
        omega[(1, 0)] = 0.5;
        let mut sigma = faer::Mat::zeros(2, 2);
        sigma[(0, 0)] = 0.1;
        sigma[(1, 1)] = 0.2;
        sigma[(0, 1)] = 0.01;
        sigma[(1, 0)] = 0.01;
        let lp = lava::locus::LocusParams {
            id: "L".into(),
            chr: Some(1),
            start: Some(10),
            stop: Some(20),
            n_snps: 5,
            k: 3,
            nref_scale: 1.0,
            phenos: vec!["a".into(), "b".into()],
            omega,
            sigma,
            n: vec![100.0, 200.0],
            binary: vec![false, true],
            h2_obs: vec![0.01, f64::NAN],
            h2_latent: vec![f64::NAN, 0.02],
            ascertained_h2: vec![false, true],
        };
        let rows = lp.to_param_rows();
        let back = lava::locus::locus_params_from_rows(&rows);
        assert_eq!(back.len(), 1);
        let b = &back[0];
        assert_eq!(b.id, "L");
        assert_eq!(b.k, 3);
        assert_eq!(b.phenos, vec!["a".to_string(), "b".to_string()]);
        assert!((b.omega[(0, 1)] - 0.5).abs() < 1e-12);
        assert!((b.sigma[(1, 0)] - 0.01).abs() < 1e-12);
        assert!((b.n[1] - 200.0).abs() < 1e-12);
        assert!(b.binary[1]);
        assert!((b.h2_obs[0] - 0.01).abs() < 1e-12);
    }

    /// Smoke test: the node builds from a minimal spec, and the hardcoded
    /// per-chromosome PLINK reference ([`REF_PREFIX_TEMPLATE`]) actually loads
    /// for chromosome 1. Ignored by default — it needs the local 1000G EUR panel
    /// at `/mnt/disk2/dataset/1000g_plink`, which is not available in CI.
    /// Run with: `cargo test -p data-engine -- --ignored load_reference`
    #[tokio::test]
    #[ignore = "needs local 1000G EUR PLINK panel at /mnt/disk2/dataset/1000g_plink"]
    async fn load_reference() {
        let spec = LavaLocusSpec {
            loci: vec![LavaLocus {
                loc: "1:1-1000000".into(),
                chr: 1,
                start: 1,
                stop: 1_000_000,
            }],
            pheno_meta: HashMap::new(),
            sample_overlap: None,
            prune_thresh: d_prune(),
            max_prop_k: d_max_prop_k(),
            min_k: d_min_k(),
        };
        let node = LavaLocusNode::new(spec);
        assert_eq!(node.kind(), LOCUS_KIND);
        assert_eq!(node.ports().input_ports().len(), 1);
        assert_eq!(node.ports().output_ports().len(), 1);

        // The hardcoded reference template must resolve + load chr1.
        let reference = lava::plink::load_reference_template(REF_PREFIX_TEMPLATE, &[1])
            .expect("1000G EUR chr1 reference loads");
        assert!(reference.sample_size > 0, "non-empty .fam sample size");
        assert!(
            !reference.snp_info.snp.is_empty(),
            "chr1 .bim contributed SNPs"
        );
        assert_eq!(
            reference.chr_prefix.get(&1).map(|p| p.to_path_buf()),
            Some(std::path::PathBuf::from(
                "/mnt/disk2/dataset/1000g_plink/eur/chr1/1000G.EUR.chr1.qc"
            ))
        );
    }
}
