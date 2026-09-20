//! MRlap DAG node — sample-overlap-aware Mendelian randomisation.
//!
//! Wraps the [`mrlap`] crate's full pipeline (cross-trait LDSC → IVW-MR →
//! de-biasing correction) as a single DAG node with two GWAS sumstat inputs.
//!
//! Implements the MRlap-internal LD-score-regression stage
//! (3-way inner join of exposure × outcome × the VFS LD-score panel) and emits
//! the correction results as a one-row summary table.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
    LargeStringArray, RecordBatch, StringArray, StringViewArray, UInt8Array, UInt16Array,
    UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use dag_core::dag::runtime::RuntimeStatus;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, DataBundle, DataBundleBinding, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

const MRLAP_KIND: &str = "mrlap";

// Input column names the node expects on both GWAS ports. EA/NEA are the
// primary allele contract; ALT/REF is retained for older graphs.
const IN_RSID: &str = "rsid";
const IN_CHR: &str = "chr";
const IN_POS: &str = "pos";
const IN_ALT: &str = "alt";
const IN_REF: &str = "ref";
const IN_EA: &str = "ea";
const IN_NEA: &str = "nea";
const IN_BETA: &str = "beta";
const IN_SE: &str = "se";
const IN_OR: &str = "or";
const IN_Z: &str = "z";
const IN_N: &str = "n";

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
/// Hardcoded VFS LD-score panel table used by MRlap's internal regression.

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
    ld_panel: DataBundle,
}

impl MrlapNode {
    pub fn new(spec: MrlapSpec, ld_panel: DataBundle) -> Self {
        Self {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_input_port(None)
                .add_output_port(Some(result_schema())),
            spec,
            ld_panel,
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
        "Reads two GWAS sumstat tables + a VFS LD-score panel, runs the \
         full MRlap pipeline: cross-trait LDSC (h², λ, rg), distance-pruned \
         IVW-MR, and the de-biasing correction for sample overlap / weak \
         instruments / Winner's curse. Each GWAS input requires lowercase \
         rsid, chr, pos, ea, nea, and n columns, plus either z or beta/se \
         (or/se is also accepted). eaf may be present and is ignored by this \
         implementation. Emits a one-row summary."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MrlapSpec)
    }
    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        vec![DataBundleBinding::new(
            "ld_panel",
            nodes_ldsc::ldsc_common::BUNDLE_LDSCORE_1000G_EUR,
        )]
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
        ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Ok(Box::new(MrlapNode::new(
            serde_json::from_value(spec)?,
            ctx.bound_data_bundle("ld_panel")?.clone(),
        )))
    }
}

// ---- arrow column helpers ----

fn primitive_string_value(arr: &dyn Array, index: usize) -> Result<Option<String>, String> {
    if arr.is_null(index) {
        return Ok(None);
    }
    if let Some(values) = arr.as_any().downcast_ref::<StringArray>() {
        Ok(Some(values.value(index).to_string()))
    } else if let Some(values) = arr.as_any().downcast_ref::<LargeStringArray>() {
        Ok(Some(values.value(index).to_string()))
    } else if let Some(values) = arr.as_any().downcast_ref::<StringViewArray>() {
        Ok(Some(values.value(index).to_string()))
    } else {
        Err(format!(
            "expected a string column, found {}",
            arr.data_type()
        ))
    }
}

fn col_str(batches: &[RecordBatch], name: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for b in batches {
        let Some(col) = b.column_by_name(name) else {
            return Err(format!("column `{name}` is missing"));
        };
        for index in 0..col.len() {
            out.push(primitive_string_value(col.as_ref(), index)?.unwrap_or_default());
        }
    }
    Ok(out)
}

fn optional_col_str(
    batches: &[RecordBatch],
    names: &[&str],
) -> Result<Option<Vec<String>>, String> {
    for name in names {
        if batches
            .iter()
            .all(|batch| batch.column_by_name(name).is_some())
        {
            return col_str(batches, name).map(Some);
        }
    }
    Ok(None)
}

fn arr_f64(arr: &dyn Array, i: usize) -> Result<f64, String> {
    macro_rules! integer_value {
        ($array:ident) => {
            if let Some(values) = arr.as_any().downcast_ref::<$array>() {
                return Ok(values.value(i) as f64);
            }
        };
    }
    if let Some(values) = arr.as_any().downcast_ref::<Float64Array>() {
        return Ok(values.value(i));
    }
    if let Some(values) = arr.as_any().downcast_ref::<Float32Array>() {
        return Ok(values.value(i) as f64);
    }
    integer_value!(Int8Array);
    integer_value!(Int16Array);
    integer_value!(Int32Array);
    integer_value!(Int64Array);
    integer_value!(UInt8Array);
    integer_value!(UInt16Array);
    integer_value!(UInt32Array);
    integer_value!(UInt64Array);
    Err(format!(
        "expected a numeric column, found {}",
        arr.data_type()
    ))
}

