//! MRlap DAG node — sample-overlap-aware Mendelian randomisation.
//!
//! Wraps the [`mrlap`] crate's full pipeline (cross-trait LDSC → IVW-MR →
//! de-biasing correction) as a single DAG node with two GWAS sumstat inputs.
//!
//! Mirrors [`super::ldsc_rg::LdscRgNode`] for the LD-score-regression stage
//! (3-way inner join of exposure × outcome × the Iceberg LD-score panel) and
//! [`super::hdl_l::HdlLNode`] for the sumstat parsing + one-row result emission.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::dag::runtime::RuntimeStatus;
use crate::dag::{DagError, graph::PortOutputs};
use crate::node_registry::registry::{NodeCtx, NodeFactory};

const MRLAP_KIND: &str = "mrlap";

// Input column names the node expects on both GWAS ports (lower-case, matching
// the aliases MRlap's tidy_inputGWAS accepts).
const IN_RSID: &str = "rsid";
const IN_Z: &str = "z";
const IN_N: &str = "n";
const IN_CHR: &str = "chr";
const IN_POS: &str = "pos";
const IN_ALT: &str = "alt";
const IN_REF: &str = "ref";

fn result_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("exposure", DataType::Utf8, false),
        Field::new("outcome", DataType::Utf8, false),
        Field::new("observed_effect", DataType::Float64, true),
        Field::new("observed_effect_se", DataType::Float64, true),
        Field::new("observed_effect_p", DataType::Float64, true),
        Field::new("corrected_effect", DataType::Float64, true),
        Field::new("corrected_effect_se", DataType::Float64, true),
        Field::new("corrected_effect_p", DataType::Float64, true),
        Field::new("test_difference", DataType::Float64, true),
        Field::new("p_difference", DataType::Float64, true),
        Field::new("egger_b", DataType::Float64, true),
        Field::new("egger_se", DataType::Float64, true),
        Field::new("egger_intercept_p", DataType::Float64, true),
        Field::new("n_iv", DataType::Int64, false),
        Field::new("h2_exp", DataType::Float64, true),
        Field::new("h2_exp_se", DataType::Float64, true),
        Field::new("h2_out", DataType::Float64, true),
        Field::new("h2_out_se", DataType::Float64, true),
        Field::new("rg", DataType::Float64, true),
        Field::new("int_crosstrait", DataType::Float64, true),
        Field::new("int_crosstrait_se", DataType::Float64, true),
        Field::new("polygenicity", DataType::Float64, true),
        Field::new("per_snp_heritability", DataType::Float64, true),
        Field::new("n_sim", DataType::Int64, false),
    ]))
}

/// Spec for [`MrlapNode`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MrlapSpec {
    pub exposure_name: String,
    pub outcome_name: String,
    /// Iceberg LD-score panel table (`iceberg.ld_score.<table>`), e.g.
    /// `ukbb_eur`. Used for both ref_ld and w_ld (single-annotation baseline).
    #[serde(default = "default_ld_table")]
    pub ld_table: String,
    /// Block-jackknife block count for the LDSC stage.
    #[serde(default = "default_n_blocks")]
    pub n_blocks: usize,
    /// P-value threshold for selecting MR instruments (≤ 1e-5).
    #[serde(default = "default_mr_threshold")]
    pub mr_threshold: f64,
    /// Distance-based pruning window in kb (MRlap default 500).
    #[serde(default = "default_pruning_dist")]
    pub mr_pruning_dist_kb: f64,
    /// Reverse-direction filter p (MRlap default 1e-3; set 0 to disable).
    #[serde(default = "default_mr_reverse")]
    pub mr_reverse: f64,
    /// Seed for the correction parametric-bootstrap RNG.
    #[serde(default = "default_seed")]
    pub seed: u64,
}
fn default_ld_table() -> String {
    "ukbb_eur".into()
}
fn default_n_blocks() -> usize {
    200
}
fn default_mr_threshold() -> f64 {
    5e-8
}
fn default_pruning_dist() -> f64 {
    500.0
}
fn default_mr_reverse() -> f64 {
    1e-3
}
fn default_seed() -> u64 {
    42
}

