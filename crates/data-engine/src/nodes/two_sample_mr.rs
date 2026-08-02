//! Mendelian randomisation (MR) transform node.
//!
//! Wraps the pure-Rust [`mr`] crate (a port of TwoSampleMR's algorithm API).
//!
//! Takes a single upstream `DataFrame` whose rows are **already merged on SNP**
//! — one record per SNP, carrying both the exposure and the outcome summary
//! statistics (effect alleles, betas, SEs, effect allele frequencies). The
//! exposure and outcome trait identifiers (`id_exposure` / `id_outcome`) are
//! supplied as spec parameters, not as DataFrame columns — they are constant
//! across the whole run and would otherwise be duplicated once per SNP row.
//!
//! Performs **LD clumping** via the OpenGWAS `/ld/clump` endpoint
//! (1000 Genomes reference panel) to ensure instrument independence before
//! harmonisation. Exposure p-values are computed from `beta_exposure /
//! se_exposure` and sent to OpenGWAS; only the returned index SNPs are
//! retained. This requires the `OPENGWAS_TOKEN` environment variable.
//!
//! The node then runs allele harmonisation
//! ([`mr::harmonise::harmonise_data_with`]) and the main MR dispatch
//! ([`mr::dispatch::mr`]) over the requested methods, emitting one row per
//! method estimate.
//!
//! The upstream merge-on-SNP is expected to be done upstream (e.g. via a SQL
//! node); this node deliberately stays single-input.

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    StringArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
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

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum TwoSampleMrNodeError {
    #[error("MR computation failed: {0}")]
    Mr(#[from] mr::MrError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("column '{name}' is not the expected type (got {dtype})")]
    WrongColumnType { name: String, dtype: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
    #[error("column length mismatch: '{name}' has {len} rows, expected {expected}")]
    LengthMismatch {
        name: String,
        len: usize,
        expected: usize,
    },
    #[error("LD clumping failed: {0}")]
    Clump(String),
}

impl From<TwoSampleMrNodeError> for DagError {
    fn from(e: TwoSampleMrNodeError) -> Self {
        DagError::NodeError {
            node_type: TWO_SAMPLE_MR_NODE_KIND.to_string(),
            msg: e.to_string(),
        }
    }
}

// =====================================================================
// Fixed input / output schemas
// =====================================================================

/// Required input column names (TwoSampleMR conventions). The upstream
/// `DataFrame` must expose exactly these names — enforced by [`input_schema`]
/// so the DAG rejects mis-shaped edges at `add_edge` time.
const IN_SNP: &str = "snp";
const IN_BETA_EXP: &str = "beta_exposure";
const IN_BETA_OUT: &str = "beta_outcome";
const IN_SE_EXP: &str = "se_exposure";
const IN_SE_OUT: &str = "se_outcome";
const IN_EA_EXP: &str = "effect_allele_exposure";
const IN_OA_EXP: &str = "other_allele_exposure";
const IN_EA_OUT: &str = "effect_allele_outcome";
const IN_OA_OUT: &str = "other_allele_outcome";
const IN_EAF_EXP: &str = "eaf_exposure";
const IN_EAF_OUT: &str = "eaf_outcome";

/// The fixed input port schema: per-SNP exposure+outcome summary stats, already
/// merged on SNP. Effect-allele / other-allele / EAF columns are nullable (R's
/// `NA`); betas/SEs are Float64 with null read as NaN (the `mr` crate's NA
/// convention).
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(IN_SNP, DataType::Utf8, false).with_metadata(std::collections::HashMap::from([
            (
                "doc".to_string(),
                "SNP rsID identifier; join key between exposure and outcome.".to_string(),
            ),
        ])),
        Field::new(IN_BETA_EXP, DataType::Float64, true).with_metadata(
            std::collections::HashMap::from([(
                "doc".to_string(),
                "Per-allele effect estimate of the SNP on the exposure.".to_string(),
            )]),
        ),
        Field::new(IN_BETA_OUT, DataType::Float64, true).with_metadata(
            std::collections::HashMap::from([(
                "doc".to_string(),
                "Per-allele effect estimate of the SNP on the outcome.".to_string(),
            )]),
        ),
        Field::new(IN_SE_EXP, DataType::Float64, true).with_metadata(
            std::collections::HashMap::from([(
                "doc".to_string(),
                "Standard error of the exposure effect estimate.".to_string(),
            )]),
        ),
        Field::new(IN_SE_OUT, DataType::Float64, true).with_metadata(
            std::collections::HashMap::from([(
                "doc".to_string(),
                "Standard error of the outcome effect estimate.".to_string(),
            )]),
        ),
        Field::new(IN_EA_EXP, DataType::Utf8, true).with_metadata(std::collections::HashMap::from(
            [(
                "doc".to_string(),
                "Effect allele of the SNP in the exposure GWAS.".to_string(),
            )],
        )),
        Field::new(IN_OA_EXP, DataType::Utf8, true).with_metadata(std::collections::HashMap::from(
            [(
                "doc".to_string(),
                "Non-effect allele of the SNP in the exposure GWAS.".to_string(),
            )],
        )),
        Field::new(IN_EA_OUT, DataType::Utf8, true).with_metadata(std::collections::HashMap::from(
            [(
                "doc".to_string(),
                "Effect allele of the SNP in the outcome GWAS.".to_string(),
            )],
        )),
        Field::new(IN_OA_OUT, DataType::Utf8, true).with_metadata(std::collections::HashMap::from(
            [(
                "doc".to_string(),
                "Non-effect allele of the SNP in the outcome GWAS.".to_string(),
            )],
        )),
        Field::new(IN_EAF_EXP, DataType::Float64, true).with_metadata(
            std::collections::HashMap::from([(
                "doc".to_string(),
                "Effect-allele frequency of the SNP in the exposure sample.".to_string(),
            )]),
        ),
        Field::new(IN_EAF_OUT, DataType::Float64, true).with_metadata(
            std::collections::HashMap::from([(
                "doc".to_string(),
                "Effect-allele frequency of the SNP in the outcome sample.".to_string(),
            )]),
        ),
    ]))
}