fn col_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    for b in batches {
        let Some(col) = b.column_by_name(name) else {
            return Err(format!("column `{name}` is missing"));
        };
        for i in 0..col.len() {
            out.push(if col.is_null(i) {
                f64::NAN
            } else {
                arr_f64(col.as_ref(), i)?
            });
        }
    }
    Ok(out)
}

fn optional_col_f64(batches: &[RecordBatch], names: &[&str]) -> Result<Option<Vec<f64>>, String> {
    for name in names {
        if batches
            .iter()
            .all(|batch| batch.column_by_name(name).is_some())
        {
            return col_f64(batches, name).map(Some);
        }
    }
    Ok(None)
}

fn arr_i64(arr: &dyn Array, index: usize) -> Result<Option<i64>, String> {
    if arr.is_null(index) {
        return Ok(None);
    }
    macro_rules! integer_value {
        ($array:ident) => {
            if let Some(values) = arr.as_any().downcast_ref::<$array>() {
                return Ok(Some(values.value(index) as i64));
            }
        };
    }
    integer_value!(Int8Array);
    integer_value!(Int16Array);
    integer_value!(Int32Array);
    integer_value!(Int64Array);
    integer_value!(UInt8Array);
    integer_value!(UInt16Array);
    integer_value!(UInt32Array);
    integer_value!(UInt64Array);
    if let Some(value) = primitive_string_value(arr, index)? {
        let parsed = value
            .trim()
            .parse::<i64>()
            .map_err(|error| format!("invalid integer `{value}`: {error}"))?;
        return Ok(Some(parsed));
    }
    Ok(None)
}

fn col_i64(batches: &[RecordBatch], name: &str) -> Result<Vec<Option<i64>>, String> {
    let mut out = Vec::new();
    for b in batches {
        let Some(col) = b.column_by_name(name) else {
            return Err(format!("column `{name}` is missing"));
        };
        for index in 0..col.len() {
            out.push(arr_i64(col.as_ref(), index)?);
        }
    }
    Ok(out)
}

fn err(msg: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: MRLAP_KIND.into(),
        msg: msg.into(),
    }
}

async fn collect_batches(input: &NodeInput) -> Result<Vec<RecordBatch>, DagError> {
    let batches: Vec<RecordBatch> = input
        .dataframe()?
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
    let rsid = col_str(batches, IN_RSID).map_err(err)?;
    let n = col_f64(batches, IN_N).map_err(err)?;
    let z = optional_col_f64(batches, &[IN_Z]).map_err(err)?;
    let beta = optional_col_f64(batches, &[IN_BETA]).map_err(err)?;
    let odds_ratio = optional_col_f64(batches, &[IN_OR]).map_err(err)?;
    let se = optional_col_f64(batches, &[IN_SE]).map_err(err)?;
    let Some(alt) = optional_col_str(batches, &[IN_EA, IN_ALT]).map_err(err)? else {
        return Err(err("missing ea/nea (or alt/ref) allele columns"));
    };
    let Some(ref_) = optional_col_str(batches, &[IN_NEA, IN_REF]).map_err(err)? else {
        return Err(err("missing ea/nea (or alt/ref) allele columns"));
    };
    let chr = col_i64(batches, IN_CHR).map_err(err)?;
    let pos = col_i64(batches, IN_POS).map_err(err)?;
    let mut rows = Vec::with_capacity(rsid.len());
    for i in 0..rsid.len() {
        let chr = chr
            .get(i)
            .copied()
            .flatten()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| err(format!("row {i}: chr is missing or outside Int32 range")))?;
        let pos = pos
            .get(i)
            .copied()
            .flatten()
            .ok_or_else(|| err(format!("row {i}: pos is missing")))?;
        rows.push(mrlap::input::RawGwasRow {
            rsid: rsid[i].clone(),
            chr: Some(chr),
            pos: Some(pos),
            alt: alt.get(i).cloned().unwrap_or_default(),
            ref_allele: ref_.get(i).cloned().unwrap_or_default(),
            beta: beta.as_ref().and_then(|values| values.get(i).copied()),
            or: odds_ratio
                .as_ref()
                .and_then(|values| values.get(i).copied()),
            se: se.as_ref().and_then(|values| values.get(i).copied()),
            z: z.as_ref().and_then(|values| values.get(i).copied()),
            n: n[i],
        });
    }
    Ok(rows)
}

