//! MRlap DAG node — sample-overlap-aware Mendelian randomisation.
//!
//! Wraps the [`mrlap`] crate's full pipeline (cross-trait LDSC → IVW-MR →
//! de-biasing correction) as a single DAG node with two GWAS sumstat inputs.
//!
//! Implements the MRlap-internal LD-score-regression stage (inner join of the
//! allele-harmonised SNP table × the VFS LD-score panel) and emits the
//! correction results as a one-row summary table.
//!
//! # Unvalidated deviations from official MRlap `run_LDSC.R`
//!
//! These differences are documented, not fixed — validating them requires the
//! official R pipeline end-to-end:
//!
//! - the LD-score panel is the fixed 1000G EUR bundle, not a
//!   population-matched panel per trait;
//! - M (total SNP count) is the LD panel row count, not the munged-panel M
//!   the official pipeline derives;
//! - jackknife blocks are contiguous slices of position-sorted rows, not the
//!   official block assignment over the merged panel.
//!
//! Palindromic-SNP handling by allele frequency (the eaf-based ambiguity
//! rules of a full harmonisation scheme) is likewise not implemented; `eaf`
//! columns are only used for a consistency diagnostic.

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
const IN_EAF: &str = "eaf";

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
         (or/se is also accepted); the same derived z and allele-alignment \
         direction are used by the LDSC and MR stages. eaf, when present on \
         both inputs, is used only for an allele-frequency consistency \
         diagnostic (palindromic-SNP rules are not implemented). The LD \
         panel is the fixed 1000G EUR bundle and M is its row count — \
         deviations from official MRlap run_LDSC.R are documented in the \
         module docs and unvalidated. Emits a one-row summary."
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

// ---- LDSC stage: join the harmonised SNP table with the LD panel ----

/// Per-SNP columns fed to the cross-trait LDSC regression.
struct LdscColumns {
    z1: Vec<f64>,
    z2: Vec<f64>,
    n1: Vec<f64>,
    n2: Vec<f64>,
    ref_ld: Vec<f64>,
    w_ld: Vec<f64>,
}

/// Build the harmonised per-SNP table consumed by the LDSC join.
///
/// z is recovered as `std_beta * sqrt(n)` — exactly the standardised
/// quantities the MR stage consumes — so the z-scores reaching LDSC are
/// same-source and same-direction as MR by construction: the outcome column
/// already carries the allele-alignment flip applied by
/// [`mrlap::harmonise::harmonise`], and beta/se-only or or/se-only inputs
/// (which have no `z` column) are handled by the same tidy derivation
/// instead of being read from the raw sumstat tables.
fn harmonised_ldsc_batch(
    harm: &[mrlap::harmonise::HarmonisedRow],
) -> Result<RecordBatch, DagError> {
    let n_rows = harm.len();
    let mut rsid = Vec::with_capacity(n_rows);
    let mut z1 = Vec::with_capacity(n_rows);
    let mut z2 = Vec::with_capacity(n_rows);
    let mut n1 = Vec::with_capacity(n_rows);
    let mut n2 = Vec::with_capacity(n_rows);
    for row in harm {
        rsid.push(row.rsid.clone());
        z1.push(row.std_beta_exp * row.n_exp.sqrt());
        z2.push(row.std_beta_out * row.n_out.sqrt());
        n1.push(row.n_exp);
        n2.push(row.n_out);
    }
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("z1", DataType::Float64, false),
            Field::new("z2", DataType::Float64, false),
            Field::new("n1", DataType::Float64, false),
            Field::new("n2", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(rsid)),
            Arc::new(Float64Array::from(z1)),
            Arc::new(Float64Array::from(z2)),
            Arc::new(Float64Array::from(n1)),
            Arc::new(Float64Array::from(n2)),
        ],
    )
    .map_err(|e| err(format!("harmonised LDSC table: {e}")))?;
    Ok(batch)
}