/// The fixed output schema: one row per `(id_exposure, id_outcome, method)`.
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id_exposure", DataType::Utf8, false),
        Field::new("id_outcome", DataType::Utf8, false),
        Field::new("method", DataType::Utf8, false),
        Field::new("nsnp", DataType::Int64, false),
        Field::new("b", DataType::Float64, false),
        Field::new("se", DataType::Float64, false),
        Field::new("pval", DataType::Float64, false),
    ]))
}

// =====================================================================
// Column extraction — Arrow array → Rust
// =====================================================================

fn column_index(batches: &[RecordBatch], name: &str) -> Result<usize, TwoSampleMrNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or(TwoSampleMrNodeError::EmptyInput)?;
    schema
        .index_of(name)
        .map_err(|_| TwoSampleMrNodeError::MissingColumn {
            name: name.to_string(),
        })
}

/// Extract a required (non-null) Utf8 column into `Vec<String>`. Errors on null
/// cells — `snp` / `id_exposure` / `id_outcome` are mandatory join/group keys.
fn extract_required_string(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<String>, TwoSampleMrNodeError> {
    let idx = column_index(batches, name)?;
    // Validate the type once on the first batch (all batches share the schema).
    let dtype = batches[0].schema().field(idx).data_type().clone();
    if !matches!(
        dtype,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
    ) {
        return Err(TwoSampleMrNodeError::WrongColumnType {
            name: name.to_string(),
            dtype: dtype.to_string(),
        });
    }

    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        let opt_iter: Box<dyn Iterator<Item = Option<&str>>> = match dtype {
            DataType::Utf8 => {
                let a = col.as_any().downcast_ref::<StringArray>().unwrap();
                Box::new(a.iter())
            }
            DataType::Utf8View => {
                let a = col
                    .as_any()
                    .downcast_ref::<arrow_array::StringViewArray>()
                    .unwrap();
                Box::new(a.iter())
            }
            _ => Box::new(
                col.as_any()
                    .downcast_ref::<arrow_array::LargeStringArray>()
                    .unwrap()
                    .iter(),
            ),
        };
        for v in opt_iter {
            out.push(
                v.map(str::to_string)
                    .ok_or(TwoSampleMrNodeError::WrongColumnType {
                        name: name.to_string(),
                        dtype: "null".to_string(),
                    })?,
            );
        }
    }
    Ok(out)
}