fn column_type(batches: &[RecordBatch], name: &str) -> String {
    batches
        .iter()
        .find_map(|batch| batch.column_by_name(name))
        .map(|column| column.data_type().to_string())
        .unwrap_or_else(|| "missing".into())
}

fn no_harmonised_snps_message(
    b1: &[RecordBatch],
    b2: &[RecordBatch],
    tidy1: &[mrlap::input::TidyRow],
    tidy2: &[mrlap::input::TidyRow],
) -> String {
    let keys1: HashSet<&str> = tidy1.iter().map(|row| row.rsid.as_str()).collect();
    let keys2: HashSet<&str> = tidy2.iter().map(|row| row.rsid.as_str()).collect();
    let shared = keys1.intersection(&keys2).count();
    let outcomes: HashMap<&str, Vec<&mrlap::input::TidyRow>> = {
        let mut map: HashMap<&str, Vec<&mrlap::input::TidyRow>> =
            HashMap::with_capacity(tidy2.len());
        for row in tidy2 {
            map.entry(row.rsid.as_str()).or_default().push(row);
        }
        map
    };
    let allele_aligned = tidy1
        .iter()
        .filter_map(|exposure| {
            outcomes
                .get(exposure.rsid.as_str())
                .map(|rows| (exposure, rows))
        })
        .flat_map(|(exposure, rows)| {
            rows.iter().filter(|outcome| {
                (exposure.alt == outcome.alt && exposure.ref_allele == outcome.ref_allele)
                    || (exposure.ref_allele == outcome.alt && exposure.alt == outcome.ref_allele)
            })
        })
        .count();

    format!(
        "no SNPs survive harmonisation: exposure={} valid rows / {} unique rsids, outcome={} valid rows / {} unique rsids, shared rsids={}, allele-aligned joins={}; rsid types: exposure={}, outcome={}",
        tidy1.len(),
        keys1.len(),
        tidy2.len(),
        keys2.len(),
        shared,
        allele_aligned,
        column_type(b1, IN_RSID),
        column_type(b2, IN_RSID),
    )
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
        reporter: &dag_core::dag::node_event::NodeReporter,
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
            return Err(err(no_harmonised_snps_message(&b1, &b2, &tidy1, &tidy2)));
        }

        // ---- LDSC stage: 3-way join via the VFS LD panel ----
        let ctx = node_ctx.session();
        ctx.register_table("sumstats1", in0.dataframe()?.clone().into_view())
            .map_err(|e| err(format!("register sumstats1: {e}")))?;
        ctx.register_table("sumstats2", in1.dataframe()?.clone().into_view())
            .map_err(|e| err(format!("register sumstats2: {e}")))?;

        nodes_ldsc::ldsc_common::register_listing_table(
            &ctx,
            "ld_panel",
            &nodes_ldsc::ldsc_common::storage_url(&self.ld_panel),
        )
        .await
        .map_err(|e| err(format!("register ld panel: {e}")))?;

        // M = total SNPs in the LD panel.
        let m = count_panel_snp(&ctx, "ld_panel").await?;
        let sql = format!(
            r#"SELECT CAST(s1."{z}" AS DOUBLE) AS z1, CAST(s2."{z}" AS DOUBLE) AS z2,
                      CAST(s1."{n}" AS DOUBLE) AS n1, CAST(s2."{n}" AS DOUBLE) AS n2,
                      CAST(l.ld_score AS DOUBLE) AS ref_ld,
                      CAST(l.w_ld AS DOUBLE) AS w_ld
               FROM sumstats1 AS s1
               INNER JOIN sumstats2 AS s2
                 ON CAST(s1."{rsid}" AS VARCHAR) = CAST(s2."{rsid}" AS VARCHAR)
               INNER JOIN {tbl} AS l
                 ON CAST(s1."{rsid}" AS VARCHAR) = CAST(l.rsid AS VARCHAR)
               ORDER BY l.locus.position"#,
            z = IN_Z,
            n = IN_N,
            rsid = IN_RSID,
            tbl = nodes_ldsc::ldsc_common::quote_table("ld_panel"),
        );
        let joined = ctx
            .sql(&sql)
            .await
            .map_err(|e| err(format!("ldsc join: {e}")))?;
        let jb = joined
            .collect()
            .await
            .map_err(|e| err(format!("ldsc collect: {e}")))?;
        let z1 = col_f64(&jb, "z1").map_err(err)?;
        let z2 = col_f64(&jb, "z2").map_err(err)?;
        let n1 = col_f64(&jb, "n1").map_err(err)?;
        let n2 = col_f64(&jb, "n2").map_err(err)?;
        let ref_ld = col_f64(&jb, "ref_ld").map_err(err)?;
        let w_ld = col_f64(&jb, "w_ld").map_err(err)?;
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
    ld_table_sql: &str,
) -> Result<usize, DagError> {
    let sql = format!(
        "SELECT COUNT(*) AS n FROM {}",
        nodes_ldsc::ldsc_common::quote_table(ld_table_sql)
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::UInt32Array;
    use arrow_schema::{Field, Schema};

    #[tokio::test]
    async fn missing_inputs_yield_clear_error() {
        let mut node = MrlapNode::new(
            serde_json::from_value::<MrlapSpec>(serde_json::json!({
                "exposure_name": "exposure",
                "outcome_name": "outcome"
            }))
            .unwrap(),
            DataBundle::new(
                nodes_ldsc::ldsc_common::BUNDLE_LDSCORE_1000G_EUR,
                "1000G LD Scores",
                "/bundles/ldsc/1000g.parquet",
            ),
        );

        let error = node
            .execute(
                &NodeCtx::new(
                    datafusion::prelude::SessionContext::new().runtime_env(),
                    None,
                ),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("no exposure input"));
    }

    #[test]
    fn parses_primary_gwas_contract_with_string_view_keys() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("rsid", DataType::Utf8View, false),
                Field::new("chr", DataType::Utf8View, false),
                Field::new("pos", DataType::UInt32, false),
                Field::new("ea", DataType::Utf8View, false),
                Field::new("nea", DataType::LargeUtf8, false),
                Field::new("beta", DataType::Float64, false),
                Field::new("se", DataType::Float64, false),
                Field::new("n", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringViewArray::from(vec!["rs1"])),
                Arc::new(StringViewArray::from(vec!["1"])),
                Arc::new(UInt32Array::from(vec![101])),
                Arc::new(StringViewArray::from(vec!["A"])),
                Arc::new(LargeStringArray::from(vec!["G"])),
                Arc::new(Float64Array::from(vec![2.5])),
                Arc::new(Float64Array::from(vec![1.0])),
                Arc::new(Float64Array::from(vec![100.0])),
            ],
        )
        .unwrap();

        let raw = parse_gwas(&[batch]).unwrap();
        let tidy = mrlap::input::tidy(&raw, true).unwrap();

        assert_eq!(tidy.len(), 1);
        assert_eq!(tidy[0].rsid, "rs1");
        assert_eq!(tidy[0].chr, Some(1));
        assert_eq!(tidy[0].pos, Some(101));
        assert_eq!(tidy[0].alt, "A");
        assert_eq!(tidy[0].ref_allele, "G");
        assert_eq!(tidy[0].z, 2.5);
    }

    #[test]
    fn empty_harmonisation_diagnostic_reports_key_overlap() {
        let make_batch = |rsid: ArrayRef, ea: ArrayRef, nea: ArrayRef| {
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("rsid", rsid.data_type().clone(), false),
                    Field::new("chr", DataType::Int64, false),
                    Field::new("pos", DataType::Int64, false),
                    Field::new("ea", ea.data_type().clone(), false),
                    Field::new("nea", nea.data_type().clone(), false),
                    Field::new("z", DataType::Float64, false),
                    Field::new("n", DataType::Float64, false),
                ])),
                vec![
                    rsid,
                    Arc::new(Int64Array::from(vec![1])),
                    Arc::new(Int64Array::from(vec![101])),
                    ea,
                    nea,
                    Arc::new(Float64Array::from(vec![1.0])),
                    Arc::new(Float64Array::from(vec![100.0])),
                ],
            )
            .unwrap()
        };
        let b1 = make_batch(
            Arc::new(StringViewArray::from(vec!["rs1"])),
            Arc::new(StringArray::from(vec!["A"])),
            Arc::new(StringArray::from(vec!["G"])),
        );
        let b2 = make_batch(
            Arc::new(LargeStringArray::from(vec!["rs1"])),
            Arc::new(StringArray::from(vec!["C"])),
            Arc::new(StringArray::from(vec!["T"])),
        );
        let tidy1 =
            mrlap::input::tidy(&parse_gwas(std::slice::from_ref(&b1)).unwrap(), true).unwrap();
        let tidy2 =
            mrlap::input::tidy(&parse_gwas(std::slice::from_ref(&b2)).unwrap(), true).unwrap();

        let message = no_harmonised_snps_message(&[b1], &[b2], &tidy1, &tidy2);

        assert!(message.contains("shared rsids=1"), "{message}");
        assert!(message.contains("allele-aligned joins=0"), "{message}");
        assert!(message.contains("Utf8View"), "{message}");
        assert!(message.contains("LargeUtf8"), "{message}");
    }
}