#[derive(Clone)]
pub struct MrlapNode {
    meta: NodePorts,
    spec: MrlapSpec,
}

impl MrlapNode {
    pub fn new(spec: MrlapSpec) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_input_port(None)
                .add_output_port(Some(result_schema())),
            spec,
        }
    }
}

pub struct MrlapNodeFactory {}

impl NodeFactory for MrlapNodeFactory {
    fn kind(&self) -> &'static str {
        MRLAP_KIND
    }
    fn desc(&self) -> &'static str {
        "MRlap: sample-overlap-aware Mendelian randomisation (cross-trait LDSC + IVW-MR + correction)."
    }
    fn doc(&self) -> &'static str {
        "Reads two GWAS sumstat tables + an Iceberg LD-score panel, runs the \
         full MRlap pipeline: cross-trait LDSC (h², λ, rg), distance-pruned \
         IVW-MR, and the de-biasing correction for sample overlap / weak \
         instruments / Winner's curse. Emits a one-row summary."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MrlapSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_input_port(None)
            .add_output_port(Some(result_schema()))
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(MrlapNode::new(serde_json::from_value(spec)?)))
    }
}

// ---- arrow column helpers ----

fn arr_f64(arr: &dyn Array, i: usize) -> f64 {
    if let Some(a) = arr.as_any().downcast_ref::<Float64Array>() {
        return a.value(i);
    }
    f64::NAN
}

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

fn col_i32(batches: &[RecordBatch], name: &str) -> Option<Vec<Option<i32>>> {
    use arrow_array::{Int32Array, Int64Array as I64};
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        if let Some(a) = col.as_any().downcast_ref::<Int32Array>() {
            for i in 0..a.len() {
                out.push(if a.is_null(i) { None } else { Some(a.value(i)) });
            }
        } else if let Some(a) = col.as_any().downcast_ref::<I64>() {
            for i in 0..a.len() {
                out.push(if a.is_null(i) {
                    None
                } else {
                    Some(a.value(i) as i32)
                });
            }
        } else {
            return None;
        }
    }
    Some(out)
}

fn col_i64(batches: &[RecordBatch], name: &str) -> Option<Vec<Option<i64>>> {
    use arrow_array::{Int32Array, Int64Array as I64};
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name(name)?;
        if let Some(a) = col.as_any().downcast_ref::<I64>() {
            for i in 0..a.len() {
                out.push(if a.is_null(i) { None } else { Some(a.value(i)) });
            }
        } else if let Some(a) = col.as_any().downcast_ref::<Int32Array>() {
            for i in 0..a.len() {
                out.push(if a.is_null(i) {
                    None
                } else {
                    Some(a.value(i) as i64)
                });
            }
        } else {
            return None;
        }
    }
    Some(out)
}

fn err(msg: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: MRLAP_KIND.into(),
        msg: msg.into(),
    }
}

async fn collect_batches(input: &NodeInput) -> Result<Vec<RecordBatch>, DagError> {
    let batches: Vec<RecordBatch> = input
        .data
        .clone()
        .collect()
        .await
        .map_err(|e| err(format!("collect: {e}")))?;
    if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
        return Err(err("empty GWAS input"));
    }
    Ok(batches)
}