/// Extract a nullable Utf8 column into `Vec<Option<String>>` (R's `NA` alleles).
fn extract_opt_string(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<Option<String>>, TwoSampleMrNodeError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();
    if !matches!(
        dtype,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
    ) {
        return Err(TwoSampleMrNodeError::WrongColumnType {
            name: name.to_string(),
            dtype: dtype.to_string(),
        });
    }

    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        match dtype {
            DataType::Utf8 => {
                for v in col.as_any().downcast_ref::<StringArray>().unwrap().iter() {
                    out.push(v.map(str::to_string));
                }
            }
            DataType::Utf8View => {
                for v in col
                    .as_any()
                    .downcast_ref::<arrow_array::StringViewArray>()
                    .unwrap()
                    .iter()
                {
                    out.push(v.map(str::to_string));
                }
            }
            _ => {
                for v in col
                    .as_any()
                    .downcast_ref::<arrow_array::LargeStringArray>()
                    .unwrap()
                    .iter()
                {
                    out.push(v.map(str::to_string));
                }
            }
        }
    }
    Ok(out)
}

/// Extract a numeric column into `Vec<f64>`, casting integer/float types and
/// replacing nulls with `NaN` (the `mr` crate's NA convention). Adapted from
/// `linear_regression::extract_numeric_column`.
fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, TwoSampleMrNodeError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();
    let is_numeric = matches!(
        dtype,
        DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    );
    if !is_numeric {
        return Err(TwoSampleMrNodeError::WrongColumnType {
            name: name.to_string(),
            dtype: dtype.to_string(),
        });
    }

    let mut values = Vec::new();
    for batch in batches {
        push_numeric(batch.column(idx), &mut values);
    }
    Ok(values)
}

/// Extract a numeric column into `Vec<Option<f64>>` (null → `None`). Used for
/// the optional effect-allele-frequency columns.
fn extract_opt_f64(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<Option<f64>>, TwoSampleMrNodeError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();
    let is_numeric = matches!(
        dtype,
        DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    );
    if !is_numeric {
        return Err(TwoSampleMrNodeError::WrongColumnType {
            name: name.to_string(),
            dtype: dtype.to_string(),
        });
    }

    let mut values = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        macro_rules! cast_opt {
            ($arr:expr, $T:ty) => {
                if let Some(a) = $arr.as_any().downcast_ref::<$T>() {
                    for v in a.iter() {
                        values.push(v.map(|x| x as f64));
                    }
                    continue;
                }
            };
        }
        cast_opt!(col, Int8Array);
        cast_opt!(col, Int16Array);
        cast_opt!(col, Int32Array);
        cast_opt!(col, Int64Array);
        cast_opt!(col, UInt8Array);
        cast_opt!(col, UInt16Array);
        cast_opt!(col, UInt32Array);
        cast_opt!(col, UInt64Array);
        cast_opt!(col, Float32Array);
        if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
            for v in a.iter() {
                values.push(v);
            }
            continue;
        }
    }
    Ok(values)
}

