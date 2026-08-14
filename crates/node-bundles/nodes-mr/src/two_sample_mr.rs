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

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    codegen::context::{CodegenCtx, CodegenError, NodeCodegen},
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
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

impl ::dag_core::dag::NodeError for TwoSampleMrNodeError {
    fn node_type(&self) -> &str {
        TWO_SAMPLE_MR_NODE_KIND
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

/// LD clumping backend selection.
///
/// `Opengwas` (the default) sends SNPs to the remote OpenGWAS `/ld/clump`
/// endpoint. `IcebergLd` queries the Iceberg `ld_matrix.eur_chr{N}` pairwise
/// r² tables and performs greedy clumping entirely in Rust — no network
/// access required.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type")]
#[derive(Default)]
pub enum ClumpMode {
    /// Remote OpenGWAS `/ld/clump` endpoint (1000 Genomes reference panel).
    /// Requires the `OPENGWAS_TOKEN` environment variable.
    #[serde(rename = "opengwas")]
    #[default]
    Opengwas,
    /// Local Iceberg `ld_matrix.eur_chr{N}` pairwise r² tables.
    ///
    /// The node queries `iceberg.ld_matrix.eur_chr{chrom}` for r² pairs
    /// involving the instrument SNPs and runs greedy clumping in Rust.
    /// No network access or API token required.
    #[serde(rename = "iceberg_ld")]
    IcebergLd,
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
    /// or `"AMR"`. Default `"EUR"`. Only used in [`ClumpMode::Opengwas`].
    #[serde(default = "default_clump_pop")]
    pub pop: String,
    /// Clumping backend. Default: OpenGWAS remote API ([`ClumpMode::Opengwas`]).
    /// Use [`ClumpMode::IcebergLd`] to clump against the local Iceberg
    /// `ld_matrix.eur_chr{N}` pairwise r² tables — no network or API token
    /// required.
    #[serde(default)]
    pub mode: ClumpMode,
}

impl Default for ClumpConfig {
    fn default() -> Self {
        Self {
            r2: default_clump_r2(),
            kb: default_clump_kb(),
            p1: default_clump_p1(),
            pop: default_clump_pop(),
            mode: ClumpMode::default(),
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
/// JSON response. The endpoint may return either a flat array of rsID strings
/// (`["rs1", "rs2"]`) or an array of objects with a `"rsid"` field
/// (`[{"rsid": "rs1", "chr": "1", …}]`); both shapes are handled.
fn parse_clumped_rsids(resp: &serde_json::Value) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if let Some(arr) = resp.as_array() {
        for elem in arr {
            // Flat string: "rs12345"
            if let Some(s) = elem.as_str() {
                out.insert(s.to_string());
            }
            // Object: {"rsid": "rs12345", ...}
            else if let Some(rsid) = elem.get("rsid").and_then(|v| v.as_str()) {
                out.insert(rsid.to_string());
            }
        }
    }
    out
}

/// Filter `inputs` to only the independent index SNPs. Dispatches to the
/// OpenGWAS remote endpoint or the Iceberg LD matrix based on
/// [`ClumpConfig::mode`].
async fn clump_instruments(
    inputs: Vec<mr::harmonise::HarmoniseInput>,
    cfg: &ClumpConfig,
    session: &datafusion::prelude::SessionContext,
    ld_base: Option<&str>,
) -> Result<Vec<mr::harmonise::HarmoniseInput>, TwoSampleMrNodeError> {
    if inputs.is_empty() {
        return Ok(inputs);
    }
    match &cfg.mode {
        ClumpMode::Opengwas => clump_opengwas(inputs, cfg).await,
        ClumpMode::IcebergLd => clump_iceberg_ld(inputs, cfg, session, ld_base).await,
    }
}

/// OpenGWAS `/ld/clump` backend. Exposure p-values are derived from
/// `beta_exposure / se_exposure`, sent to the endpoint, and only the returned
/// index SNPs are retained.
async fn clump_opengwas(
    inputs: Vec<mr::harmonise::HarmoniseInput>,
    cfg: &ClumpConfig,
) -> Result<Vec<mr::harmonise::HarmoniseInput>, TwoSampleMrNodeError> {
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
        "LD clumping (OpenGWAS): {} of {} SNPs retained as independent instruments (r²={}, kb={}, pop={})",
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

/// Iceberg LD matrix backend. Queries `iceberg.ld_matrix.eur_chr{N}` for
/// pairwise r² values between instrument SNPs and runs greedy clumping in
/// Rust. No network access or API token required.
async fn clump_iceberg_ld(
    inputs: Vec<mr::harmonise::HarmoniseInput>,
    cfg: &ClumpConfig,
    session: &datafusion::prelude::SessionContext,
    ld_base: Option<&str>,
) -> Result<Vec<mr::harmonise::HarmoniseInput>, TwoSampleMrNodeError> {
    use std::collections::{HashMap, HashSet};

    // Build the set of instrument rsIDs for filtering.
    let snp_set: HashSet<String> = inputs.iter().map(|r| r.snp.clone()).collect();

    // Build a quoted SNP-list SQL fragment so the query only returns rows
    // involving our instruments, rather than scanning the entire LD table.
    let snp_list = snp_set
        .iter()
        .map(|s| format!("'{}'", s.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");

    // Query the Iceberg ld_matrix tables for all r² pairs involving our SNPs.
    // We query chromosomes 1-22 (standard autosomes).
    let mut r2_map: HashMap<(String, String), f64> = HashMap::new();
    let mut skipped_chroms: Vec<u32> = Vec::new();
    for chrom in 1..=22 {
        // Resolve per-chromosome LD-matrix table SQL from the catalog, or
        // fall back to the hardcoded `iceberg.ld_matrix.eur_chr{N}`.
        let table_sql = match ld_base {
            Some(base) => format!("{base}{chrom}"),
            None => {
                let table_name = format!("ld_matrix_eur_chr{chrom}");
                nodes_ldsc::ldsc_common::register_listing_table(
                    session,
                    &table_name,
                    &format!("vfs:///data/oss/ld_matrix/eur_chr{chrom}/"),
                )
                .await
                .map_err(|e| TwoSampleMrNodeError::Clump(format!("register LD matrix: {e}")))?;
                table_name
            }
        };
        let sql = format!(
            "SELECT id_a, id_b, unphased_r2 \
             FROM {table_sql} \
             WHERE unphased_r2 >= {} \
               AND (id_a IN ({snp_list}) OR id_b IN ({snp_list}))",
            cfg.r2
        );
        let df = match session.sql(&sql).await {
            Ok(df) => df,
            Err(e) => {
                tracing::debug!("LD matrix table for chr{chrom} unavailable ({e}); skipping");
                skipped_chroms.push(chrom);
                continue;
            }
        };
        let batches = df.collect().await.map_err(|e| {
            TwoSampleMrNodeError::Clump(format!("LD matrix query failed (chr{chrom}): {e}"))
        })?;

        for batch in &batches {
            let col_a = batch.column_by_name("id_a");
            let col_b = batch.column_by_name("id_b");
            let col_r2 = batch.column_by_name("unphased_r2");
            let (col_a, col_b, col_r2) = match (col_a, col_b, col_r2) {
                (Some(a), Some(b), Some(r)) => (a, b, r),
                _ => continue,
            };
            let a_vals = dag_core::node::string_opt_values(col_a.as_ref());
            let b_vals = dag_core::node::string_opt_values(col_b.as_ref());
            if let (Some(a_vals), Some(b_vals)) = (a_vals, b_vals) {
                for i in 0..batch.num_rows() {
                    if col_r2.is_null(i) {
                        continue;
                    }
                    let r2 = if let Some(a) = col_r2.as_any().downcast_ref::<Float64Array>() {
                        a.value(i)
                    } else {
                        continue;
                    };
                    let a = match a_vals.get(i).and_then(|v| v.as_ref()) {
                        Some(v) => v.to_string(),
                        None => continue,
                    };
                    let b = match b_vals.get(i).and_then(|v| v.as_ref()) {
                        Some(v) => v.to_string(),
                        None => continue,
                    };
                    // Only keep pairs where at least one endpoint is in our SNP set.
                    if snp_set.contains(&a) || snp_set.contains(&b) {
                        // Store both directions for easy lookup.
                        r2_map.insert((a.clone(), b.clone()), r2);
                        r2_map.insert((b, a), r2);
                    }
                }
            }
        }
    }

    tracing::info!(
        "LD matrix: loaded {} r² pairs (≥ {}) for {} instrument SNPs{}",
        r2_map.len(),
        cfg.r2,
        snp_set.len(),
        if skipped_chroms.is_empty() {
            String::new()
        } else {
            format!("; skipped chromosomes (no table): {:?}", skipped_chroms)
        },
    );

    if !skipped_chroms.is_empty() {
        tracing::warn!(
            "LD matrix: chromosomes {:?} had no ld_matrix table; \
             SNPs on those chromosomes are treated as having no LD data \
             (kept as independent instruments)",
            skipped_chroms
        );
    }

    // Build ClumpSnp list. Since the Iceberg ld_matrix doesn't provide
    // chromosome/position info, we pass placeholder values — the greedy
    // clumping uses r² from the pre-computed table directly.
    let clump_snps: Vec<mr::clump::ClumpSnp> = inputs
        .iter()
        .map(|r| mr::clump::ClumpSnp {
            rsid: r.snp.clone(),
            pval: pval_from_beta_se(r.beta_exposure, r.se_exposure),
            chr: 0,
            bp: 0,
        })
        .collect();

    // r² closure: look up from the pre-computed HashMap. For SNPs not in the
    // table (no LD data), return 0.0 (treat as independent).
    let r2_fn = |i: usize, j: usize| -> f64 {
        let key = (clump_snps[i].rsid.clone(), clump_snps[j].rsid.clone());
        *r2_map.get(&key).unwrap_or(&0.0)
    };

    // Run greedy clumping. Since all positions are 0 (the ld_matrix table
    // has no bp column), the kb window distance is always 0 ≤ cfg.kb — i.e.
    // the window constraint is effectively disabled and pruning is driven
    // solely by the pre-computed r² values, which already encode proximity.
    let kept_rsids = mr::clump::greedy_clump(&clump_snps, cfg.r2, cfg.kb, r2_fn);

    tracing::info!(
        "LD clumping (Iceberg ld_matrix): {} of {} SNPs retained as independent instruments (r²={})",
        kept_rsids.len(),
        inputs.len(),
        cfg.r2,
    );

    if kept_rsids.is_empty() {
        return Err(TwoSampleMrNodeError::Clump(
            "clumping removed all instruments; check the p-value threshold or input data".into(),
        ));
    }

    let kept_set: HashSet<&str> = kept_rsids.iter().map(|s| s.as_str()).collect();
    let filtered: Vec<_> = inputs
        .into_iter()
        .filter(|r| kept_set.contains(r.snp.as_str()))
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
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: TwoSampleMrNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TwoSampleMrNode::new(spec)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let spec: TwoSampleMrNodeSpec =
            serde_json::from_value(spec.clone()).map_err(|e| CodegenError::BadSpec {
                kind: "two_sample_mr".into(),
                source: e,
            })?;

        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        // Format upstream merged data as TwoSampleMR exposure + outcome.
        let exp_dat = ctx.fresh_var("exposure_dat");
        let out_dat = ctx.fresh_var("outcome_dat");
        let harm = ctx.fresh_var("harmonised");

        let mut code = vec![
            format!("# Format upstream data as TwoSampleMR exposure data"),
            format!("{exp_dat} <- data.frame("),
            format!("  SNP = {input}$snp,"),
            format!("  beta.exposure = {input}$beta_exposure,"),
            format!("  se.exposure = {input}$se_exposure,"),
            format!("  effect_allele.exposure = {input}$effect_allele_exposure,"),
            format!("  other_allele.exposure = {input}$other_allele_exposure,"),
            format!("  eaf.exposure = {input}$eaf_exposure,"),
            format!("  id.exposure = \"{}\",", spec.id_exposure),
            format!("  exposure = \"{}\"", spec.id_exposure),
            format!(")"),
            String::new(),
            format!("# Format upstream data as TwoSampleMR outcome data"),
            format!("{out_dat} <- data.frame("),
            format!("  SNP = {input}$snp,"),
            format!("  beta.outcome = {input}$beta_outcome,"),
            format!("  se.outcome = {input}$se_outcome,"),
            format!("  effect_allele.outcome = {input}$effect_allele_outcome,"),
            format!("  other_allele.outcome = {input}$other_allele_outcome,"),
            format!("  eaf.outcome = {input}$eaf_outcome,"),
            format!("  id.outcome = \"{}\",", spec.id_outcome),
            format!("  outcome = \"{}\"", spec.id_outcome),
            format!(")"),
        ];

        // LD clumping — branch on the configured backend.
        code.push(String::new());
        match &spec.clump.mode {
            ClumpMode::Opengwas => {
                code.push("# LD clumping via OpenGWAS (requires API token)".to_string());
                code.push(format!(
                    "{exp_dat} <- clump_data({exp_dat}, clump_r2 = {}, clump_kb = {}, clump_p1 = {}, pop = \"{}\")",
                    spec.clump.r2, spec.clump.kb, spec.clump.p1, spec.clump.pop,
                ));
            }
            ClumpMode::IcebergLd => {
                // Iceberg LD clumping: the Rust runtime queries the
                // iceberg.ld_matrix.eur_chr{N} tables and performs greedy
                // clumping. In the R codegen (used for cross-validation),
                // we emit a note since R's clump_data() only supports
                // OpenGWAS. The exposure data entering this point has
                // already been clumped by the Rust node.
                code.push("# LD clumping performed via Iceberg ld_matrix table".to_string());
                code.push("# (Rust runtime uses greedy clumping on pre-computed r²;".to_string());
                code.push(format!(
                    "#  {exp_dat} is already clumped to independent instruments)"
                ));
            }
        }

        // Harmonise
        code.push(String::new());
        code.push("# Allele harmonisation".to_string());
        code.push(format!(
            "{harm} <- harmonise_data({exp_dat}, {out_dat}, action = {})",
            match spec.action {
                mr::harmonise::HarmoniseAction::ForwardStrand => 1,
                mr::harmonise::HarmoniseAction::InferStrand => 2,
                mr::harmonise::HarmoniseAction::ExcludePalindromic => 3,
            }
        ));

        // MR dispatch
        let methods = if spec.method_list.is_empty() {
            "c(\"mr_ivw\", \"mr_egger_regression\", \"mr_weighted_median\", \"mr_simple_mode\", \"mr_weighted_mode\")".to_string()
        } else {
            format!(
                "c({})",
                spec.method_list
                    .iter()
                    .map(|m| format!("\"{m}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        code.push(String::new());
        code.push("# Mendelian randomisation".to_string());
        code.push(format!("{out} <- mr({harm}, method_list = {methods})"));
        code.push(format!("print({out})"));

        Ok(NodeCodegen {
            code,
            output_vars: vec![out],
            extra_packages: vec![],
        })
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["TwoSampleMR".into()]
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
        node_ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
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
        let session = node_ctx.session();
        // Resolve the LD-matrix base table from the catalog (falls back to
        // hardcoded `iceberg.ld_matrix.eur_chr{N}` when not registered).
        let hinputs =
            clump_instruments(hinputs, &self.spec.clump, &session, Some("eur_chr")).await?;

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
        assert_eq!(c.mode, ClumpMode::Opengwas);
    }

    #[test]
    fn clump_mode_opengwas_default() {
        let json = serde_json::json!({});
        let c: ClumpConfig = serde_json::from_value(json).unwrap();
        assert_eq!(c.mode, ClumpMode::Opengwas);
    }

    #[test]
    fn clump_mode_iceberg_ld_deserialises() {
        let json = serde_json::json!({
            "mode": { "type": "iceberg_ld" },
        });
        let c: ClumpConfig = serde_json::from_value(json).unwrap();
        assert_eq!(c.mode, ClumpMode::IcebergLd);
    }

    #[test]
    fn clump_mode_opengwas_explicit() {
        let json = serde_json::json!({
            "mode": { "type": "opengwas" },
        });
        let c: ClumpConfig = serde_json::from_value(json).unwrap();
        assert_eq!(c.mode, ClumpMode::Opengwas);
    }

    #[test]
    fn spec_with_iceberg_ld_clump_deserialises() {
        let json = serde_json::json!({
            "id_exposure": "exp",
            "id_outcome": "out",
            "clump": {
                "r2": 0.01,
                "kb": 1000,
                "mode": { "type": "iceberg_ld" },
            },
        });
        let spec: TwoSampleMrNodeSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.clump.mode, ClumpMode::IcebergLd);
        assert!((spec.clump.r2 - 0.01).abs() < f64::EPSILON);
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
        // Object format: [{"rsid": "rs1", ...}, ...]
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
    fn parse_clumped_rsids_flat_string_array() {
        // Flat string format: ["rs1", "rs2"] (the actual OpenGWAS response shape)
        let resp = serde_json::json!(["rs1558902", "rs10938397"]);
        let ids = parse_clumped_rsids(&resp);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("rs1558902"));
        assert!(ids.contains("rs10938397"));
    }

    #[test]
    fn parse_clumped_rsids_empty_response() {
        let resp = serde_json::json!([]);
        let ids = parse_clumped_rsids(&resp);
        assert!(ids.is_empty());
    }

    // ---- End-to-end tests (require OPENGWAS_TOKEN + network) ----

    /// Build a test SessionContext (used by clump_instruments for IcebergLd mode).
    fn test_session() -> datafusion::prelude::SessionContext {
        datafusion::prelude::SessionContext::new()
    }

    /// Well-known BMI-associated SNPs (mostly in LD on different chromosomes)
    /// with approximate exposure p-values. Includes several pairs in known LD
    /// to verify that clumping removes the dependent ones.
    fn real_bmi_instruments() -> Vec<mr::harmonise::HarmoniseInput> {
        // (rsid, beta, se) — p-values derived from beta/se inside
        // clump_instruments. All p-values are well below the default p1=5e-8.
        let data: &[(&str, f64, f64)] = &[
            // FTO locus (chr 16) — multiple SNPs in LD, should clump to 1
            ("rs1558902", 0.090, 0.009),  // z=10.0, p ≈ 1e-23
            ("rs1421085", 0.080, 0.009),  // z=8.9,  p ≈ 5e-19, LD with rs1558902
            ("rs17817449", 0.080, 0.009), // z=8.9, p ≈ 5e-19, LD with rs1558902
            // GNPDA2 (chr 4) — independent locus
            ("rs10938397", 0.060, 0.008), // z=7.5, p ≈ 6e-14
            // MC4R (chr 18) — independent locus
            ("rs17782313", 0.055, 0.008), // z=6.9, p ≈ 5e-12
            // TMEM18 (chr 2) — independent locus
            ("rs6548238", 0.054, 0.008), // z=6.75, p ≈ 1e-11
        ];

        data.iter()
            .map(|(rsid, beta, se)| mr::harmonise::HarmoniseInput {
                snp: rsid.to_string(),
                id_exposure: "ieu-a-2".to_string(),
                id_outcome: "ieu-a-7".to_string(),
                beta_exposure: *beta,
                beta_outcome: 0.5 * beta,
                se_exposure: *se,
                se_outcome: *se,
                effect_allele_exposure: Some("A".to_string()),
                other_allele_exposure: Some("G".to_string()),
                effect_allele_outcome: Some("A".to_string()),
                other_allele_outcome: Some("G".to_string()),
                eaf_exposure: Some(0.4),
                eaf_outcome: Some(0.4),
            })
            .collect()
    }

    #[tokio::test]
    #[ignore = "requires OPENGWAS_TOKEN and network access"]
    async fn e2e_clump_reduces_fto_ld_block() {
        let inputs = real_bmi_instruments();
        let n_before = inputs.len();
        assert_eq!(n_before, 6, "expected 6 input SNPs");

        let cfg = ClumpConfig::default();
        let session = test_session();
        let clumped = clump_instruments(inputs, &cfg, &session, Some("eur_chr"))
            .await
            .expect("clumping should succeed");

        let n_after = clumped.len();
        println!("E2E clumping: {n_before} → {n_after} SNPs");
        for r in &clumped {
            println!("  kept: {}", r.snp);
        }

        // The three FTO-locus SNPs (rs1558902, rs1421085, rs17817449) are in
        // high LD → clumping should retain at most 1 of them.
        let fto_kept: Vec<_> = clumped
            .iter()
            .filter(|r| matches!(r.snp.as_str(), "rs1558902" | "rs1421085" | "rs17817449"))
            .collect();
        assert!(
            fto_kept.len() <= 1,
            "expected ≤1 FTO-locus SNP after clumping, got {}: {:?}",
            fto_kept.len(),
            fto_kept.iter().map(|r| &r.snp).collect::<Vec<_>>()
        );

        // Total should be strictly fewer than input (at least FTO LD removed).
        assert!(
            n_after < n_before,
            "expected clumping to remove some SNPs ({n_before} → {n_after})"
        );

        // Should retain at least the independent loci.
        assert!(
            n_after >= 3,
            "expected ≥3 independent loci after clumping, got {n_after}"
        );
    }

    #[tokio::test]
    #[ignore = "requires OPENGWAS_TOKEN and network access"]
    async fn e2e_clump_with_relaxed_r2_keeps_more() {
        let inputs = real_bmi_instruments();

        // Very relaxed r² → almost nothing gets pruned.
        let cfg = ClumpConfig {
            r2: 0.99,
            kb: 10_000,
            p1: 5e-8,
            pop: "EUR".to_string(),
            ..ClumpConfig::default()
        };
        let session = test_session();
        let clumped = clump_instruments(inputs, &cfg, &session, Some("eur_chr"))
            .await
            .expect("clumping should succeed");
        let relaxed_count = clumped.len();

        // Strict default r² → more aggressive pruning.
        let inputs2 = real_bmi_instruments();
        let strict = clump_instruments(inputs2, &ClumpConfig::default(), &session, Some("eur_chr"))
            .await
            .expect("clumping should succeed");
        let strict_count = strict.len();

        println!("E2E r² sweep: relaxed(0.99)={relaxed_count}, strict(0.001)={strict_count}");
        assert!(
            relaxed_count >= strict_count,
            "relaxed r² should retain ≥ SNPs than strict ({relaxed_count} < {strict_count})"
        );
    }

    #[tokio::test]
    #[ignore = "requires OPENGWAS_TOKEN and network access"]
    async fn e2e_clump_pop_filter_matters() {
        // Same SNPs, different populations — the set of retained index SNPs
        // may differ because LD patterns vary by ancestry.
        let session = test_session();
        let inputs_eur = real_bmi_instruments();
        let eur = clump_instruments(
            inputs_eur,
            &ClumpConfig {
                pop: "EUR".to_string(),
                ..ClumpConfig::default()
            },
            &session,
            None,
        )
        .await
        .expect("EUR clumping");

        let inputs_afr = real_bmi_instruments();
        let afr = clump_instruments(
            inputs_afr,
            &ClumpConfig {
                pop: "AFR".to_string(),
                ..ClumpConfig::default()
            },
            &session,
            None,
        )
        .await
        .expect("AFR clumping");

        let eur_snps: std::collections::HashSet<_> = eur.iter().map(|r| r.snp.clone()).collect();
        let afr_snps: std::collections::HashSet<_> = afr.iter().map(|r| r.snp.clone()).collect();

        println!("E2E pop: EUR kept {:?}, AFR kept {:?}", eur_snps, afr_snps);
        // Both should succeed and return non-empty results.
        assert!(!eur_snps.is_empty(), "EUR clumping returned no SNPs");
        assert!(!afr_snps.is_empty(), "AFR clumping returned no SNPs");
    }

    // ---- IcebergLd clumping tests (mock Iceberg ld_matrix tables) ----

    /// Register mock `iceberg.ld_matrix.eur_chr{N}` tables in the session with
    /// known r² pairs for testing. All other chromosomes are empty.
    async fn register_mock_ld_tables(session: &datafusion::prelude::SessionContext) {
        use datafusion::catalog::{
            CatalogProvider, MemTable, MemoryCatalogProvider, MemorySchemaProvider, SchemaProvider,
        };

        // r² pairs: rs1↔rs2 = 0.9 (high LD), rs3↔rs4 = 0.8 (high LD).
        // rs1 and rs3 are independent (no r² entry → treated as 0).
        let id_a = StringArray::from(vec!["rs1", "rs3"]);
        let id_b = StringArray::from(vec!["rs2", "rs4"]);
        let r2 = Float64Array::from(vec![0.9, 0.8]);

        let schema = Arc::new(Schema::new(vec![
            Field::new("id_a", DataType::Utf8, false),
            Field::new("id_b", DataType::Utf8, false),
            Field::new("unphased_r2", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(id_a), Arc::new(id_b), Arc::new(r2)],
        )
        .unwrap();

        for chrom in 1u32..=22 {
            let table_name = format!("eur_chr{chrom}");
            let table_data: Vec<Vec<RecordBatch>> = if chrom == 1 {
                vec![vec![batch.clone()]]
            } else {
                vec![vec![]]
            };
            let table = MemTable::try_new(schema.clone(), table_data).unwrap();
            session
                .register_table(table_name, std::sync::Arc::new(table))
                .unwrap();
        }
    }

    /// Build 4 test instruments: rs1 (most significant) through rs4.
    fn iceberg_test_inputs() -> Vec<mr::harmonise::HarmoniseInput> {
        let data: &[(&str, f64, f64)] = &[
            ("rs1", 0.10, 0.01), // z=10, p ≈ 1e-23 — most significant
            ("rs2", 0.08, 0.01), // z=8
            ("rs3", 0.06, 0.01), // z=6
            ("rs4", 0.05, 0.01), // z=5 — least significant
        ];
        data.iter()
            .map(|(rsid, beta, se)| mr::harmonise::HarmoniseInput {
                snp: rsid.to_string(),
                id_exposure: "test_exp".to_string(),
                id_outcome: "test_out".to_string(),
                beta_exposure: *beta,
                beta_outcome: 0.5 * beta,
                se_exposure: *se,
                se_outcome: *se,
                effect_allele_exposure: Some("A".to_string()),
                other_allele_exposure: Some("G".to_string()),
                effect_allele_outcome: Some("A".to_string()),
                other_allele_outcome: Some("G".to_string()),
                eaf_exposure: Some(0.4),
                eaf_outcome: Some(0.4),
            })
            .collect()
    }

    #[tokio::test]
    async fn iceberg_clump_prunes_high_ld() {
        // rs1↔rs2 r²=0.9, rs3↔rs4 r²=0.8. With r2_thresh=0.001:
        // rs1 (most significant) selected as index → prunes rs2.
        // rs3 selected as index → prunes rs4.
        // Result: {rs1, rs3}.
        let session = test_session();
        register_mock_ld_tables(&session).await;

        let inputs = iceberg_test_inputs();
        let cfg = ClumpConfig {
            r2: 0.001,
            kb: 5000,
            p1: 5e-8,
            mode: ClumpMode::IcebergLd,
            ..ClumpConfig::default()
        };
        let clumped = clump_instruments(inputs, &cfg, &session, Some("eur_chr"))
            .await
            .expect("iceberg clumping should succeed");

        let snps: std::collections::HashSet<&str> =
            clumped.iter().map(|r| r.snp.as_str()).collect();
        assert!(
            snps.contains("rs1"),
            "most significant SNP rs1 should be retained"
        );
        assert!(
            !snps.contains("rs2"),
            "rs2 (r²=0.9 with rs1) should be pruned"
        );
        assert!(
            snps.contains("rs3"),
            "rs3 should be retained (independent of rs1)"
        );
        assert!(
            !snps.contains("rs4"),
            "rs4 (r²=0.8 with rs3) should be pruned"
        );
    }

    #[tokio::test]
    async fn iceberg_clump_relaxed_threshold_keeps_more() {
        // With r2=0.95, no pair exceeds threshold → all 4 retained.
        let session = test_session();
        register_mock_ld_tables(&session).await;

        let inputs = iceberg_test_inputs();
        let cfg = ClumpConfig {
            r2: 0.95,
            kb: 5000,
            p1: 5e-8,
            mode: ClumpMode::IcebergLd,
            ..ClumpConfig::default()
        };
        let clumped = clump_instruments(inputs, &cfg, &session, Some("eur_chr"))
            .await
            .expect("relaxed clumping should succeed");
        assert_eq!(clumped.len(), 4, "with r²=0.95 no SNPs should be pruned");
    }

    #[tokio::test]
    async fn iceberg_clump_no_ld_data_keeps_all() {
        // SNPs with no r² entries in the table → treated as independent → all kept.
        let session = test_session();
        register_mock_ld_tables(&session).await;

        let inputs = vec![
            mr::harmonise::HarmoniseInput {
                snp: "rs_isolated1".to_string(),
                id_exposure: "exp".to_string(),
                id_outcome: "out".to_string(),
                beta_exposure: 0.1,
                beta_outcome: 0.05,
                se_exposure: 0.01,
                se_outcome: 0.01,
                effect_allele_exposure: Some("A".to_string()),
                other_allele_exposure: Some("G".to_string()),
                effect_allele_outcome: Some("A".to_string()),
                other_allele_outcome: Some("G".to_string()),
                eaf_exposure: Some(0.4),
                eaf_outcome: Some(0.4),
            },
            mr::harmonise::HarmoniseInput {
                snp: "rs_isolated2".to_string(),
                id_exposure: "exp".to_string(),
                id_outcome: "out".to_string(),
                beta_exposure: 0.08,
                beta_outcome: 0.04,
                se_exposure: 0.01,
                se_outcome: 0.01,
                effect_allele_exposure: Some("A".to_string()),
                other_allele_exposure: Some("G".to_string()),
                effect_allele_outcome: Some("A".to_string()),
                other_allele_outcome: Some("G".to_string()),
                eaf_exposure: Some(0.4),
                eaf_outcome: Some(0.4),
            },
        ];
        let cfg = ClumpConfig {
            r2: 0.001,
            kb: 5000,
            p1: 5e-8,
            mode: ClumpMode::IcebergLd,
            ..ClumpConfig::default()
        };
        let clumped = clump_instruments(inputs, &cfg, &session, Some("eur_chr"))
            .await
            .expect("clumping should succeed");
        assert_eq!(
            clumped.len(),
            2,
            "SNPs with no LD data should be treated as independent"
        );
    }
}
