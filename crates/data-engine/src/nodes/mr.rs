//! Mendelian randomisation (MR) transform node.
//!
//! Wraps the pure-Rust [`mr`] crate (a port of TwoSampleMR's algorithm API).
//!
//! Takes a single upstream `DataFrame` whose rows are **already merged on SNP**
//! — one record per SNP per `(id_exposure, id_outcome)` pair, carrying both the
//! exposure and the outcome summary statistics (effect alleles, betas, SEs,
//! effect allele frequencies). The node runs allele harmonisation
//! ([`mr::harmonise::harmonise_data_with`]) and then the main MR dispatch
//! ([`mr::dispatch::mr`]) over the requested methods, emitting one row per
//! `(exposure, outcome, method)` estimate.
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
pub enum MrNodeError {
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
    #[error("harmonise action must be 1, 2, or 3 (got {0})")]
    InvalidAction(u8),
    #[error("column length mismatch: '{name}' has {len} rows, expected {expected}")]
    LengthMismatch {
        name: String,
        len: usize,
        expected: usize,
    },
}

impl From<MrNodeError> for DagError {
    fn from(e: MrNodeError) -> Self {
        DagError::NodeError {
            node_type: "mr".to_string(),
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
const IN_ID_EXP: &str = "id_exposure";
const IN_ID_OUT: &str = "id_outcome";
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
        Field::new(IN_SNP, DataType::Utf8, false),
        Field::new(IN_ID_EXP, DataType::Utf8, false),
        Field::new(IN_ID_OUT, DataType::Utf8, false),
        Field::new(IN_BETA_EXP, DataType::Float64, true),
        Field::new(IN_BETA_OUT, DataType::Float64, true),
        Field::new(IN_SE_EXP, DataType::Float64, true),
        Field::new(IN_SE_OUT, DataType::Float64, true),
        Field::new(IN_EA_EXP, DataType::Utf8, true),
        Field::new(IN_OA_EXP, DataType::Utf8, true),
        Field::new(IN_EA_OUT, DataType::Utf8, true),
        Field::new(IN_OA_OUT, DataType::Utf8, true),
        Field::new(IN_EAF_EXP, DataType::Float64, true),
        Field::new(IN_EAF_OUT, DataType::Float64, true),
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

fn column_index(batches: &[RecordBatch], name: &str) -> Result<usize, MrNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or(MrNodeError::EmptyInput)?;
    schema
        .index_of(name)
        .map_err(|_| MrNodeError::MissingColumn {
            name: name.to_string(),
        })
}

/// Extract a required (non-null) Utf8 column into `Vec<String>`. Errors on null
/// cells — `snp` / `id_exposure` / `id_outcome` are mandatory join/group keys.
fn extract_required_string(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<String>, MrNodeError> {
    let idx = column_index(batches, name)?;
    // Validate the type once on the first batch (all batches share the schema).
    let dtype = batches[0].schema().field(idx).data_type().clone();
    if !matches!(dtype, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View) {
        return Err(MrNodeError::WrongColumnType {
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
            out.push(v.map(str::to_string).ok_or(MrNodeError::WrongColumnType {
                name: name.to_string(),
                dtype: "null".to_string(),
            })?);
        }
    }
    Ok(out)
}

/// Extract a nullable Utf8 column into `Vec<Option<String>>` (R's `NA` alleles).
fn extract_opt_string(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<Option<String>>, MrNodeError> {
    let idx = column_index(batches, name)?;
    let dtype = batches[0].schema().field(idx).data_type().clone();
    if !matches!(dtype, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View) {
        return Err(MrNodeError::WrongColumnType {
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
fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, MrNodeError> {
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
        return Err(MrNodeError::WrongColumnType {
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
fn extract_opt_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<Option<f64>>, MrNodeError> {
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
        return Err(MrNodeError::WrongColumnType {
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
pub struct MrParameters {
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
    /// Loss function name: `"l2"`, `"huber"`, `"tukey"`.
    pub loss_function: String,
    /// Whether empirical partially-Bayes shrinkage is applied.
    pub shrinkage: bool,
}

impl Default for MrParameters {
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
            loss_function: p.loss_function,
            shrinkage: p.shrinkage,
        }
    }
}

impl From<MrParameters> for mr::Parameters {
    fn from(p: MrParameters) -> Self {
        mr::Parameters {
            test_dist: p.test_dist,
            nboot: p.nboot,
            cov: p.cov,
            penk: p.penk,
            phi: p.phi,
            alpha: p.alpha,
            qthresh: p.qthresh,
            over_dispersion: p.over_dispersion,
            loss_function: p.loss_function,
            shrinkage: p.shrinkage,
        }
    }
}

fn default_action() -> u8 {
    2
}
fn default_tolerance() -> f64 {
    0.08
}

/// Spec for the MR node, deserialised from the registry-provided JSON.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MrNodeSpec {
    /// Methods to run, as `mr_method_list()` `obj` names (e.g. `"mr_ivw"`).
    /// Empty (the default) selects the `use_by_default` method set.
    #[serde(default)]
    pub method_list: Vec<String>,
    /// Harmonisation strictness: `1` (forward strand), `2` (infer strand via
    /// allele frequencies, default), `3` (drop all palindromic SNPs).
    #[serde(default = "default_action")]
    pub action: u8,
    /// Allele-frequency tolerance for palindrome inference (default `0.08`).
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Full MR [`MrParameters`] (defaults reproduce `default_parameters()`).
    #[serde(default)]
    pub parameters: MrParameters,
}

// =====================================================================
// Node
// =====================================================================

const MR_NODE_KIND: &str = "mr";

/// A transform node that runs allele harmonisation + the main MR dispatch.
///
/// The upstream `DataFrame` must be merged on SNP and carry the fixed input
/// columns (see [`input_schema`]).
#[derive(Clone)]
pub struct MrNode {
    meta: NodePorts,
    spec: MrNodeSpec,
}

impl MrNode {
    /// Construct an [`MrNode`] from a fully-specified [`MrNodeSpec`].
    pub fn new(spec: MrNodeSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
        }
    }
}

pub struct MrNodeFactory {}

/// Static port layout for every [`MrNode`]: one typed input (harmonised GWAS
/// sumstats, see [`input_schema`]) and one typed output ([`output_schema`]).
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl NodeFactory for MrNodeFactory {
    fn kind(&self) -> &'static str {
        MR_NODE_KIND
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
        schema_for!(MrNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let spec: MrNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MrNode::new(spec)))
    }
}

#[async_trait]
impl DagNode for MrNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        MR_NODE_KIND
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
        let input = inputs.first().ok_or(MrNodeError::EmptyInput)?;
        if !matches!(self.spec.action, 1..=3) {
            return Err(MrNodeError::InvalidAction(self.spec.action).into());
        }

        let batches: Vec<RecordBatch> =
            input
                .data
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: MR_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            return Err(MrNodeError::EmptyInput.into());
        }

        // ---- extract columns ----
        let snp = extract_required_string(&batches, IN_SNP)?;
        let id_exp = extract_required_string(&batches, IN_ID_EXP)?;
        let id_out = extract_required_string(&batches, IN_ID_OUT)?;
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
            (IN_ID_EXP, id_exp.len()),
            (IN_ID_OUT, id_out.len()),
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
                return Err(MrNodeError::LengthMismatch {
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
                id_exposure: id_exp[i].clone(),
                id_outcome: id_out[i].clone(),
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

        // ---- harmonise ----
        let harmonised =
            mr::harmonise::harmonise_data_with(&hinputs, self.spec.action, self.spec.tolerance);

        // ---- dispatch mr() ----
        let parameters: mr::Parameters = self.spec.parameters.clone().into();
        let method_refs: Vec<&str> = self.spec.method_list.iter().map(|s| s.as_str()).collect();
        let rows =
            mr::dispatch::mr(&harmonised, &parameters, &method_refs).map_err(MrNodeError::Mr)?;

        // ---- build output batch ----
        let batch = build_result_batch(&rows)?;
        let ctx = node_ctx.session();
        let df = ctx.read_batch(batch).map_err(MrNodeError::from)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build the output `RecordBatch` (one row per [`mr::dispatch::MrResultRow`]).
fn build_result_batch(rows: &[mr::dispatch::MrResultRow]) -> Result<RecordBatch, MrNodeError> {
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
    fn node_ctx() -> crate::node_registry::registry::NodeCtx {
        crate::node_registry::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }
    use super::*;
    use arrow_array::Array;
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    /// Build a `RecordBatch` from typed column arrays.
    fn make_batch(columns: Vec<(&str, Arc<dyn Array>)>) -> RecordBatch {
        let fields: Vec<Field> = columns
            .iter()
            .map(|(name, arr)| Field::new(*name, arr.data_type().clone(), arr.null_count() != 0))
            .collect();
        let arrays: Vec<Arc<dyn Array>> = columns.into_iter().map(|(_, a)| a).collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    /// Build a small SNP-merged input with matching alleles (so harmonise keeps
    /// all rows) across several instruments — enough for IVW / Egger / median /
    /// mode to fire.
    fn make_test_input() -> super::super::meta::NodeInput {
        let snp: Vec<&str> = vec!["rs1", "rs2", "rs3", "rs4"];
        let id_exp: Vec<&str> = vec!["exp"; 4];
        let id_out: Vec<&str> = vec!["out"; 4];
        let beta_exp = vec![0.10, 0.20, -0.15, 0.05];
        let beta_out = vec![0.045, 0.091, -0.060, 0.022];
        let se_exp = vec![0.01, 0.01, 0.01, 0.01];
        let se_out = vec![0.01, 0.01, 0.01, 0.01];
        let ea_exp: Vec<&str> = vec!["A", "G", "T", "C"];
        let oa_exp: Vec<&str> = vec!["G", "A", "C", "G"];
        let ea_out: Vec<&str> = vec!["A", "G", "T", "C"];
        let oa_out: Vec<&str> = vec!["G", "A", "C", "G"];
        let eaf_exp = vec![0.2, 0.3, 0.25, 0.4];
        let eaf_out = vec![0.2, 0.3, 0.25, 0.4];

        let batch = make_batch(vec![
            (IN_SNP, Arc::new(StringArray::from(snp.clone())) as _),
            (IN_ID_EXP, Arc::new(StringArray::from(id_exp)) as _),
            (IN_ID_OUT, Arc::new(StringArray::from(id_out)) as _),
            (IN_BETA_EXP, Arc::new(Float64Array::from(beta_exp)) as _),
            (IN_BETA_OUT, Arc::new(Float64Array::from(beta_out)) as _),
            (IN_SE_EXP, Arc::new(Float64Array::from(se_exp)) as _),
            (IN_SE_OUT, Arc::new(Float64Array::from(se_out)) as _),
            (IN_EA_EXP, Arc::new(StringArray::from(ea_exp)) as _),
            (IN_OA_EXP, Arc::new(StringArray::from(oa_exp)) as _),
            (IN_EA_OUT, Arc::new(StringArray::from(ea_out)) as _),
            (IN_OA_OUT, Arc::new(StringArray::from(oa_out)) as _),
            (IN_EAF_EXP, Arc::new(Float64Array::from(eaf_exp)) as _),
            (IN_EAF_OUT, Arc::new(Float64Array::from(eaf_out)) as _),
        ]);

        super::super::meta::NodeInput {
            port: 0,
            data: datafusion::prelude::SessionContext::new()
                .read_batch(batch)
                .unwrap(),
        }
    }

    #[tokio::test]
    async fn runs_default_methods_and_emits_ivw() {
        let mut node = MrNode::new(MrNodeSpec {
            method_list: vec![],
            action: default_action(),
            tolerance: default_tolerance(),
            parameters: MrParameters::default(),
        });
        assert_eq!(node.kind(), "mr");

        let outs = node
            .execute(
                &node_ctx(),
                &[make_test_input()],
                &crate::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outs.get(&0).unwrap().clone();
        let batches = df.collect().await.unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert!(total > 0, "expected at least one MR estimate row");

        // Collect the `method` column and confirm IVW is among the estimates.
        let methods: Vec<String> = batches
            .iter()
            .flat_map(|b| {
                b.column(2)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .iter()
            })
            .map(|v| v.unwrap().to_string())
            .collect();
        assert!(
            methods.iter().any(|m| m == "Inverse variance weighted"),
            "IVW estimate missing; got {methods:?}"
        );

        // Verify the output schema: nsnp is Int64.
        assert_eq!(batches[0].schema().field(3).data_type(), &DataType::Int64);
    }

    #[tokio::test]
    async fn respects_explicit_method_list() {
        let mut node = MrNode::new(MrNodeSpec {
            method_list: vec!["mr_ivw".to_string()],
            action: default_action(),
            tolerance: default_tolerance(),
            parameters: MrParameters::default(),
        });
        let outs = node
            .execute(
                &node_ctx(),
                &[make_test_input()],
                &crate::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let batches = outs.get(&0).unwrap().clone().collect().await.unwrap();
        let methods: Vec<String> = batches
            .iter()
            .flat_map(|b| {
                b.column(2)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .iter()
            })
            .map(|v| v.unwrap().to_string())
            .collect();
        // With a single SNP-set of size >1 and an explicit one-method list,
        // only IVW should be emitted.
        assert_eq!(methods, vec!["Inverse variance weighted"]);
    }

    #[tokio::test]
    async fn rejects_invalid_action() {
        let mut node = MrNode::new(MrNodeSpec {
            method_list: vec![],
            action: 9,
            tolerance: default_tolerance(),
            parameters: MrParameters::default(),
        });
        let err = node
            .execute(
                &node_ctx(),
                &[make_test_input()],
                &crate::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("action"), "got: {err}");
    }
}