/// Parse a GWAS RecordBatch set into [`mrlap::input::RawGwasRow`].
fn parse_gwas(batches: &[RecordBatch]) -> Result<Vec<mrlap::input::RawGwasRow>, DagError> {
    let rsid = col_str(batches, IN_RSID).ok_or_else(|| err("missing rsid column"))?;
    let z = col_f64(batches, IN_Z).ok_or_else(|| err("missing z column"))?;
    let n = col_f64(batches, IN_N).ok_or_else(|| err("missing n column"))?;
    let alt = col_str(batches, IN_ALT).unwrap_or_else(|| vec![String::new(); rsid.len()]);
    let ref_ = col_str(batches, IN_REF).unwrap_or_else(|| vec![String::new(); rsid.len()]);
    let chr = col_i32(batches, IN_CHR).unwrap_or_else(|| vec![None; rsid.len()]);
    let pos = col_i64(batches, IN_POS).unwrap_or_else(|| vec![None; rsid.len()]);
    let mut rows = Vec::with_capacity(rsid.len());
    for i in 0..rsid.len() {
        rows.push(mrlap::input::RawGwasRow {
            rsid: rsid[i].clone(),
            chr: chr[i],
            pos: pos[i],
            alt: alt.get(i).cloned().unwrap_or_default(),
            ref_allele: ref_.get(i).cloned().unwrap_or_default(),
            z: Some(z[i]),
            n: n[i],
            ..Default::default()
        });
    }
    Ok(rows)
}