/// Join the harmonised SNP table with the LD-score panel and collect the
/// per-SNP LDSC columns (ordered by panel position).
async fn ldsc_join_columns(
    ctx: &datafusion::prelude::SessionContext,
    harm: &[mrlap::harmonise::HarmonisedRow],
    panel_table: &str,
) -> Result<LdscColumns, DagError> {
    let batch = harmonised_ldsc_batch(harm)?;
    let df = ctx
        .read_batch(batch)
        .map_err(|e| err(format!("harmonised table: {e}")))?;
    ctx.register_table("mrlap_harmonised", df.into_view())
        .map_err(|e| err(format!("register harmonised table: {e}")))?;
    let sql = format!(
        r#"SELECT h."z1" AS z1, h."z2" AS z2,
                  h."n1" AS n1, h."n2" AS n2,
                  CAST(l.ld_score AS DOUBLE) AS ref_ld,
                  CAST(l.w_ld AS DOUBLE) AS w_ld
           FROM mrlap_harmonised AS h
           INNER JOIN {tbl} AS l
             ON CAST(h."rsid" AS VARCHAR) = CAST(l.rsid AS VARCHAR)
           ORDER BY l.locus.position"#,
        tbl = nodes_ldsc::ldsc_common::quote_table(panel_table),
    );
    let joined = ctx
        .sql(&sql)
        .await
        .map_err(|e| err(format!("ldsc join: {e}")))?;
    let jb = joined
        .collect()
        .await
        .map_err(|e| err(format!("ldsc collect: {e}")))?;
    Ok(LdscColumns {
        z1: col_f64(&jb, "z1").map_err(err)?,
        z2: col_f64(&jb, "z2").map_err(err)?,
        n1: col_f64(&jb, "n1").map_err(err)?,
        n2: col_f64(&jb, "n2").map_err(err)?,
        ref_ld: col_f64(&jb, "ref_ld").map_err(err)?,
        w_ld: col_f64(&jb, "w_ld").map_err(err)?,
    })
}