/// Push numeric values from a single column array into `out`, converting nulls
/// to NaN. Dispatches on the array type.
fn push_numeric(col: &dyn Array, out: &mut Vec<f64>) {
    macro_rules! cast {
        ($arr:expr, $T:ty) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$T>() {
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
    // Non-dispatched slots (e.g. Float16) fall back to NaN defensively,
    // regardless of null state — the null/nonnull distinction is discarded
    // for unsupported physical types.
    for _ in 0..col.len() {
        out.push(f64::NAN);
    }
}

// =====================================================================
// Config
// =====================================================================

/// Mirror of [`mr::Parameters`] that derives the serde/JSON-Schema derives the
/// registry needs (`mr::Parameters` itself does not). Field defaults reproduce
/// [`mr::Parameters::default_for`] exactly.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TwoSampleMrParameters {
    /// `"z"` or `"t"` — test distribution for some methods.
    pub test_dist: String,
    /// Number of bootstrap replications for SE estimation.
    pub nboot: usize,
    /// Outcome–exposure beta covariance for the delta-method SE.
    pub cov: f64,
    /// Penalisation constant (`penk`) for penalised weighted median / mode.
    pub penk: f64,
    /// Bandwidth multiplier (`phi`) for the mode estimator.
    pub phi: f64,
    /// Two-sided significance level for confidence intervals.
    pub alpha: f64,
    /// Q-statistic threshold for the Rucker framework.
    pub qthresh: f64,
    /// Whether the model accounts for overdispersion.
    pub over_dispersion: bool,
    /// Whether empirical partially-Bayes shrinkage is applied.
    pub shrinkage: bool,
}

impl Default for TwoSampleMrParameters {
    fn default() -> Self {
        let p = mr::Parameters::default_for();
        Self {
            test_dist: p.test_dist,
            nboot: p.nboot,
            cov: p.cov,
            penk: p.penk,
            phi: p.phi,
            alpha: p.alpha,
            qthresh: p.qthresh,
            over_dispersion: p.over_dispersion,
            shrinkage: p.shrinkage,
        }
    }
}

impl From<TwoSampleMrParameters> for mr::Parameters {
    fn from(p: TwoSampleMrParameters) -> Self {
        mr::Parameters {
            test_dist: p.test_dist,
            nboot: p.nboot,
            cov: p.cov,
            penk: p.penk,
            phi: p.phi,
            alpha: p.alpha,
            qthresh: p.qthresh,
            over_dispersion: p.over_dispersion,
            shrinkage: p.shrinkage,
        }
    }
}

fn default_tolerance() -> f64 {
    0.08
}

fn default_clump_r2() -> f64 {
    0.001
}
fn default_clump_kb() -> i32 {
    5000
}
fn default_clump_p1() -> f64 {
    5e-8
}
fn default_clump_pop() -> String {
    "EUR".to_string()
}

/// LD clumping parameters for selecting independent instruments via the
/// OpenGWAS `/ld/clump` endpoint (1000 Genomes reference panel).
///
/// When attached to [`TwoSampleMrNodeSpec::clump`], exposure p-values are
/// computed from `beta_exposure / se_exposure`, sent to OpenGWAS for
/// clumping, and only the returned index SNPs are retained for MR.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ClumpConfig {
    /// LD r² threshold. SNPs in LD (r² ≥ this) with a more significant index
    /// SNP are removed. Default `0.001`.
    #[serde(default = "default_clump_r2")]
    pub r2: f64,
    /// Clumping window in kilobases around the index SNP. Default `5000`.
    #[serde(default = "default_clump_kb")]
    pub kb: i32,
    /// P-value threshold for index (top) SNPs — only SNPs with exposure
    /// p ≤ this are eligible as clump representatives. Default `5e-8`.
    #[serde(default = "default_clump_p1")]
    pub p1: f64,
    /// 1000 Genomes reference population: `"EUR"`, `"SAS"`, `"EAS"`, `"AFR"`,
    /// or `"AMR"`. Default `"EUR"`.
    #[serde(default = "default_clump_pop")]
    pub pop: String,
}

impl Default for ClumpConfig {
    fn default() -> Self {
        Self {
            r2: default_clump_r2(),
            kb: default_clump_kb(),
            p1: default_clump_p1(),
            pop: default_clump_pop(),
        }
    }
}

/// Spec for the MR node, deserialised from the registry-provided JSON.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TwoSampleMrNodeSpec {
    /// Exposure trait identifier (e.g. `"ieu-a-2"`). Passed through to every
    /// output row as `id_exposure`; not expected as a column in the input
    /// DataFrame (avoids duplicating one string across all SNP rows).
    pub id_exposure: String,
    /// Outcome trait identifier (e.g. `"ieu-a-7"`).
    pub id_outcome: String,
    /// Methods to run, as `mr_method_list()` `obj` names (e.g. `"mr_ivw"`).
    /// Empty (the default) selects the `use_by_default` method set.
    #[serde(default)]
    pub method_list: Vec<String>,
    /// Harmonisation strictness. Defaults to [`HarmoniseAction::InferStrand`].
    /// See [`mr::harmonise::HarmoniseAction`] for variant semantics.
    #[serde(default)]
    pub action: mr::harmonise::HarmoniseAction,
    /// Allele-frequency tolerance for palindrome inference (default `0.08`).
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// LD clumping configuration. Instruments are always clumped via the
    /// OpenGWAS `/ld/clump` endpoint before harmonisation to ensure
    /// independence. Defaults to standard TwoSampleMR parameters
    /// (r²=0.001, kb=5000, p1=5e-8, pop=EUR). Requires the
    /// `OPENGWAS_TOKEN` environment variable.
    #[serde(default)]
    pub clump: ClumpConfig,
    /// Full MR [`TwoSampleMrParameters`] (defaults reproduce `default_parameters()`).
    #[serde(default)]
    pub parameters: TwoSampleMrParameters,
}