#[async_trait]
impl DagNode for MrlapNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        MRLAP_KIND
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

        let in0 = inputs
            .first()
            .ok_or_else(|| err("no exposure input (port 0)"))?;
        let in1 = inputs
            .get(1)
            .ok_or_else(|| err("no outcome input (port 1)"))?;
        let b1 = collect_batches(in0).await?;
        let b2 = collect_batches(in1).await?;
        reporter.info(format!(
            "mrlap: {} ({} rows) ~ {} ({} rows)",
            self.spec.exposure_name,
            b1.iter().map(|b| b.num_rows()).sum::<usize>(),
            self.spec.outcome_name,
            b2.iter().map(|b| b.num_rows()).sum::<usize>(),
        ));

        // ---- tidy + harmonise (full sumstats, for MR + correction) ----
        let raw1 = parse_gwas(&b1)?;
        let raw2 = parse_gwas(&b2)?;
        let tidy1 = mrlap::input::tidy(&raw1, true).map_err(|e| err(e.to_string()))?;
        let tidy2 = mrlap::input::tidy(&raw2, true).map_err(|e| err(e.to_string()))?;
        let harm = mrlap::harmonise::harmonise(&tidy1, &tidy2);
        if harm.is_empty() {
            return Err(err("no SNPs survive harmonisation"));
        }

        // ---- LDSC stage: 3-way join via the Iceberg LD panel ----
        let ctx = node_ctx.session();
        ctx.register_table("sumstats1", in0.data.clone().into_view())
            .map_err(|e| err(format!("register sumstats1: {e}")))?;
        ctx.register_table("sumstats2", in1.data.clone().into_view())
            .map_err(|e| err(format!("register sumstats2: {e}")))?;

        // M = total SNPs in the LD panel.
        let m = count_panel_snp(&ctx, &self.spec.ld_table).await?;
        let sql = format!(
            r#"SELECT s1."{z}" AS z1, s2."{z}" AS z2,
                      s1."{n}" AS n1, s2."{n}" AS n2,
                      l.ld_score AS ref_ld, l.ld_score AS w_ld
               FROM sumstats1 AS s1
               INNER JOIN sumstats2 AS s2 ON s1."{rsid}" = s2."{rsid}"
               INNER JOIN iceberg.ld_score.{tbl} AS l ON s1."{rsid}" = l.rsid
               ORDER BY l.locus.position"#,
            z = IN_Z,
            n = IN_N,
            rsid = IN_RSID,
            tbl = self.spec.ld_table,
        );
        let joined = ctx
            .sql(&sql)
            .await
            .map_err(|e| err(format!("ldsc join: {e}")))?;
        let jb = joined
            .collect()
            .await
            .map_err(|e| err(format!("ldsc collect: {e}")))?;
        let z1 = col_f64(&jb, "z1").ok_or_else(|| err("ldsc join missing z1"))?;
        let z2 = col_f64(&jb, "z2").ok_or_else(|| err("ldsc join missing z2"))?;
        let n1 = col_f64(&jb, "n1").ok_or_else(|| err("ldsc join missing n1"))?;
        let n2 = col_f64(&jb, "n2").ok_or_else(|| err("ldsc join missing n2"))?;
        let ref_ld = col_f64(&jb, "ref_ld").ok_or_else(|| err("ldsc join missing ref_ld"))?;
        let w_ld = col_f64(&jb, "wld")
            .or_else(|| col_f64(&jb, "w_ld"))
            .unwrap_or_else(|| ref_ld.clone());
        let n_snp = z1.len();
        if n_snp < 2 {
            return Err(err("LDSC join yielded < 2 shared SNPs"));
        }
        let _ = Mat::from_fn(n_snp, 1, |i, _| ref_ld[i]); // shape sanity
        let ldsc_res = mrlap::ldsc_runner::run_ldsc(&mrlap::ldsc_runner::LdscInput {
            z1: &z1,
            z2: &z2,
            n1: &n1,
            n2: &n2,
            ref_ld: &ref_ld,
            w_ld: &w_ld,
            m: m as f64,
            n_blocks: self.spec.n_blocks,
        })
        .map_err(|e| err(format!("LDSC: {e}")))?;
        reporter.info(format!(
            "mrlap LDSC: h2_exp={:.4}±{:.4} rg={:.3} λ={:.4}±{:.4}",
            ldsc_res.h2_exp, ldsc_res.h2_exp_se, ldsc_res.rg, ldsc_res.lambda, ldsc_res.lambda_se,
        ));

        // ---- MR stage: prune + IVW + Egger ----
        let mr_reverse = if self.spec.mr_reverse > 0.0 {
            Some(self.spec.mr_reverse)
        } else {
            None
        };
        let (ivs, _pruned) = mrlap::pruning::select_instruments(
            &harm,
            &mrlap::pruning::PruneMode::Distance {
                mr_threshold: self.spec.mr_threshold,
                mr_reverse,
                pruning_dist_kb: self.spec.mr_pruning_dist_kb,
            },
        )
        .map_err(|e| err(format!("pruning: {e}")))?;
        if ivs.is_empty() {
            return Err(err("no instruments survived pruning"));
        }
        let mr = mrlap::mr_runner::run_mr(&ivs);
        reporter.info(format!(
            "mrlap MR: {} IVs, observed effect {:.4}±{:.4}",
            ivs.len(),
            mr.alpha_obs,
            mr.alpha_obs_se,
        ));

        // ---- correction ----
        let iv_beta: Vec<f64> = ivs.iter().map(|i| i.std_beta_exp).collect();
        let iv_se: Vec<f64> = ivs.iter().map(|i| i.std_se_exp).collect();
        let corr = mrlap::correction::correct(
            &mrlap::correction::CorrectionInput {
                iv_std_beta_exp: &iv_beta,
                iv_std_se_exp: &iv_se,
                lambda: ldsc_res.lambda,
                lambda_se: ldsc_res.lambda_se,
                h2_ldsc: ldsc_res.h2_exp,
                h2_ldsc_se: ldsc_res.h2_exp_se,
                alpha_obs: mr.alpha_obs,
                alpha_obs_se: mr.alpha_obs_se,
                n_exp: mr.n_exp,
                n_out: mr.n_out,
                mr_threshold: self.spec.mr_threshold,
            },
            self.spec.seed,
        )
        .map_err(|e| err(format!("correction: {e}")))?;
        reporter.info(format!(
            "mrlap corrected: {:.4}±{:.4} (p_diff={:.3e}, {} sims)",
            corr.alpha_corrected, corr.alpha_corrected_se, corr.p_diff, corr.n_sim,
        ));

        // ---- emit one-row result ----
        let f = |x: f64| if x.is_nan() { None } else { Some(x) };
        let batch = RecordBatch::try_new(
            result_schema(),
            vec![
                Arc::new(StringArray::from(vec![self.spec.exposure_name.clone()])),
                Arc::new(StringArray::from(vec![self.spec.outcome_name.clone()])),
                Arc::new(Float64Array::from(vec![f(mr.alpha_obs)])),
                Arc::new(Float64Array::from(vec![f(mr.alpha_obs_se)])),
                Arc::new(Float64Array::from(vec![f(mrlap::input::pnorm2_abs(
                    mr.alpha_obs / mr.alpha_obs_se,
                ))])),
                Arc::new(Float64Array::from(vec![f(corr.alpha_corrected)])),
                Arc::new(Float64Array::from(vec![f(corr.alpha_corrected_se)])),
                Arc::new(Float64Array::from(vec![f(mrlap::input::pnorm2_abs(
                    corr.alpha_corrected / corr.alpha_corrected_se,
                ))])),
                Arc::new(Float64Array::from(vec![f(corr.test_diff)])),
                Arc::new(Float64Array::from(vec![f(corr.p_diff)])),
                Arc::new(Float64Array::from(vec![f(mr.egger_b)])),
                Arc::new(Float64Array::from(vec![f(mr.egger_se)])),
                Arc::new(Float64Array::from(vec![f(mr.egger_intercept_p)])),
                Arc::new(Int64Array::from(vec![ivs.len() as i64])),
                Arc::new(Float64Array::from(vec![f(ldsc_res.h2_exp)])),
                Arc::new(Float64Array::from(vec![f(ldsc_res.h2_exp_se)])),
                Arc::new(Float64Array::from(vec![f(ldsc_res.h2_out)])),
                Arc::new(Float64Array::from(vec![f(ldsc_res.h2_out_se)])),
                Arc::new(Float64Array::from(vec![f(ldsc_res.rg)])),
                Arc::new(Float64Array::from(vec![f(ldsc_res.lambda)])),
                Arc::new(Float64Array::from(vec![f(ldsc_res.lambda_se)])),
                Arc::new(Float64Array::from(vec![f(corr.pi_x)])),
                Arc::new(Float64Array::from(vec![f(corr.sigma2_x)])),
                Arc::new(Int64Array::from(vec![corr.n_sim as i64])),
            ],
        )
        .map_err(|e| err(format!("arrow: {e}")))?;

        let df = ctx
            .read_batch(batch)
            .map_err(|e| err(format!("read_batch: {e}")))?;
        let mut out: PortOutputs = PortOutputs::new();
        out.insert(0, df);
        Ok(out)
    }
}

/// Count rows in the LD-score panel table (derives M for LDSC).
async fn count_panel_snp(
    ctx: &datafusion::prelude::SessionContext,
    ld_table: &str,
) -> Result<usize, DagError> {
    let sql = format!("SELECT COUNT(*) AS n FROM iceberg.ld_score.{ld_table}");
    let df = ctx
        .sql(&sql)
        .await
        .map_err(|e| err(format!("count panel: {e}")))?;
    let batches = df
        .collect()
        .await
        .map_err(|e| err(format!("count collect: {e}")))?;
    let batch = batches
        .first()
        .ok_or_else(|| err("count_panel: no batches"))?;
    let col = batch.column(
        batch
            .schema()
            .index_of("n")
            .map_err(|_| err("count_panel: no n col"))?,
    );
    Ok(match col.data_type() {
        DataType::UInt64 => col
            .as_any()
            .downcast_ref::<arrow_array::UInt64Array>()
            .unwrap()
            .value(0) as usize,
        DataType::Int64 => col.as_any().downcast_ref::<Int64Array>().unwrap().value(0) as usize,
        _ => return Err(err("count_panel: unexpected count type")),
    })
}