/// eaf consistency diagnostic over the harmonised SNPs.
///
/// Full palindromic-SNP / allele-ambiguity rules are a scheme-level concern
/// and deliberately NOT implemented here; this only counts harmonised SNPs
/// whose effect-allele frequencies point in opposite directions
/// (|eaf_exp − eaf_out| > 0.5), which a real frequency-aware harmonisation
/// would resolve. Returns `None` when either input lacks an `eaf` column.
fn eaf_discordance_diagnostic(
    b1: &[RecordBatch],
    b2: &[RecordBatch],
    harm: &[mrlap::harmonise::HarmonisedRow],
) -> Result<Option<(usize, usize)>, DagError> {
    let e1 = optional_col_f64(b1, &[IN_EAF]).map_err(err)?;
    let e2 = optional_col_f64(b2, &[IN_EAF]).map_err(err)?;
    let (Some(e1), Some(e2)) = (e1, e2) else {
        return Ok(None);
    };
    let r1 = col_str(b1, IN_RSID).map_err(err)?;
    let r2 = col_str(b2, IN_RSID).map_err(err)?;
    let map1: HashMap<&str, f64> = r1
        .iter()
        .zip(e1.iter())
        .filter(|(_, e)| e.is_finite())
        .map(|(r, e)| (r.as_str(), *e))
        .collect();
    let map2: HashMap<&str, f64> = r2
        .iter()
        .zip(e2.iter())
        .filter(|(_, e)| e.is_finite())
        .map(|(r, e)| (r.as_str(), *e))
        .collect();
    let mut discordant = 0usize;
    let mut checked = 0usize;
    for row in harm {
        if let (Some(a), Some(b)) = (map1.get(row.rsid.as_str()), map2.get(row.rsid.as_str())) {
            checked += 1;
            if (a - b).abs() > 0.5 {
                discordant += 1;
            }
        }
    }
    Ok(Some((discordant, checked)))
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

        // ---- LDSC stage: harmonised SNP table joined with the VFS LD panel ----
        // The join consumes the harmonised rows (z already aligned to the
        // exposure effect allele), so the cross-trait z products used by LDSC
        // are same-source and same-direction as the MR stage, and beta/se-only
        // inputs reach LDSC through the tidy derivation instead of requiring a
        // raw `z` column.
        let ctx = node_ctx.session();
        nodes_ldsc::ldsc_common::register_listing_table(
            &ctx,
            "ld_panel",
            &nodes_ldsc::ldsc_common::storage_url(&self.ld_panel),
        )
        .await
        .map_err(|e| err(format!("register ld panel: {e}")))?;

        if let Some((discordant, checked)) = eaf_discordance_diagnostic(&b1, &b2, &harm)? {
            let message = format!(
                "mrlap eaf diagnostic: {discordant}/{checked} harmonised SNPs have \
                 |eaf_exp - eaf_out| > 0.5; palindromic-SNP frequency rules are not \
                 implemented (see module docs)"
            );
            if discordant > 0 {
                reporter.warn(message);
            } else if checked > 0 {
                reporter.info(message);
            }
        }

        // M = total SNPs in the LD panel.
        let m = count_panel_snp(&ctx, "ld_panel").await?;
        let cols = ldsc_join_columns(&ctx, &harm, "ld_panel").await?;
        let LdscColumns {
            z1,
            z2,
            n1,
            n2,
            ref_ld,
            w_ld,
        } = cols;
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

    // ---- in-memory LDSC SQL harness ----

    /// Register a mock LD-score panel (rsid, ld_score, w_ld, locus.position)
    /// whose positions deliberately do not follow rsid sort order.
    fn register_mock_ld_panel(ctx: &datafusion::prelude::SessionContext) {
        use arrow_array::{Array, Int64Array, StructArray};
        use datafusion::catalog::MemTable;

        let rows: &[(&str, i64, f64, f64)] = &[
            ("rs9", 100, 1.0, 1.0),
            ("rs1", 110, 1.5, 0.5),
            ("rs5", 120, 2.0, 0.6),
            ("rs2", 130, 2.5, 0.7),
            ("rs3", 140, 3.0, 0.8),
        ];
        let position_field = Arc::new(Field::new("position", DataType::Int64, false));
        let locus = StructArray::new(
            vec![position_field].into(),
            vec![Arc::new(Int64Array::from(
                rows.iter().map(|(_, pos, _, _)| *pos).collect::<Vec<_>>(),
            )) as Arc<dyn Array>],
            None,
        );
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("rsid", DataType::Utf8, false),
                Field::new("ld_score", DataType::Float64, false),
                Field::new("w_ld", DataType::Float64, false),
                Field::new(
                    "locus",
                    DataType::Struct(
                        vec![Arc::new(Field::new("position", DataType::Int64, false))].into(),
                    ),
                    false,
                ),
            ])),
            vec![
                Arc::new(StringArray::from(
                    rows.iter().map(|(rsid, _, _, _)| *rsid).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    rows.iter().map(|(_, _, ld, _)| *ld).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    rows.iter().map(|(_, _, _, w)| *w).collect::<Vec<_>>(),
                )),
                Arc::new(locus) as Arc<dyn Array>,
            ],
        )
        .unwrap();
        let table =
            MemTable::try_new(batch.schema(), vec![vec![batch]]).expect("mock LD panel schema");
        ctx.register_table("mock_panel", Arc::new(table))
            .expect("register mock panel");
    }

    /// Build a minimal GWAS batch: effect given either as beta/se or as z.
    #[allow(clippy::too_many_arguments)]
    fn gwas_batch(
        rsids: &[&str],
        alleles: &[(&str, &str)],
        beta: Option<&[f64]>,
        se: Option<&[f64]>,
        z: Option<&[f64]>,
        eaf: Option<&[f64]>,
        n: f64,
    ) -> RecordBatch {
        let mut fields = vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("chr", DataType::Int64, false),
            Field::new("pos", DataType::Int64, false),
            Field::new("ea", DataType::Utf8, false),
            Field::new("nea", DataType::Utf8, false),
        ];
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(rsids.to_vec())),
            Arc::new(Int64Array::from(vec![1; rsids.len()])),
            Arc::new(Int64Array::from(
                (1000..1000 + rsids.len() as i64).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                alleles.iter().map(|(ea, _)| *ea).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                alleles.iter().map(|(_, nea)| *nea).collect::<Vec<_>>(),
            )),
        ];
        if let (Some(beta), Some(se)) = (beta, se) {
            fields.push(Field::new("beta", DataType::Float64, false));
            columns.push(Arc::new(Float64Array::from(beta.to_vec())));
            fields.push(Field::new("se", DataType::Float64, false));
            columns.push(Arc::new(Float64Array::from(se.to_vec())));
        }
        if let Some(z) = z {
            fields.push(Field::new("z", DataType::Float64, false));
            columns.push(Arc::new(Float64Array::from(z.to_vec())));
        }
        if let Some(eaf) = eaf {
            fields.push(Field::new("eaf", DataType::Float64, false));
            columns.push(Arc::new(Float64Array::from(eaf.to_vec())));
        }
        fields.push(Field::new("n", DataType::Float64, false));
        columns.push(Arc::new(Float64Array::from(vec![n; rsids.len()])));

        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
    }

    fn harmonise_batches(
        b1: &RecordBatch,
        b2: &RecordBatch,
    ) -> Vec<mrlap::harmonise::HarmonisedRow> {
        let raw1 = parse_gwas(std::slice::from_ref(b1)).unwrap();
        let raw2 = parse_gwas(std::slice::from_ref(b2)).unwrap();
        let tidy1 = mrlap::input::tidy(&raw1, true).unwrap();
        let tidy2 = mrlap::input::tidy(&raw2, true).unwrap();
        mrlap::harmonise::harmonise(&tidy1, &tidy2)
    }

    #[tokio::test]
    async fn ldsc_join_derives_z_from_beta_se_and_flips_swapped_alleles() {
        let ctx = datafusion::prelude::SessionContext::new();
        register_mock_ld_panel(&ctx);

        // beta/se-only inputs (no z column): previously the LDSC SQL failed
        // on the missing `z` column even though parse_gwas accepts beta/se.
        let exposure = gwas_batch(
            &["rs1", "rs2", "rs3"],
            &[("A", "G"), ("A", "G"), ("A", "G")],
            Some(&[2.0, 3.0, -1.0]),
            Some(&[1.0, 2.0, 2.0]),
            None,
            None,
            100.0,
        );
        // rs2 has swapped effect alleles in the outcome → harmonise must flip
        // its z before it reaches the cross-trait LDSC products.
        let outcome = gwas_batch(
            &["rs1", "rs2", "rs3"],
            &[("A", "G"), ("G", "A"), ("A", "G")],
            Some(&[1.0, 1.5, -5.0]),
            Some(&[1.0, 2.0, 4.0]),
            None,
            None,
            400.0,
        );

        let harm = harmonise_batches(&exposure, &outcome);
        assert_eq!(harm.len(), 3);
        let cols = ldsc_join_columns(&ctx, &harm, "mock_panel").await.unwrap();

        // Panel-position order: rs1 (110) < rs2 (130) < rs3 (140).
        for (got, want) in cols.z1.iter().zip([2.0, 1.5, -0.5]) {
            assert!(
                (got - want).abs() < 1e-12,
                "z1 derived from beta/se: got {got}, want {want}"
            );
        }
        for (got, want) in cols.z2.iter().zip([1.0, -0.75, -1.25]) {
            assert!(
                (got - want).abs() < 1e-12,
                "z2 must carry the harmonisation flip (rs2 swapped): got {got}, want {want}"
            );
        }
        assert!(cols.n1.iter().all(|v| (*v - 100.0).abs() < 1e-12));
        assert!(cols.n2.iter().all(|v| (*v - 400.0).abs() < 1e-12));
        // Only panel SNPs survive the join, with panel LD columns attached.
        for (got, want) in cols.ref_ld.iter().zip([1.5, 2.5, 3.0]) {
            assert!((got - want).abs() < 1e-12);
        }
    }

    #[tokio::test]
    async fn ldsc_join_applies_the_same_flip_to_z_column_inputs() {
        let ctx = datafusion::prelude::SessionContext::new();
        register_mock_ld_panel(&ctx);

        let exposure = gwas_batch(
            &["rs1", "rs2", "rs3"],
            &[("A", "G"), ("A", "G"), ("A", "G")],
            None,
            None,
            Some(&[2.0, 1.5, -0.5]),
            None,
            100.0,
        );
        // rs2 outcome z is +0.75 on swapped alleles → LDSC must see −0.75,
        // i.e. the z-column path goes through the same alignment as MR.
        let outcome = gwas_batch(
            &["rs1", "rs2", "rs3"],
            &[("A", "G"), ("G", "A"), ("A", "G")],
            None,
            None,
            Some(&[1.0, 0.75, -1.25]),
            None,
            400.0,
        );

        let harm = harmonise_batches(&exposure, &outcome);
        let cols = ldsc_join_columns(&ctx, &harm, "mock_panel").await.unwrap();

        for (got, want) in cols.z1.iter().zip([2.0, 1.5, -0.5]) {
            assert!((got - want).abs() < 1e-12, "z1: got {got}, want {want}");
        }
        for (got, want) in cols.z2.iter().zip([1.0, -0.75, -1.25]) {
            assert!(
                (got - want).abs() < 1e-12,
                "z2 flip from z column: got {got}, want {want}"
            );
        }
    }

    #[test]
    fn eaf_diagnostic_counts_discordant_frequencies_only_when_present() {
        // Exposure eaf ~ T allele freq; outcome rs2 reports the freq of the
        // opposite allele (swapped alleles) → |eaf diff| > 0.5 for rs2.
        let exposure = gwas_batch(
            &["rs1", "rs2", "rs3"],
            &[("A", "G"), ("A", "G"), ("A", "G")],
            None,
            None,
            Some(&[2.0, 1.5, -0.5]),
            Some(&[0.9, 0.8, 0.3]),
            100.0,
        );
        let outcome = gwas_batch(
            &["rs1", "rs2", "rs3"],
            &[("A", "G"), ("G", "A"), ("A", "G")],
            None,
            None,
            Some(&[1.0, 0.75, -1.25]),
            Some(&[0.85, 0.75, 0.35]),
            400.0,
        );
        let harm = harmonise_batches(&exposure, &outcome);
        let diagnostic =
            eaf_discordance_diagnostic(&[exposure.clone()], &[outcome.clone()], &harm).unwrap();
        assert_eq!(diagnostic, Some((0, 3)), "no discordance in this fixture");

        let outcome_flipped = gwas_batch(
            &["rs1", "rs2", "rs3"],
            &[("A", "G"), ("G", "A"), ("A", "G")],
            None,
            None,
            Some(&[1.0, 0.75, -1.25]),
            Some(&[0.85, 0.1, 0.35]),
            400.0,
        );
        let harm = harmonise_batches(&exposure, &outcome_flipped);
        let diagnostic =
            eaf_discordance_diagnostic(&[exposure.clone()], &[outcome_flipped.clone()], &harm)
                .unwrap();
        assert_eq!(diagnostic, Some((1, 3)), "rs2 |0.8 - 0.1| > 0.5");

        // No eaf columns on the outcome → diagnostic is unavailable.
        let outcome_no_eaf = gwas_batch(
            &["rs1", "rs2", "rs3"],
            &[("A", "G"), ("G", "A"), ("A", "G")],
            None,
            None,
            Some(&[1.0, 0.75, -1.25]),
            None,
            400.0,
        );
        let harm = harmonise_batches(&exposure, &outcome_no_eaf);
        assert!(
            eaf_discordance_diagnostic(&[exposure], &[outcome_no_eaf], &harm)
                .unwrap()
                .is_none()
        );
    }

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