// =====================================================================
// LD clumping helper
// =====================================================================

/// Compute a two-sided z-test p-value from beta and SE.
fn pval_from_beta_se(beta: f64, se: f64) -> f64 {
    if !beta.is_finite() || !se.is_finite() || se <= 0.0 {
        return 1.0; // non-finite → won't be selected as index SNP
    }
    let z = (beta / se).abs();
    let normal = statrs::distribution::Normal::new(0.0, 1.0).unwrap();
    2.0 * statrs::distribution::ContinuousCDF::cdf(&normal, -z)
}

/// Extract the set of clumped index-SNP rsIDs from the OpenGWAS `/ld/clump`
/// JSON response (an array of objects, each with a `"rsid"` string field).
fn parse_clumped_rsids(resp: &serde_json::Value) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if let Some(arr) = resp.as_array() {
        for row in arr {
            if let Some(rsid) = row.get("rsid").and_then(|v| v.as_str()) {
                out.insert(rsid.to_string());
            }
        }
    }
    out
}

/// Filter `inputs` to only the independent index SNPs returned by OpenGWAS
/// `/ld/clump`. Exposure p-values are derived from `beta_exposure /
/// se_exposure`.
async fn clump_instruments(
    inputs: Vec<mr::harmonise::HarmoniseInput>,
    cfg: &ClumpConfig,
) -> Result<Vec<mr::harmonise::HarmoniseInput>, TwoSampleMrNodeError> {
    if inputs.is_empty() {
        return Ok(inputs);
    }

    let rsids: Vec<String> = inputs.iter().map(|r| r.snp.clone()).collect();
    let pvals: Vec<f64> = inputs
        .iter()
        .map(|r| pval_from_beta_se(r.beta_exposure, r.se_exposure))
        .collect();

    let client = opengwas::OpengwasClient::new(None).map_err(|e| {
        TwoSampleMrNodeError::Clump(format!(
            "failed to create OpenGWAS client (is OPENGWAS_TOKEN set?): {e}"
        ))
    })?;

    let req = opengwas::types::LdClumpRequest {
        rsid: rsids.clone(),
        pval: pvals,
        pthresh: Some(cfg.p1),
        r2: Some(cfg.r2),
        kb: Some(cfg.kb),
        pop: Some(cfg.pop.clone()),
    };

    let resp = client.ld_clump(&req).await.map_err(|e| {
        TwoSampleMrNodeError::Clump(format!("OpenGWAS /ld/clump request failed: {e}"))
    })?;

    let kept = parse_clumped_rsids(&resp);
    tracing::info!(
        "LD clumping: {} of {} SNPs retained as independent instruments (r²={}, kb={}, pop={})",
        kept.len(),
        inputs.len(),
        cfg.r2,
        cfg.kb,
        cfg.pop,
    );

    if kept.is_empty() {
        return Err(TwoSampleMrNodeError::Clump(
            "clumping removed all instruments; check the p-value threshold or input data".into(),
        ));
    }

    let filtered: Vec<_> = inputs
        .into_iter()
        .filter(|r| kept.contains(&r.snp))
        .collect();
    Ok(filtered)
}

// =====================================================================
// Node
// =====================================================================

const TWO_SAMPLE_MR_NODE_KIND: &str = "two_sample_mr";

/// A transform node that runs allele harmonisation + the main MR dispatch.
///
/// The upstream `DataFrame` must be merged on SNP and carry the fixed input
/// columns (see [`input_schema`]).
#[derive(Clone)]
pub struct TwoSampleMrNode {
    meta: NodePorts,
    spec: TwoSampleMrNodeSpec,
}

impl TwoSampleMrNode {
    /// Construct an [`TwoSampleMrNode`] from a fully-specified [`TwoSampleMrNodeSpec`].
    pub fn new(spec: TwoSampleMrNodeSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
        }
    }
}

pub struct TwoSampleMrNodeFactory {}

/// Static port layout for every [`TwoSampleMrNode`]: one typed input (harmonised GWAS
/// sumstats, see [`input_schema`]) and one typed output ([`output_schema`]).
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl NodeFactory for TwoSampleMrNodeFactory {
    fn kind(&self) -> &'static str {
        TWO_SAMPLE_MR_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Mendelian Randomisation: harmonises alleles and dispatches causal inference methods."
    }

    fn doc(&self) -> &'static str {
        "Mendelian Randomisation (MR) transform node. Takes a single upstream \
        DataFrame of SNP-merged exposure-outcome summary statistics, performs \
        allele harmonisation, and dispatches user-selected MR methods (IVW, \
        weighted median, MR-Egger, etc.). Outputs one row per \
        (exposure, outcome, method) with estimate, SE, p-value, and SNP count."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TwoSampleMrNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let spec: TwoSampleMrNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TwoSampleMrNode::new(spec)))
    }
}

#[async_trait]
impl DagNode for TwoSampleMrNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        TWO_SAMPLE_MR_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(TwoSampleMrNodeError::EmptyInput)?;

        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: TWO_SAMPLE_MR_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            return Err(TwoSampleMrNodeError::EmptyInput.into());
        }

        // ---- extract columns ----
        let snp = extract_required_string(&batches, IN_SNP)?;
        let beta_exp = extract_f64(&batches, IN_BETA_EXP)?;
        let beta_out = extract_f64(&batches, IN_BETA_OUT)?;
        let se_exp = extract_f64(&batches, IN_SE_EXP)?;
        let se_out = extract_f64(&batches, IN_SE_OUT)?;
        let ea_exp = extract_opt_string(&batches, IN_EA_EXP)?;
        let oa_exp = extract_opt_string(&batches, IN_OA_EXP)?;
        let ea_out = extract_opt_string(&batches, IN_EA_OUT)?;
        let oa_out = extract_opt_string(&batches, IN_OA_OUT)?;
        let eaf_exp = extract_opt_f64(&batches, IN_EAF_EXP)?;
        let eaf_out = extract_opt_f64(&batches, IN_EAF_OUT)?;

        let n = snp.len();
        for (name, len) in [
            (IN_BETA_EXP, beta_exp.len()),
            (IN_BETA_OUT, beta_out.len()),
            (IN_SE_EXP, se_exp.len()),
            (IN_SE_OUT, se_out.len()),
            (IN_EA_EXP, ea_exp.len()),
            (IN_OA_EXP, oa_exp.len()),
            (IN_EA_OUT, ea_out.len()),
            (IN_OA_OUT, oa_out.len()),
            (IN_EAF_EXP, eaf_exp.len()),
            (IN_EAF_OUT, eaf_out.len()),
        ] {
            if len != n {
                return Err(TwoSampleMrNodeError::LengthMismatch {
                    name: name.into(),
                    len,
                    expected: n,
                }
                .into());
            }
        }

        // ---- build HarmoniseInput rows ----
        let mut hinputs = Vec::with_capacity(n);
        for i in 0..n {
            hinputs.push(mr::harmonise::HarmoniseInput {
                snp: snp[i].clone(),
                id_exposure: self.spec.id_exposure.clone(),
                id_outcome: self.spec.id_outcome.clone(),
                beta_exposure: beta_exp[i],
                beta_outcome: beta_out[i],
                se_exposure: se_exp[i],
                se_outcome: se_out[i],
                effect_allele_exposure: ea_exp[i].clone(),
                other_allele_exposure: oa_exp[i].clone(),
                effect_allele_outcome: ea_out[i].clone(),
                other_allele_outcome: oa_out[i].clone(),
                eaf_exposure: eaf_exp[i],
                eaf_outcome: eaf_out[i],
            });
        }

        // ---- LD clumping ----
        let hinputs = clump_instruments(hinputs, &self.spec.clump).await?;

        // ---- harmonise ----
        let harmonised =
            mr::harmonise::harmonise_data_with(&hinputs, self.spec.action, self.spec.tolerance);

        // ---- dispatch mr() ----
        let parameters: mr::Parameters = self.spec.parameters.clone().into();
        let method_refs: Vec<&str> = self.spec.method_list.iter().map(|s| s.as_str()).collect();
        let rows = mr::dispatch::mr(&harmonised, &parameters, &method_refs)
            .map_err(TwoSampleMrNodeError::Mr)?;

        // ---- build output batch ----
        let batch = build_result_batch(&rows)?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(TwoSampleMrNodeError::from)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build the output `RecordBatch` (one row per [`mr::dispatch::MrResultRow`]).
fn build_result_batch(
    rows: &[mr::dispatch::MrResultRow],
) -> Result<RecordBatch, TwoSampleMrNodeError> {
    let id_exp: Vec<&str> = rows.iter().map(|r| r.id_exposure.as_str()).collect();
    let id_out: Vec<&str> = rows.iter().map(|r| r.id_outcome.as_str()).collect();
    let method: Vec<&str> = rows.iter().map(|r| r.method.as_str()).collect();
    let nsnp: Vec<i64> = rows.iter().map(|r| r.nsnp as i64).collect();
    let b: Vec<f64> = rows.iter().map(|r| r.b).collect();
    let se: Vec<f64> = rows.iter().map(|r| r.se).collect();
    let pval: Vec<f64> = rows.iter().map(|r| r.pval).collect();

    let batch = RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(StringArray::from(id_exp)),
            Arc::new(StringArray::from(id_out)),
            Arc::new(StringArray::from(method)),
            Arc::new(Int64Array::from(nsnp)),
            Arc::new(Float64Array::from(b)),
            Arc::new(Float64Array::from(se)),
            Arc::new(Float64Array::from(pval)),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a small set of HarmoniseInput rows with matching alleles (so
    /// harmonise keeps all rows) across several instruments — enough for IVW /
    /// Egger / median / mode to fire. These are pre-clumped (independent).
    fn make_test_harmonise_inputs() -> Vec<mr::harmonise::HarmoniseInput> {
        let snp = ["rs1", "rs2", "rs3", "rs4"];
        let beta_exp = [0.10, 0.20, -0.15, 0.05];
        let beta_out = [0.045, 0.091, -0.060, 0.022];
        let se_exp = [0.01, 0.01, 0.01, 0.01];
        let se_out = [0.01, 0.01, 0.01, 0.01];
        let ea_exp = ["A", "G", "T", "C"];
        let oa_exp = ["G", "A", "C", "G"];
        let ea_out = ["A", "G", "T", "C"];
        let oa_out = ["G", "A", "C", "G"];
        let eaf_exp = [0.2, 0.3, 0.25, 0.4];
        let eaf_out = [0.2, 0.3, 0.25, 0.4];

        (0..4)
            .map(|i| mr::harmonise::HarmoniseInput {
                snp: snp[i].to_string(),
                id_exposure: "exp".to_string(),
                id_outcome: "out".to_string(),
                beta_exposure: beta_exp[i],
                beta_outcome: beta_out[i],
                se_exposure: se_exp[i],
                se_outcome: se_out[i],
                effect_allele_exposure: Some(ea_exp[i].to_string()),
                other_allele_exposure: Some(oa_exp[i].to_string()),
                effect_allele_outcome: Some(ea_out[i].to_string()),
                other_allele_outcome: Some(oa_out[i].to_string()),
                eaf_exposure: Some(eaf_exp[i]),
                eaf_outcome: Some(eaf_out[i]),
            })
            .collect()
    }

    #[test]
    fn runs_default_methods_and_emits_ivw() {
        // Test the harmonise + dispatch pipeline directly (post-clumping).
        // The clumping step is network-dependent and tested separately.
        let hinputs = make_test_harmonise_inputs();
        let harmonised = mr::harmonise::harmonise_data_with(
            &hinputs,
            mr::harmonise::HarmoniseAction::default(),
            default_tolerance(),
        );
        let params: mr::Parameters = TwoSampleMrParameters::default().into();
        let rows = mr::dispatch::mr(&harmonised, &params, &[]).unwrap();
        assert!(!rows.is_empty(), "expected at least one MR estimate row");

        let methods: Vec<String> = rows.iter().map(|r| r.method.clone()).collect();
        assert!(
            methods.iter().any(|m| m == "Inverse variance weighted"),
            "IVW estimate missing; got {methods:?}"
        );
    }

    #[test]
    fn respects_explicit_method_list() {
        let hinputs = make_test_harmonise_inputs();
        let harmonised = mr::harmonise::harmonise_data_with(
            &hinputs,
            mr::harmonise::HarmoniseAction::default(),
            default_tolerance(),
        );
        let params: mr::Parameters = TwoSampleMrParameters::default().into();
        let rows = mr::dispatch::mr(&harmonised, &params, &["mr_ivw"]).unwrap();
        let methods: Vec<String> = rows.iter().map(|r| r.method.clone()).collect();
        // With a single SNP-set of size >1 and an explicit one-method list,
        // only IVW should be emitted.
        assert_eq!(methods, vec!["Inverse variance weighted".to_string()]);
    }

    #[test]
    fn rejects_invalid_action_at_deserialisation() {
        // With HarmoniseAction as a serde enum, invalid values are rejected at
        // the JSON boundary — the type system makes them unconstructable in Rust.
        let json = serde_json::json!({
            "id_exposure": "exp",
            "id_outcome": "out",
            "action": "nonexistent_variant",
        });
        assert!(serde_json::from_value::<TwoSampleMrNodeSpec>(json).is_err());
    }

    #[test]
    fn clump_config_defaults() {
        let c = ClumpConfig::default();
        assert!((c.r2 - 0.001).abs() < f64::EPSILON);
        assert_eq!(c.kb, 5000);
        assert!((c.p1 - 5e-8).abs() < f64::EPSILON);
        assert_eq!(c.pop, "EUR");
    }

    #[test]
    fn clump_config_deserialises_from_json() {
        let json = serde_json::json!({
            "r2": 0.01,
            "kb": 1000,
            "p1": 1e-5,
            "pop": "EAS",
        });
        let c: ClumpConfig = serde_json::from_value(json).unwrap();
        assert!((c.r2 - 0.01).abs() < f64::EPSILON);
        assert_eq!(c.kb, 1000);
        assert!((c.p1 - 1e-5).abs() < f64::EPSILON);
        assert_eq!(c.pop, "EAS");
    }

    #[test]
    fn clump_config_uses_defaults_for_missing_fields() {
        let json = serde_json::json!({});
        let c: ClumpConfig = serde_json::from_value(json).unwrap();
        assert!((c.r2 - 0.001).abs() < f64::EPSILON);
        assert_eq!(c.kb, 5000);
        assert_eq!(c.pop, "EUR");
    }

    #[test]
    fn spec_with_clump_deserialises() {
        let json = serde_json::json!({
            "id_exposure": "exp",
            "id_outcome": "out",
            "clump": { "r2": 0.05, "pop": "SAS" },
        });
        let spec: TwoSampleMrNodeSpec = serde_json::from_value(json).unwrap();
        assert!((spec.clump.r2 - 0.05).abs() < f64::EPSILON);
        assert_eq!(spec.clump.pop, "SAS");
    }

    #[test]
    fn spec_without_clump_uses_defaults() {
        let json = serde_json::json!({
            "id_exposure": "exp",
            "id_outcome": "out",
        });
        let spec: TwoSampleMrNodeSpec = serde_json::from_value(json).unwrap();
        assert!((spec.clump.r2 - 0.001).abs() < f64::EPSILON);
        assert_eq!(spec.clump.kb, 5000);
        assert_eq!(spec.clump.pop, "EUR");
    }

    #[test]
    fn pval_from_beta_se_basic() {
        // z = 0.1/0.01 = 10 → p ≈ 1.5e-23
        let p = pval_from_beta_se(0.1, 0.01);
        assert!(p < 1e-20);

        // z = 0 → p = 1
        let p = pval_from_beta_se(0.0, 1.0);
        assert!((p - 1.0).abs() < 1e-10);

        // non-finite inputs → p = 1.0 (won't be selected as index SNP)
        assert!((pval_from_beta_se(f64::NAN, 0.01) - 1.0).abs() < f64::EPSILON);
        assert!((pval_from_beta_se(0.1, 0.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_clumped_rsids_extracts_rsid_field() {
        let resp = serde_json::json!([
            { "rsid": "rs1", "chr": "1", "position": 100 },
            { "rsid": "rs2", "chr": "2", "position": 200 },
        ]);
        let ids = parse_clumped_rsids(&resp);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("rs1"));
        assert!(ids.contains("rs2"));
    }

    #[test]
    fn parse_clumped_rsids_empty_response() {
        let resp = serde_json::json!([]);
        let ids = parse_clumped_rsids(&resp);
        assert!(ids.is_empty());
    }
}
