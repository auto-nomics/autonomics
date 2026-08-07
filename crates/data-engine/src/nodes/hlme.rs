//! Latent class linear mixed model DAG nodes (`hlme`, `hlme_predict`, `hlme_compare`).
//!
//! Wraps the pure-Rust [`lcmm`] crate — a faithful port of R `lcmm::hlme`
//! (Proust-Lima, Philipps, Liquet 2017, JSS 78(2)). The `hlme` function
//! subsumes GBTM (group-based trajectory modeling), LCGA (latent class
//! growth analysis), and LGMM (latent growth mixture modeling) as special cases.
//!
//! ## Nodes
//!
//! | Node | Kind | Description |
//! |------|------|-------------|
//! | **hlme** | `hlme` | Fit a latent class linear mixed model |
//! | **hlme_predict** | `hlme_predict` | Predict class-conditional trajectories for new data |
//! | **hlme_compare** | `hlme_compare` | Compare multiple fits via BIC/AIC |
//!
//! ## hlme outputs
//!
//! **Port 0 — summary** (1 row): loglik, AIC, BIC, convergence, class count, etc.
//!
//! **Port 1 — params** (1 row per parameter): section, index, estimate, std_error.
//!
//! **Port 2 — posterior** (1 row per subject): subject ID, assigned class, per-class
//! posterior probabilities.
//!
//! **Port 3 — fitted** (1 row per observation): outcome, marginal fitted, subject-specific
//! fitted, residuals.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{
    Array, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use lcmm::{
    HlmeControl, HlmeFit, LongData, ModelSpec, ParamLayout,
    hlme as hlme_fit, predict_y,
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use super::numeric_util::{ColumnError, column_index, extract_numeric_lenient};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error
// =====================================================================

#[derive(Debug, Error)]
pub enum HlmeNodeError {
    #[error("lcmm fit failed: {0}")]
    Lcmm(#[from] lcmm::LcmmError),
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("datafusion error: {0}")]
    Df(#[from] datafusion::error::DataFusionError),
    #[error("column error: {0}")]
    Column(String),
    #[error("invalid spec: {0}")]
    Spec(String),
    #[error("no input data")]
    EmptyInput,
}

impl From<ColumnError> for HlmeNodeError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl ::dag_core::dag::NodeError for HlmeNodeError {
    fn node_type(&self) -> &str { "hlme" }
}

// =====================================================================
// Term expansion — parse R-style formula terms
// =====================================================================

/// A single design-matrix term, which is either a raw column or an
/// interaction product of multiple columns.
#[derive(Clone, Debug)]
struct Term {
    /// Canonical name (e.g. `"Time"`, `"Time:X1"`).
    name: String,
    /// Source column names that are multiplied to form this term.
    sources: Vec<String>,
}

/// Expand a list of formula term strings into [`Term`]s.
///
/// Supports:
/// - `"A"` → `Term { name: "A", sources: ["A"] }`
/// - `"A:B"` → `Term { name: "A:B", sources: ["A", "B"] }`
/// - `"A*B"` → expands to three terms: `A`, `B`, `A:B`
fn expand_terms(terms: &[String]) -> Vec<Term> {
    let mut out = Vec::new();
    for t in terms {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(star_pos) = find_top_level(t, '*') {
            let lhs = t[..star_pos].trim();
            let rhs = t[star_pos + 1..].trim();
            // A*B → A, B, A:B
            for sub in expand_terms(&[lhs.to_string()]) {
                out.push(sub);
            }
            for sub in expand_terms(&[rhs.to_string()]) {
                out.push(sub);
            }
            let interact_name = format!("{}:{}", lhs, rhs);
            out.push(Term {
                name: interact_name,
                sources: vec![lhs.to_string(), rhs.to_string()],
            });
        } else if t.contains(':') {
            let parts: Vec<&str> = t.split(':').map(|s| s.trim()).collect();
            out.push(Term {
                name: t.to_string(),
                sources: parts.iter().map(|s| s.to_string()).collect(),
            });
        } else {
            out.push(Term {
                name: t.to_string(),
                sources: vec![t.to_string()],
            });
        }
    }
    out
}

/// Find the position of `ch` in `s` at parenthesis depth 0.
fn find_top_level(s: &str, ch: char) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth == 0 && c == ch => return Some(i),
            _ => {}
        }
    }
    None
}

// =====================================================================
// Design matrix builder
// =====================================================================

/// Build the design matrix, indicator vectors, and packed long-format data
/// from the input RecordBatch and the node spec.
fn build_model_data(
    batches: &[RecordBatch],
    cfg: &HlmeConfig,
) -> Result<(LongData, ModelSpec, Vec<String>), HlmeNodeError> {
    if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
        return Err(HlmeNodeError::EmptyInput);
    }

    // Parse and expand formula terms.
    let fixed_terms = expand_terms(&cfg.fixed);
    let mixture_terms = expand_terms(&cfg.mixture);
    let random_terms = expand_terms(&cfg.random);
    let classmb_terms = expand_terms(&cfg.classmb);

    // Build the column ordering: intercept first, then all unique terms
    // in order of first appearance (fixed, mixture, random, classmb).
    let mut col_names: Vec<String> = Vec::new();
    let mut col_seen: HashMap<String, usize> = HashMap::new();

    let register = |name: &str, col_names: &mut Vec<String>, col_seen: &mut HashMap<String, usize>| {
        if let std::collections::hash_map::Entry::Vacant(e) = col_seen.entry(name.to_string()) {
            e.insert(col_names.len());
            col_names.push(name.to_string());
        }
    };

    if cfg.intercept {
        register("__intercept", &mut col_names, &mut col_seen);
    }
    for t in &fixed_terms {
        register(&t.name, &mut col_names, &mut col_seen);
    }
    for t in &mixture_terms {
        register(&t.name, &mut col_names, &mut col_seen);
    }
    for t in &random_terms {
        register(&t.name, &mut col_names, &mut col_seen);
    }
    for t in &classmb_terms {
        register(&t.name, &mut col_names, &mut col_seen);
    }

    let nv = col_names.len();
    if nv == 0 {
        return Err(HlmeNodeError::Spec(
            "no design columns: specify at least one of fixed/mixture/random/classmb".into(),
        ));
    }

    // Extract outcome column.
    let y_all = extract_numeric_lenient(batches, &cfg.outcome)?;
    let nobs = y_all.len();

    // Extract subject IDs and build subject ordering.
    let subject_ids = extract_subject_ids(batches, &cfg.subject)?;
    debug_assert_eq!(subject_ids.len(), nobs);

    // Sort observations by subject ID (stable sort preserves original time order).
    let mut order: Vec<usize> = (0..nobs).collect();
    order.sort_by(|&a, &b| subject_ids[a].cmp(&subject_ids[b]));

    // Build nmes and offsets.
    let mut unique_subjects: Vec<i64> = Vec::new();
    let mut nmes: Vec<usize> = Vec::new();
    for &idx in &order {
        let sid = subject_ids[idx];
        if unique_subjects.is_empty() || *unique_subjects.last().unwrap() != sid {
            unique_subjects.push(sid);
            nmes.push(1);
        } else {
            *nmes.last_mut().unwrap() += 1;
        }
    }
    let _ns = unique_subjects.len();

    // Build Y (sorted).
    let y_sorted: Vec<f64> = order.iter().map(|&i| y_all[i]).collect();

    // Extract each design column and compute interaction terms.
    let mut col_data: Vec<Vec<f64>> = Vec::with_capacity(nv);
    for (k, name) in col_names.iter().enumerate() {
        if name == "__intercept" {
            col_data.push(vec![1.0; nobs]);
        } else {
            let term = [&fixed_terms, &mixture_terms, &random_terms, &classmb_terms]
                .into_iter()
                .flatten()
                .find(|t| &t.name == name)
                .expect("term must exist in one of the effect lists");
            let src_vals: Vec<Vec<f64>> = term
                .sources
                .iter()
                .map(|s| extract_numeric_lenient(batches, s))
                .collect::<Result<_, _>>()?;
            let col: Vec<f64> = (0..nobs)
                .map(|r| src_vals.iter().map(|v| v[r]).product())
                .collect();
            col_data.push(col);
        }
        let _ = k;
    }

    // Build packed X matrix (row-major nobs×nv, sorted by subject).
    let mut x = vec![0.0_f64; nobs * nv];
    for (out_row, &idx) in order.iter().enumerate() {
        for k in 0..nv {
            x[out_row * nv + k] = col_data[k][idx];
        }
    }

    // Build indicator vectors.
    let mut idprob = vec![0u8; nv];
    let mut idea = vec![0u8; nv];
    let mut idg = vec![0u8; nv];
    let mut idcor = vec![0u8; nv];

    // Intercept column assignment.
    if cfg.intercept {
        let k = *col_seen.get("__intercept").unwrap();
        // Intercept enters class membership when ng>1.
        if cfg.ng > 1 {
            idprob[k] = 1;
        }
        // Intercept has random effect when random is specified.
        if !random_terms.is_empty() {
            idea[k] = 1;
        }
        // Intercept is class-specific when mixture is specified.
        if !mixture_terms.is_empty() {
            idg[k] = 2;
        } else {
            idg[k] = 1;
        }
    }

    // Assign indicators for non-intercept columns.
    for (k, name) in col_names.iter().enumerate() {
        if name == "__intercept" {
            continue;
        }
        // idg: mixture takes precedence over fixed.
        if mixture_terms.iter().any(|t| &t.name == name) {
            idg[k] = 2;
        } else if fixed_terms.iter().any(|t| &t.name == name) {
            idg[k] = 1;
        }
        // idea: random effects.
        if random_terms.iter().any(|t| &t.name == name) {
            idea[k] = 1;
        }
        // idprob: class membership covariates.
        if classmb_terms.iter().any(|t| &t.name == name) {
            idprob[k] = 1;
        }
    }

    let data = LongData::new(y_sorted, x, nv, nmes, cfg.ng);
    let spec = ModelSpec {
        ng: cfg.ng,
        idiag: cfg.idiag,
        nwg: cfg.nwg,
        ncor: 0, // TODO: support BM/AR correlation
        idprob,
        idea,
        idg,
        idcor,
    };

    // Build human-readable column labels (replacing __intercept with "intercept").
    let labels: Vec<String> = col_names
        .iter()
        .map(|n| {
            if n == "__intercept" {
                "intercept".to_string()
            } else {
                n.clone()
            }
        })
        .collect();

    Ok((data, spec, labels))
}

/// Extract subject IDs from a column, accepting integer types.
fn extract_subject_ids(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<i64>, HlmeNodeError> {
    let idx = column_index(batches, name)?;
    let mut ids = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        extract_int_dispatch(col, &mut |v: Option<i64>| {
            ids.push(v.unwrap_or(0));
        });
    }
    Ok(ids)
}

fn extract_int_dispatch(col: &dyn Array, emit: &mut impl FnMut(Option<i64>)) {
    macro_rules! cast {
        ($arr:expr, $T:ty) => {
            if let Some(a) = $arr.as_any().downcast_ref::<$T>() {
                for v in a.iter() {
                    emit(v.map(|val| val as i64));
                }
                return;
            }
        };
    }
    cast!(col, arrow_array::Int8Array);
    cast!(col, arrow_array::Int16Array);
    cast!(col, arrow_array::Int32Array);
    cast!(col, arrow_array::Int64Array);
    cast!(col, arrow_array::UInt8Array);
    cast!(col, arrow_array::UInt16Array);
    cast!(col, arrow_array::UInt32Array);
    cast!(col, arrow_array::UInt64Array);
    for _ in 0..col.len() {
        emit(None);
    }
}

// =====================================================================
// Initial parameter vector
// =====================================================================

/// Compute a default starting parameter vector for the optimizer.
///
/// - NPROB: 0.0 (equal class probabilities)
/// - NEF: 0.0 for overall; for class-specific, reference class=0, others small perturbation
/// - NVC: Cholesky diagonal=1.0, off-diagonal=0.0
/// - NW: 1.0 (no class scaling difference)
/// - STDERR: standard deviation of the outcome
fn default_init_b(data: &LongData, spec: &ModelSpec) -> Vec<f64> {
    let layout = spec.layout();
    let mut b = vec![0.0_f64; layout.npm];

    // Variance-covariance Cholesky block.
    if layout.nvc > 0 {
        if spec.idiag {
            for j in 0..layout.nea {
                b[layout.i_nvc + j] = 1.0;
            }
        } else {
            // Upper-triangular packed: diagonal entries = 1.0.
            for i in 0..layout.nea {
                let diag_idx = layout.i_nvc + i * (i + 1) / 2 + i;
                b[diag_idx] = 1.0;
            }
        }
    }

    // NW (class-specific RE scaling).
    for k in 0..layout.nw {
        b[layout.i_nw + k] = 1.0;
    }

    // Residual stderr: use sample standard deviation of Y.
    let y_mean = data.y.iter().sum::<f64>() / data.y.len() as f64;
    let y_var = data.y.iter().map(|y| (y - y_mean).powi(2)).sum::<f64>() / data.y.len() as f64;
    b[layout.i_stderr] = y_var.sqrt().max(0.1);
    b
}

// =====================================================================
// Output schemas
// =====================================================================

fn summary_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("loglik", DataType::Float64, false),
        Field::new("aic", DataType::Float64, false),
        Field::new("bic", DataType::Float64, false),
        Field::new("niter", DataType::Int32, false),
        Field::new("conv", DataType::Utf8, false),
        Field::new("ng", DataType::Int32, false),
        Field::new("npm", DataType::Int32, false),
        Field::new("ns", DataType::Int32, false),
        Field::new("nobs", DataType::Int32, false),
        Field::new("n_param_eff", DataType::Int32, false),
    ]))
}

fn params_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("section", DataType::Utf8, false),
        Field::new("index", DataType::Int32, false),
        Field::new("estimate", DataType::Float64, false),
        Field::new("std_error", DataType::Float64, true),
    ]))
}

fn posterior_schema(ng: usize) -> SchemaRef {
    let mut fields = vec![
        Field::new("subject", DataType::Int64, false),
        Field::new("class", DataType::Int32, false),
    ];
    for g in 1..=ng {
        fields.push(Field::new(format!("ppi_{g}"), DataType::Float64, false));
    }
    Arc::new(Schema::new(fields))
}

fn fitted_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("subject", DataType::Int64, false),
        Field::new("outcome", DataType::Float64, false),
        Field::new("pred_marginal", DataType::Float64, true),
        Field::new("pred_subject", DataType::Float64, true),
        Field::new("resid_marginal", DataType::Float64, true),
        Field::new("resid_subject", DataType::Float64, true),
    ]))
}

fn predict_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("row", DataType::Int32, false),
        Field::new("class", DataType::Int32, false),
        Field::new("predicted", DataType::Float64, false),
    ]))
}

fn compare_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("model", DataType::Utf8, false),
        Field::new("ng", DataType::Int32, false),
        Field::new("npm", DataType::Int32, false),
        Field::new("loglik", DataType::Float64, false),
        Field::new("aic", DataType::Float64, false),
        Field::new("bic", DataType::Float64, false),
        Field::new("bic_delta", DataType::Float64, true),
        Field::new("bic_prob", DataType::Float64, true),
    ]))
}

// =====================================================================
// Node 1: hlme — Fit a latent class linear mixed model
// =====================================================================

const HLME_NODE_KIND: &str = "hlme";

/// Default values.
fn default_ng() -> usize { 1 }
fn default_intercept() -> bool { true }
fn default_idiag() -> bool { false }
fn default_nwg() -> bool { false }
fn default_maxiter() -> usize { 500 }

/// Configuration for the `hlme` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HlmeConfig {
    /// Column name for the subject/patient ID.
    pub subject: String,
    /// Column name for the outcome (continuous).
    pub outcome: String,
    /// Number of latent classes (≥1). Use 1 for a standard LMM.
    #[serde(default = "default_ng")]
    pub ng: usize,
    /// Whether to include an intercept column in the design matrix.
    #[serde(default = "default_intercept")]
    pub intercept: bool,
    /// Column names or formula terms for overall fixed effects (idg=1).
    /// Supports R-style notation: `"Time*X1"` expands to Time + X1 + Time:X1.
    #[serde(default)]
    pub fixed: Vec<String>,
    /// Column names for class-specific fixed effects (idg=2).
    /// When non-empty, the intercept (if present) also becomes class-specific.
    #[serde(default)]
    pub mixture: Vec<String>,
    /// Column names with random effects (idea=1).
    /// When non-empty, the intercept (if present) also gets a random effect.
    /// Leave empty for GBTM/LCGA (no random effects).
    #[serde(default)]
    pub random: Vec<String>,
    /// Column names for class-membership covariates (idprob=1).
    #[serde(default)]
    pub classmb: Vec<String>,
    /// Diagonal random-effect covariance (no off-diagonal correlations).
    #[serde(default = "default_idiag")]
    pub idiag: bool,
    /// Class-specific residual variance scaling (nwg).
    #[serde(default = "default_nwg")]
    pub nwg: bool,
    /// Maximum optimizer iterations.
    #[serde(default = "default_maxiter")]
    pub maxiter: usize,
    /// Optional explicit initial parameter vector (length = NPM).
    /// If omitted, reasonable defaults are computed.
    #[serde(default)]
    pub init_b: Vec<f64>,
}

fn hlme_ports(ng: usize) -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(summary_schema()))      // 0: summary
        .add_output_port(Some(params_schema()))        // 1: params
        .add_output_port(Some(posterior_schema(ng)))   // 2: posterior
        .add_output_port(Some(fitted_schema()))        // 3: fitted
}

#[derive(Clone)]
pub struct HlmeNode {
    meta: NodePorts,
    config: HlmeConfig,
}

impl HlmeNode {
    pub fn new(config: HlmeConfig) -> Self {
        let meta = hlme_ports(config.ng);
        Self { meta, config }
    }
}

pub struct HlmeNodeFactory {}

impl NodeFactory for HlmeNodeFactory {
    fn kind(&self) -> &'static str {
        HLME_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Latent class linear mixed model (lcmm::hlme): GBTM, LCGA, and LGMM."
    }

    fn doc(&self) -> &'static str {
        "Latent class linear mixed model (Proust-Lima et al. 2017, JSS 78(2)). \
        Fits a mixture of linear mixed models with class-specific and/or overall \
        fixed effects, optional random effects, and multinomial-logit class \
        membership. Subsumes GBTM (no random effects), LCGA, and LGMM. \
        Outputs: summary, parameter estimates, posterior class probabilities, \
        and fitted values with residuals."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HlmeConfig)
    }

    fn ports(&self) -> NodePorts {
        // Use ng=1 as default for the static port schema. The actual node
        // instance adjusts the posterior port columns based on its ng.
        hlme_ports(1)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: HlmeConfig = serde_json::from_value(spec)?;
        Ok(Box::new(HlmeNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let cfg = parse_spec::<HlmeConfig>(spec, "hlme")?;
        let input = ctx.input_vars.first().map(|s| s.as_str()).unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        // Build R formula strings.
        let fixed_formula = if cfg.fixed.is_empty() {
            "1".to_string()
        } else {
            cfg.fixed.join(" + ")
        };
        let response_part = format!("{} ~ {}", cfg.outcome, fixed_formula);

        let mut args: Vec<(&str, String)> = vec![
            ("fixed", response_part),
            ("subject", r_str(&cfg.subject)),
            ("ng", cfg.ng.to_string()),
            ("data", input.to_string()),
        ];

        if !cfg.mixture.is_empty() {
            args.push(("mixture", format!("~ {}", cfg.mixture.join(" + "))));
        }
        if !cfg.random.is_empty() {
            args.push(("random", format!("~ {}", cfg.random.join(" + "))));
        }
        if !cfg.classmb.is_empty() {
            args.push(("classmb", format!("~ {}", cfg.classmb.join(" + "))));
        }
        if cfg.idiag {
            args.push(("idiag", "TRUE".into()));
        }
        if cfg.nwg {
            args.push(("nwg", "TRUE".into()));
        }
        if cfg.maxiter != 500 {
            args.push(("maxiter", cfg.maxiter.to_string()));
        }

        let code = vec![
            "# hlme: Latent class linear mixed model".to_string(),
            "library(lcmm)".to_string(),
            format!("set.seed(1)"),
            format!("{out} <- {}", r_call_multiline("hlme", &args, 0)),
            format!("print(summary({out}))"),
        ];

        Ok(crate::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["lcmm".into()]
    }
}

#[async_trait]
impl DagNode for HlmeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        HLME_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(HlmeNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: HLME_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;

        let cfg = &self.config;
        let (data, spec, col_labels) = build_model_data(&batches, cfg)?;
        let layout = spec.layout();

        // Initial parameter vector.
        let b_init = if cfg.init_b.len() == layout.npm {
            cfg.init_b.clone()
        } else if !cfg.init_b.is_empty() {
            return Err(HlmeNodeError::Spec(format!(
                "init_b length {} != expected NPM {}",
                cfg.init_b.len(),
                layout.npm
            )).into());
        } else {
            default_init_b(&data, &spec)
        };

        let control = HlmeControl {
            maxiter: cfg.maxiter,
            ..Default::default()
        };

        let fit = hlme_fit(&data, &spec, &b_init, &[], &control)
            .map_err(HlmeNodeError::Lcmm)?;

        // Build output batches.
        let summary_batch = build_summary_batch(&fit, data.nobs);
        let params_batch = build_params_batch(&fit, &col_labels);
        let session = node_ctx.session();
        let df_summary = session.read_batch(summary_batch).map_err(HlmeNodeError::Df)?;
        let df_params = session.read_batch(params_batch).map_err(HlmeNodeError::Df)?;

        // Posterior + fitted ports: only computed when the fit is usable.
        let (df_posterior, df_fitted) = if fit.posterior.is_some() {
            let posterior_batch = build_posterior_batch(&fit, &data);
            let fitted_batch = build_fitted_batch(&fit, &data);
            let df_posterior = session.read_batch(posterior_batch).map_err(HlmeNodeError::Df)?;
            let df_fitted = session.read_batch(fitted_batch).map_err(HlmeNodeError::Df)?;
            (df_posterior, df_fitted)
        } else {
            // maxiter=0 or failed convergence: emit empty batches.
            let empty_posterior = session.read_batch(
                RecordBatch::new_empty(posterior_schema(fit.ng))
            ).map_err(HlmeNodeError::Df)?;
            let empty_fitted = session.read_batch(
                RecordBatch::new_empty(fitted_schema())
            ).map_err(HlmeNodeError::Df)?;
            (empty_posterior, empty_fitted)
        };

        let mut res = PortOutputs::new();
        res.insert(0, df_summary);
        res.insert(1, df_params);
        res.insert(2, df_posterior);
        res.insert(3, df_fitted);
        Ok(res)
    }
}

// =====================================================================
// Batch builders for hlme outputs
// =====================================================================

fn build_summary_batch(fit: &HlmeFit, nobs: usize) -> RecordBatch {
    RecordBatch::try_new(
        summary_schema(),
        vec![
            Arc::new(Float64Array::from(vec![fit.loglik])),
            Arc::new(Float64Array::from(vec![fit.aic])),
            Arc::new(Float64Array::from(vec![fit.bic])),
            Arc::new(Int32Array::from(vec![fit.niter as i32])),
            Arc::new(StringArray::from(vec![format!("{:?}", fit.conv)])),
            Arc::new(Int32Array::from(vec![fit.ng as i32])),
            Arc::new(Int32Array::from(vec![fit.npm() as i32])),
            Arc::new(Int32Array::from(vec![fit.ns as i32])),
            Arc::new(Int32Array::from(vec![nobs as i32])),
            Arc::new(Int32Array::from(vec![fit.npm() as i32])),
        ],
    )
    .expect("summary batch construction must not fail")
}

fn build_params_batch(fit: &HlmeFit, col_labels: &[String]) -> RecordBatch {
    #[allow(non_snake_case)]
    let L = &fit.layout;
    let best = &fit.best;

    // Extract diagonal std errors from the packed upper-tri V.
    let se = packed_diag(&fit.v, L.npm);

    let mut sections: Vec<String> = Vec::new();
    let mut indices: Vec<i32> = Vec::new();
    let mut estimates: Vec<f64> = Vec::new();
    let mut std_errs: Vec<Option<f64>> = Vec::new();

    let mut idx = 0usize;

    // NPROB section.
    if L.nprob > 0 {
        for k in 0..L.nprob {
            sections.push("class_membership".into());
            indices.push(k as i32);
            estimates.push(best[idx]);
            std_errs.push(se[idx]);
            idx += 1;
        }
    }

    // NEF section.
    let nef_end = L.i_nvc;
    while idx < nef_end {
        sections.push("fixed_effect".into());
        indices.push((idx - L.i_nef) as i32);
        estimates.push(best[idx]);
        std_errs.push(se[idx]);
        idx += 1;
    }

    // NVC section.
    if L.nvc > 0 {
        for k in 0..L.nvc {
            sections.push("varcov".into());
            indices.push(k as i32);
            estimates.push(best[idx]);
            std_errs.push(se[idx]);
            idx += 1;
        }
    }

    // NW section.
    if L.nw > 0 {
        for k in 0..L.nw {
            sections.push("nwg".into());
            indices.push(k as i32);
            estimates.push(best[idx]);
            std_errs.push(se[idx]);
            idx += 1;
        }
    }

    // NCOR section.
    if L.ncor > 0 {
        for k in 0..L.ncor {
            sections.push("correlation".into());
            indices.push(k as i32);
            estimates.push(best[idx]);
            std_errs.push(se[idx]);
            idx += 1;
        }
    }

    // STDERR.
    sections.push("stderr".into());
    indices.push(0);
    estimates.push(best[idx]);
    std_errs.push(se[idx]);

    let _ = col_labels; // TODO: enrich with term names

    RecordBatch::try_new(
        params_schema(),
        vec![
            Arc::new(StringArray::from(sections)),
            Arc::new(Int32Array::from(indices)),
            Arc::new(Float64Array::from(estimates)),
            Arc::new(Float64Array::from(std_errs)),
        ],
    )
    .expect("params batch construction must not fail")
}

fn build_posterior_batch(fit: &HlmeFit, data: &LongData) -> RecordBatch {
    let ng = fit.ng;
    let ns = data.ns;

    let post = fit.posterior.as_ref().unwrap_or_else(|| {
        panic!("posterior should be computed for conv {:?}", fit.conv)
    });

    // Subject IDs: use the sorted unique subjects from data.
    // LongData doesn't store original IDs; we emit 0-based indices.
    let subjects: Vec<i64> = (0..ns as i64).collect();
    let classes: Vec<i32> = post.class.iter().map(|&c| c as i32).collect();

    let mut ppi_cols: Vec<Vec<f64>> = Vec::with_capacity(ng);
    for g in 0..ng {
        let col: Vec<f64> = (0..ns).map(|i| post.ppi[i * ng + g]).collect();
        ppi_cols.push(col);
    }

    let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(2 + ng);
    arrays.push(Arc::new(Int64Array::from(subjects)));
    arrays.push(Arc::new(Int32Array::from(classes)));
    for col in ppi_cols {
        arrays.push(Arc::new(Float64Array::from(col)));
    }

    RecordBatch::try_new(posterior_schema(ng), arrays)
        .expect("posterior batch construction must not fail")
}

fn build_fitted_batch(fit: &HlmeFit, data: &LongData) -> RecordBatch {
    let post = fit.posterior.as_ref().unwrap();
    let ng = fit.ng;

    // Subject index for each observation (repeated nmes[i] times).
    let mut subj_ids: Vec<i64> = Vec::with_capacity(data.nobs);
    for i in 0..data.ns {
        for _ in 0..data.nmes[i] {
            subj_ids.push(i as i64);
        }
    }

    // Marginal fitted: π-weighted class mean.
    let pred_marginal: Vec<f64> = (0..data.nobs)
        .map(|r| {
            let s = subj_ids[r] as usize;
            (0..ng).map(|g| {
                let pi = post.ppi[s * ng + g];
                pi * post.pred_m_g[r * ng + g]
            }).sum::<f64>()
        })
        .collect();

    // Subject-specific fitted: π-weighted.
    let pred_subject: Vec<f64> = (0..data.nobs)
        .map(|r| {
            let s = subj_ids[r] as usize;
            (0..ng).map(|g| {
                let pi = post.ppi[s * ng + g];
                pi * post.pred_ss_g[r * ng + g]
            }).sum::<f64>()
        })
        .collect();

    RecordBatch::try_new(
        fitted_schema(),
        vec![
            Arc::new(Int64Array::from(subj_ids)),
            Arc::new(Float64Array::from(data.y.clone())),
            Arc::new(Float64Array::from(pred_marginal)),
            Arc::new(Float64Array::from(pred_subject)),
            Arc::new(Float64Array::from(post.resid_m.clone())),
            Arc::new(Float64Array::from(post.resid_ss.clone())),
        ],
    )
    .expect("fitted batch construction must not fail")
}

/// Extract diagonal elements from a packed column-major upper-triangular matrix.
fn packed_diag(v_packed: &[f64], n: usize) -> Vec<Option<f64>> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let packed_idx = i * (i + 1) / 2 + i;
        let val = v_packed.get(packed_idx).copied().unwrap_or(f64::NAN);
        if val.is_nan() {
            out.push(None);
        } else {
            out.push(Some(val));
        }
    }
    out
}

// =====================================================================
// Node 2: hlme_predict — Predict class-conditional trajectories
// =====================================================================

const HLME_PREDICT_NODE_KIND: &str = "hlme_predict";

/// Configuration for the `hlme_predict` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HlmePredictConfig {
    /// Column names matching the original fit's design matrix ordering.
    /// For interaction terms, specify the source columns in order; the node
    /// computes interactions automatically.
    pub columns: Vec<String>,
    /// Whether the original model included an intercept.
    #[serde(default = "default_intercept")]
    pub intercept: bool,
    /// Number of latent classes.
    pub ng: usize,
    /// Full parameter vector (`best`) from the fitted model.
    pub best: Vec<f64>,
    /// Model spec indicators.
    #[serde(default)]
    pub idprob: Vec<u8>,
    #[serde(default)]
    pub idea: Vec<u8>,
    pub idg: Vec<u8>,
    #[serde(default)]
    pub idcor: Vec<u8>,
    /// Original fixed/mixture/classmb term names (for interaction expansion).
    #[serde(default)]
    pub fixed_terms: Vec<String>,
    #[serde(default)]
    pub mixture_terms: Vec<String>,
    #[serde(default)]
    pub classmb_terms: Vec<String>,
}

fn predict_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)   // newdata
        .add_output_port(Some(predict_schema()))
}

#[derive(Clone)]
pub struct HlmePredictNode {
    meta: NodePorts,
    config: HlmePredictConfig,
}

impl HlmePredictNode {
    pub fn new(config: HlmePredictConfig) -> Self {
        Self {
            meta: predict_ports(),
            config,
        }
    }
}

pub struct HlmePredictNodeFactory {}

impl NodeFactory for HlmePredictNodeFactory {
    fn kind(&self) -> &'static str {
        HLME_PREDICT_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Predict class-conditional trajectories from a fitted hlme model."
    }

    fn doc(&self) -> &'static str {
        "Predicts class-conditional marginal mean trajectories for new data, \
        using the parameter estimates from a previously fitted hlme model. \
        Accepts a newdata DataFrame and produces predicted means for each class."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HlmePredictConfig)
    }

    fn ports(&self) -> NodePorts {
        predict_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: HlmePredictConfig = serde_json::from_value(spec)?;
        Ok(Box::new(HlmePredictNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let cfg = parse_spec::<HlmePredictConfig>(spec, "hlme_predict")?;
        let input = ctx.input_vars.first().map(|s| s.as_str()).unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        let newdata_cols: Vec<String> = cfg.columns.iter()
            .map(|c| r_col(input, c))
            .collect();
        let newdata_df = r_dataframe(
            &cfg.columns.iter().map(|c| c.as_str()).zip(newdata_cols.iter().map(|s| s.as_str()))
                .collect::<Vec<_>>(),
        );

        let code = vec![
            "# hlme_predict: class-conditional trajectory prediction".to_string(),
            "library(lcmm)".to_string(),
            format!("newdata <- {newdata_df}"),
            format!("{out} <- predictY(model, newdata = newdata, var.time = {}, draws = FALSE)",
                r_str(&cfg.columns.first().map(|s| s.as_str()).unwrap_or("Time"))),
        ];

        Ok(crate::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["lcmm".into()]
    }
}

#[async_trait]
impl DagNode for HlmePredictNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        HLME_PREDICT_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(HlmeNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: HLME_PREDICT_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;

        if batches.is_empty() {
            return Err(HlmeNodeError::EmptyInput.into());
        }

        let cfg = &self.config;

        // Build the design matrix for newdata, matching the original fit's
        // column ordering.
        let fixed_terms = expand_terms(&cfg.fixed_terms);
        let mixture_terms = expand_terms(&cfg.mixture_terms);
        let classmb_terms = expand_terms(&cfg.classmb_terms);

        // Assemble column list in the same order as the original fit.
        let mut col_names: Vec<String> = Vec::new();
        let mut col_seen: HashMap<String, usize> = HashMap::new();
        let register = |name: &str, col_names: &mut Vec<String>, col_seen: &mut HashMap<String, usize>| {
            if let std::collections::hash_map::Entry::Vacant(e) = col_seen.entry(name.to_string()) {
                e.insert(col_names.len());
                col_names.push(name.to_string());
            }
        };
        if cfg.intercept {
            register("__intercept", &mut col_names, &mut col_seen);
        }
        for t in &fixed_terms { register(&t.name, &mut col_names, &mut col_seen); }
        for t in &mixture_terms { register(&t.name, &mut col_names, &mut col_seen); }
        for t in &classmb_terms { register(&t.name, &mut col_names, &mut col_seen); }

        let nv = col_names.len();
        let nobs: usize = batches.iter().map(|b| b.num_rows()).sum();

        // Build X matrix.
        let mut x = vec![0.0_f64; nobs * nv];
        for (k, name) in col_names.iter().enumerate() {
            if name == "__intercept" {
                for r in 0..nobs { x[r * nv + k] = 1.0; }
            } else {
                let term = [&fixed_terms, &mixture_terms, &classmb_terms]
                    .into_iter()
                    .flatten()
                    .find(|t| &t.name == name)
                    .ok_or_else(|| HlmeNodeError::Spec(format!("term '{name}' not found")))?;
                let src_vals: Vec<Vec<f64>> = term.sources
                    .iter()
                    .map(|s| extract_numeric_lenient(&batches, s))
                    .collect::<Result<_, _>>()?;
                for r in 0..nobs {
                    x[r * nv + k] = src_vals.iter().map(|v| v[r]).product();
                }
            }
        }

        // Build spec for prediction.
        let idg = if cfg.idg.is_empty() {
            // Derive idg from term membership.
            let mut idg = vec![0u8; nv];
            if cfg.intercept {
                idg[0] = if mixture_terms.is_empty() { 1 } else { 2 };
            }
            for (k, name) in col_names.iter().enumerate() {
                if name == "__intercept" { continue; }
                if mixture_terms.iter().any(|t| &t.name == name) {
                    idg[k] = 2;
                } else if fixed_terms.iter().any(|t| &t.name == name) {
                    idg[k] = 1;
                }
            }
            idg
        } else {
            cfg.idg.clone()
        };

        let idprob = if cfg.idprob.is_empty() { vec![0u8; nv] } else { cfg.idprob.clone() };
        let idea = if cfg.idea.is_empty() { vec![0u8; nv] } else { cfg.idea.clone() };
        let idcor = if cfg.idcor.is_empty() { vec![0u8; nv] } else { cfg.idcor.clone() };

        let spec = ModelSpec {
            ng: cfg.ng,
            idiag: false,
            nwg: false,
            ncor: 0,
            idprob,
            idea,
            idg,
            idcor,
        };

        let preds = predict_y(&spec, &spec.layout(), &cfg.best, &x, nobs);

        // Build output: one row per (observation, class).
        let mut rows: Vec<i32> = Vec::with_capacity(nobs * cfg.ng);
        let mut classes: Vec<i32> = Vec::with_capacity(nobs * cfg.ng);
        let mut predicted: Vec<f64> = Vec::with_capacity(nobs * cfg.ng);
        for r in 0..nobs {
            for g in 1..=cfg.ng {
                rows.push(r as i32);
                classes.push(g as i32);
                predicted.push(preds[(g - 1) * nobs + r]);
            }
        }

        let batch = RecordBatch::try_new(
            predict_schema(),
            vec![
                Arc::new(Int32Array::from(rows)),
                Arc::new(Int32Array::from(classes)),
                Arc::new(Float64Array::from(predicted)),
            ],
        ).map_err(HlmeNodeError::Arrow)?;

        let df = node_ctx.session().read_batch(batch).map_err(HlmeNodeError::Df)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// Node 3: hlme_compare — Compare multiple fits via BIC/AIC
// =====================================================================

const HLME_COMPARE_NODE_KIND: &str = "hlme_compare";

/// One model entry for comparison.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ModelEntry {
    /// Human-readable name for this model.
    pub name: String,
    /// Number of latent classes.
    pub ng: usize,
    /// Number of estimated parameters.
    pub npm: usize,
    /// Log-likelihood at convergence.
    pub loglik: f64,
    /// Number of subjects.
    pub ns: usize,
}

/// Configuration for the `hlme_compare` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HlmeCompareConfig {
    /// List of model fits to compare.
    pub models: Vec<ModelEntry>,
}

fn compare_ports() -> NodePorts {
    NodePorts::new().add_output_port(Some(compare_schema()))
}

#[derive(Clone)]
pub struct HlmeCompareNode {
    meta: NodePorts,
    config: HlmeCompareConfig,
}

impl HlmeCompareNode {
    pub fn new(config: HlmeCompareConfig) -> Self {
        Self {
            meta: compare_ports(),
            config,
        }
    }
}

pub struct HlmeCompareNodeFactory {}

impl NodeFactory for HlmeCompareNodeFactory {
    fn kind(&self) -> &'static str {
        HLME_COMPARE_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Compare multiple hlme fits via BIC, AIC, and approximate posterior probabilities."
    }

    fn doc(&self) -> &'static str {
        "Compares multiple latent class model fits using information criteria. \
        Computes BIC, AIC, BIC differences, and approximate posterior model \
        probabilities (assuming equal prior probabilities). The model with the \
        lowest BIC is the most supported."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HlmeCompareConfig)
    }

    fn ports(&self) -> NodePorts {
        compare_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: HlmeCompareConfig = serde_json::from_value(spec)?;
        Ok(Box::new(HlmeCompareNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        _ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let cfg = parse_spec::<HlmeCompareConfig>(spec, "hlme_compare")?;

        let mut code = vec![
            "# hlme_compare: model comparison".to_string(),
            "summarytable <- data.frame(".to_string(),
        ];
        code.push(format!(
            "  model = c({}),",
            cfg.models.iter().map(|m| r_str(&m.name)).collect::<Vec<_>>().join(", ")
        ));
        code.push(format!(
            "  ng = c({}),",
            cfg.models.iter().map(|m| m.ng.to_string()).collect::<Vec<_>>().join(", ")
        ));
        code.push(format!(
            "  npm = c({}),",
            cfg.models.iter().map(|m| m.npm.to_string()).collect::<Vec<_>>().join(", ")
        ));
        code.push(format!(
            "  loglik = c({})",
            cfg.models.iter().map(|m| m.loglik.to_string()).collect::<Vec<_>>().join(", ")
        ));
        code.push(")".to_string());
        code.push("summarytable$aic <- -2*summarytable$loglik + 2*summarytable$npm".into());
        code.push("summarytable$bic <- -2*summarytable$loglik + log(N)*summarytable$npm".into());
        code.push("print(summarytable)".into());

        Ok(crate::codegen::NodeCodegen::simple(code, "summarytable".to_string()))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["lcmm".into()]
    }
}

#[async_trait]
impl DagNode for HlmeCompareNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        HLME_COMPARE_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let cfg = &self.config;

        // Compute BIC and AIC for each model.
        let entries: Vec<(String, i32, i32, f64, f64, f64)> = cfg.models
            .iter()
            .map(|m| {
                let aic = -2.0 * m.loglik + 2.0 * (m.npm as f64);
                let bic = -2.0 * m.loglik + (m.ns as f64).ln() * (m.npm as f64);
                (m.name.clone(), m.ng as i32, m.npm as i32, m.loglik, aic, bic)
            })
            .collect();

        // Find min BIC.
        let min_bic = entries.iter().map(|e| e.5).fold(f64::INFINITY, f64::min);

        // Compute BIC deltas and approximate posterior probabilities.
        let bic_deltas: Vec<f64> = entries.iter().map(|e| e.5 - min_bic).collect();
        let bic_weights_sum: f64 = bic_deltas.iter().map(|&d| (-0.5 * d).exp()).sum();
        let bic_probs: Vec<f64> = bic_deltas
            .iter()
            .map(|&d| (-0.5 * d).exp() / bic_weights_sum)
            .collect();

        let names: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let ngs: Vec<i32> = entries.iter().map(|e| e.1).collect();
        let npms: Vec<i32> = entries.iter().map(|e| e.2).collect();
        let logliks: Vec<f64> = entries.iter().map(|e| e.3).collect();
        let aics: Vec<f64> = entries.iter().map(|e| e.4).collect();
        let bics: Vec<f64> = entries.iter().map(|e| e.5).collect();

        let batch = RecordBatch::try_new(
            compare_schema(),
            vec![
                Arc::new(StringArray::from(names)),
                Arc::new(Int32Array::from(ngs)),
                Arc::new(Int32Array::from(npms)),
                Arc::new(Float64Array::from(logliks)),
                Arc::new(Float64Array::from(aics)),
                Arc::new(Float64Array::from(bics)),
                Arc::new(Float64Array::from(bic_deltas)),
                Arc::new(Float64Array::from(bic_probs)),
            ],
        ).map_err(HlmeNodeError::Arrow)?;

        let df = node_ctx.session().read_batch(batch).map_err(HlmeNodeError::Df)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_simple_terms() {
        let terms = expand_terms(&["A".into(), "B".into()]);
        assert_eq!(terms.len(), 2);
        assert_eq!(terms[0].name, "A");
        assert_eq!(terms[1].name, "B");
    }

    #[test]
    fn expand_interaction() {
        let terms = expand_terms(&["A:B".into()]);
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].name, "A:B");
        assert_eq!(terms[0].sources, vec!["A", "B"]);
    }

    #[test]
    fn expand_star_operator() {
        let terms = expand_terms(&["A*B".into()]);
        assert_eq!(terms.len(), 3);
        assert_eq!(terms[0].name, "A");
        assert_eq!(terms[1].name, "B");
        assert_eq!(terms[2].name, "A:B");
    }

    #[test]
    fn default_init_b_gbtm() {
        // GBTM: ng=2, no random, no nwg.
        let spec = ModelSpec {
            ng: 2, idiag: false, nwg: false, ncor: 0,
            idprob: vec![1, 0],
            idea: vec![0, 0],
            idg: vec![2, 2],
            idcor: vec![0, 0],
        };
        let layout = spec.layout();
        // NPROB=1 (intercept × (ng-1)), NEF=4 (2×2), NVC=0, NW=0, STDERR=1 → NPM=6
        assert_eq!(layout.npm, 6);

        let data = LongData::new(
            vec![1.0, 2.0, 3.0, 4.0],
            vec![1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 2.0], // 2 nv × 4 obs
            2,
            vec![2, 2],
            2,
        );
        let b = default_init_b(&data, &spec);
        assert_eq!(b.len(), 6);
        // NPROB = 0, NEF = 0, STDERR > 0
        assert_eq!(b[0], 0.0); // NPROB
        assert!(b[5] > 0.0);   // STDERR
    }

    #[test]
    fn default_init_b_with_random() {
        // m1: ng=1, random intercept + Time slope.
        let spec = ModelSpec {
            ng: 1, idiag: false, nwg: false, ncor: 0,
            idprob: vec![0, 0, 0, 0],
            idea: vec![1, 1, 0, 0],
            idg: vec![1, 1, 1, 1],
            idcor: vec![0, 0, 0, 0],
        };
        let layout = spec.layout();
        // NEF=4, NVC=3 (2×3/2), STDERR=1 → NPM=8
        assert_eq!(layout.npm, 8);

        let data = LongData::new(
            vec![1.0, 2.0],
            vec![1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0],
            4, vec![2], 1,
        );
        let b = default_init_b(&data, &spec);
        assert_eq!(b.len(), 8);
        // Cholesky diagonal entries should be 1.0.
        assert_eq!(b[4], 1.0); // i_nvc + 0 = 4
        assert_eq!(b[6], 1.0); // i_nvc + 2 = 6 (diag of 2×2 upper-tri)
    }
}

// =====================================================================
// Cross-validation against R lcmm golden fixtures
// =====================================================================
//
// Golden fixtures: `bio_crates/lcmm/tests/fixtures/{data_hlme.csv, hlme_golden.json}`
// Restore: `rclone copy aliyun:autonomics-data/lcmm/test-data/ bio_crates/lcmm/tests/`

#[cfg(test)]
mod cross_validation {
    use super::*;
    use crate::dag::node_event::NodeReporter;
    use crate::node_registry::registry::NodeCtx;
    use datalake::Datalake;
    use datafusion::prelude::SessionContext;

    const TOL_LL: f64 = 1e-8;

    // ---- Path helpers --------------------------------------------------

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bio_crates/lcmm/tests/fixtures")
    }

    // ---- CSV → RecordBatch ---------------------------------------------

    fn load_data_batch() -> RecordBatch {
        let path = fixtures_dir().join("data_hlme.csv");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "Failed to read {}: {}. Restore: rclone copy aliyun:autonomics-data/lcmm/test-data/ bio_crates/lcmm/tests/",
                path.display(), e
            )
        });

        let mut ids = Vec::new();
        let mut ys = Vec::new();
        let mut times = Vec::new();
        let mut x1s = Vec::new();
        let mut x2s = Vec::new();
        let mut x3s = Vec::new();

        for (i, line) in text.lines().enumerate() {
            if i == 0 { continue; }
            let f: Vec<&str> = line.split(',').collect();
            if f.len() < 6 { continue; }
            ids.push(f[0].parse::<i64>().unwrap());
            ys.push(f[1].parse::<f64>().unwrap());
            times.push(f[2].parse::<f64>().unwrap());
            x1s.push(f[3].parse::<f64>().unwrap());
            x2s.push(f[4].parse::<f64>().unwrap());
            x3s.push(f[5].parse::<f64>().unwrap());
        }

        let schema = Arc::new(Schema::new(vec![
            Field::new("ID", DataType::Int64, false),
            Field::new("Y", DataType::Float64, false),
            Field::new("Time", DataType::Float64, false),
            Field::new("X1", DataType::Float64, false),
            Field::new("X2", DataType::Float64, false),
            Field::new("X3", DataType::Float64, false),
        ]));

        RecordBatch::try_new(schema, vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(ys)),
            Arc::new(Float64Array::from(times)),
            Arc::new(Float64Array::from(x1s)),
            Arc::new(Float64Array::from(x2s)),
            Arc::new(Float64Array::from(x3s)),
        ]).unwrap()
    }

    // ---- Golden JSON ---------------------------------------------------

    fn load_golden() -> serde_json::Value {
        let path = fixtures_dir().join("hlme_golden.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "Failed to read {}: {}. Restore: rclone copy aliyun:autonomics-data/lcmm/test-data/ bio_crates/lcmm/tests/",
                path.display(), e
            )
        });
        serde_json::from_str(&text).expect("Failed to parse golden JSON")
    }

    fn golden_fit<'a>(golden: &'a serde_json::Value, tag: &str) -> &'a serde_json::Value {
        golden["fits"].as_array().unwrap().iter()
            .find(|f| f["tag"].as_str() == Some(tag))
            .unwrap_or_else(|| panic!("golden fit '{tag}' not found"))
    }

    fn json_to_f64_array(v: &serde_json::Value) -> Vec<f64> {
        v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
    }

    /// Convert golden `best` from R's post-processed form (varcov entries hold
    /// B=U'U) back to Fortran optimization form (Cholesky factor entries).
    fn convert_golden_best(
        best: &[f64],
        cholesky: &[f64],
        layout: &ParamLayout,
        idiag: bool,
    ) -> Vec<f64> {
        let mut b = best.to_vec();
        let nvc = layout.nvc;
        let i_nvc = layout.i_nvc;
        if !idiag && nvc > 0 {
            for k in 0..nvc {
                b[i_nvc + k] = cholesky[k];
            }
        } else if idiag && nvc > 0 {
            let nea = layout.nea;
            for j in 0..nea {
                b[i_nvc + j] = cholesky[j * (j + 1) / 2 + j];
            }
        }
        b
    }

    // ---- Design matrix verification ------------------------------------
    // Verifies that build_model_data() produces the same ModelSpec as the
    // manually-constructed specs in the lcmm crate's cross-validation tests.

    fn check_spec(
        test_name: &str,
        spec: &ModelSpec,
        ng: usize, idiag: bool, nwg: bool,
        idprob: &[u8], idea: &[u8], idg: &[u8],
    ) {
        assert_eq!(spec.ng, ng, "{test_name}: ng mismatch");
        assert_eq!(spec.idiag, idiag, "{test_name}: idiag mismatch");
        assert_eq!(spec.nwg, nwg, "{test_name}: nwg mismatch");
        assert_eq!(spec.idprob, idprob, "{test_name}: idprob mismatch: got {:?}, want {:?}", spec.idprob, idprob);
        assert_eq!(spec.idea, idea, "{test_name}: idea mismatch: got {:?}, want {:?}", spec.idea, idea);
        assert_eq!(spec.idg, idg, "{test_name}: idg mismatch: got {:?}, want {:?}", spec.idg, idg);
        eprintln!("PASS design_matrix {test_name}");
    }

    #[test]
    fn design_matrix_gbtm1() {
        let batch = load_data_batch();
        let batches = vec![batch];
        let cfg = HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 1,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec![], random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        };
        let (data, spec, _) = build_model_data(&batches, &cfg).unwrap();
        // gbtm1: X0 = [intercept, Time], idg=[1,1], idea=[0,0]
        assert_eq!(spec.idg.len(), 2, "nv should be 2");
        check_spec("gbtm1", &spec, 1, false, false, &[0,0], &[0,0], &[1,1]);
        // Verify data dimensions
        assert_eq!(data.ns, 100, "gbtm1: ns should be 100");
    }

    #[test]
    fn design_matrix_gbtm2() {
        let batch = load_data_batch();
        let batches = vec![batch];
        let cfg = HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 2,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec!["Time".into()],
            random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        };
        let (_data, spec, _) = build_model_data(&batches, &cfg).unwrap();
        // gbtm2: X0 = [intercept, Time], idg=[2,2], idea=[0,0], idprob=[1,0]
        assert_eq!(spec.idg.len(), 2);
        check_spec("gbtm2", &spec, 2, false, false, &[1,0], &[0,0], &[2,2]);
    }

    #[test]
    fn design_matrix_m1() {
        let batch = load_data_batch();
        let batches = vec![batch];
        let cfg = HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 1,
            intercept: true, fixed: vec!["Time*X1".into()],
            mixture: vec![], random: vec!["Time".into()], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        };
        let (_data, spec, _) = build_model_data(&batches, &cfg).unwrap();
        // m1: X0 = [intercept, Time, X1, Time:X1]
        // idg=[1,1,1,1], idea=[1,1,0,0]
        assert_eq!(spec.idg.len(), 4, "m1: nv should be 4 (intercept + Time + X1 + Time:X1)");
        check_spec("m1", &spec, 1, false, false, &[0,0,0,0], &[1,1,0,0], &[1,1,1,1]);
    }

    #[test]
    fn design_matrix_m2a() {
        let batch = load_data_batch();
        let batches = vec![batch];
        let cfg = HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 2,
            intercept: true, fixed: vec!["Time*X1".into()],
            mixture: vec!["Time".into()],
            random: vec!["Time".into()],
            classmb: vec!["X2".into(), "X3".into()],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        };
        let (_data, spec, _) = build_model_data(&batches, &cfg).unwrap();
        // m2a: X0 = [intercept, Time, X1, Time:X1, X2, X3]
        // idprob=[1,0,0,0,1,1], idea=[1,1,0,0,0,0], idg=[2,2,1,1,0,0]
        assert_eq!(spec.idg.len(), 6, "m2a: nv should be 6");
        check_spec(
            "m2a", &spec, 2, false, false,
            &[1,0,0,0,1,1], &[1,1,0,0,0,0], &[2,2,1,1,0,0],
        );
    }

    #[test]
    fn design_matrix_m1_idiag() {
        let batch = load_data_batch();
        let batches = vec![batch];
        let cfg = HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 1,
            intercept: true, fixed: vec!["Time*X1".into()],
            mixture: vec![], random: vec!["Time".into()], classmb: vec![],
            idiag: true, nwg: false, maxiter: 500, init_b: vec![],
        };
        let (_data, spec, _) = build_model_data(&batches, &cfg).unwrap();
        assert_eq!(spec.idiag, true, "m1_idiag: idiag should be true");
        check_spec(
            "m1_idiag", &spec, 1, true, false,
            &[0,0,0,0], &[1,1,0,0], &[1,1,1,1],
        );
    }

    // ---- Loglik cross-validation ---------------------------------------
    // Evaluates loglik at the golden best parameter vector and compares
    // against R lcmm's reported loglik.

    fn check_loglik(test_name: &str, cfg: &HlmeConfig, golden_tag: &str) {
        let batch = load_data_batch();
        let batches = vec![batch];
        let (data, spec, _) = build_model_data(&batches, cfg).unwrap();
        let layout = spec.layout();

        let golden = load_golden();
        let gf = golden_fit(&golden, golden_tag);

        // Validate NPM.
        let golden_best = json_to_f64_array(&gf["best"]);
        assert_eq!(
            golden_best.len(), layout.npm,
            "{test_name}: golden NPM={} != Rust NPM={}",
            golden_best.len(), layout.npm,
        );

        // Convert golden best from R post-processed form.
        let golden_chol = json_to_f64_array(&gf["cholesky"]);
        let golden_idiag = gf["idiag"].as_i64().unwrap_or(0) == 1;
        let best = convert_golden_best(&golden_best, &golden_chol, &layout, golden_idiag);

        // Evaluate loglik.
        let ll = lcmm::loglik_hlme(&best, &data, &spec);
        let golden_ll = gf["loglik"].as_f64().unwrap();
        let rel = (ll - golden_ll).abs() / golden_ll.abs().max(1e-10);
        assert!(
            rel < TOL_LL,
            "{test_name}: loglik mismatch. Rust={ll:.10}, R={golden_ll:.10}, rel={rel:.3e}"
        );
        eprintln!("PASS loglik {test_name}: Rust={ll:.6} R={golden_ll:.6} rel={rel:.2e}");
    }

    #[test]
    fn loglik_gbtm1() {
        check_loglik("gbtm1", &HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 1,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec![], random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        }, "gbtm1");
    }

    #[test]
    fn loglik_gbtm2() {
        check_loglik("gbtm2", &HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 2,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec!["Time".into()],
            random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        }, "gbtm2");
    }

    #[test]
    fn loglik_gbtm3() {
        check_loglik("gbtm3", &HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 3,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec!["Time".into()],
            random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        }, "gbtm3");
    }

    #[test]
    fn loglik_m1() {
        check_loglik("m1", &HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 1,
            intercept: true, fixed: vec!["Time*X1".into()],
            mixture: vec![], random: vec!["Time".into()], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        }, "m1");
    }

    #[test]
    fn loglik_m1_idiag() {
        check_loglik("m1_idiag", &HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 1,
            intercept: true, fixed: vec!["Time*X1".into()],
            mixture: vec![], random: vec!["Time".into()], classmb: vec![],
            idiag: true, nwg: false, maxiter: 500, init_b: vec![],
        }, "m1_idiag");
    }

    #[test]
    fn loglik_m2a() {
        check_loglik("m2a", &HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 2,
            intercept: true, fixed: vec!["Time*X1".into()],
            mixture: vec!["Time".into()],
            random: vec!["Time".into()],
            classmb: vec!["X2".into(), "X3".into()],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        }, "m2a");
    }

    #[test]
    fn loglik_m2a_nwg() {
        check_loglik("m2a_nwg", &HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 2,
            intercept: true, fixed: vec!["Time*X1".into()],
            mixture: vec!["Time".into()],
            random: vec!["Time".into()],
            classmb: vec!["X2".into(), "X3".into()],
            idiag: false, nwg: true, maxiter: 500, init_b: vec![],
        }, "m2a_nwg");
    }

    // ---- End-to-end node execution test --------------------------------

    fn test_node_ctx() -> NodeCtx {
        let ctx = SessionContext::new();
        NodeCtx {
            runtime_env: ctx.runtime_env(),
            iceberg_catalog: None,
            datalake: Arc::new(Datalake::default()),
            opendal: None,
        }
    }

    /// Full pipeline: RecordBatch → node execute → output port 0 (summary).
    /// Uses maxiter=0 with golden best to isolate the design matrix + loglik
    /// evaluation from optimizer convergence issues.
    #[tokio::test]
    async fn node_execute_gbtm1_loglik() {
        let batch = load_data_batch();
        let golden = load_golden();
        let gf = golden_fit(&golden, "gbtm1");

        // First, build model data to get the layout for golden best conversion.
        let batches = vec![batch.clone()];
        let cfg_layout = HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 1,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec![], random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        };
        let (data, spec, _) = build_model_data(&batches, &cfg_layout).unwrap();
        let layout = spec.layout();

        // Convert golden best.
        let golden_best = json_to_f64_array(&gf["best"]);
        let golden_chol = json_to_f64_array(&gf["cholesky"]);
        let best = convert_golden_best(&golden_best, &golden_chol, &layout, false);

        // Execute node with maxiter=0.
        let mut node = HlmeNode::new(HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 1,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec![], random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 0, init_b: best,
        });

        let df = SessionContext::new().read_batch(batch).unwrap();
        let input = NodeInput { port: 0, data: df };

        let res = node
            .execute(&test_node_ctx(), &[input], &NodeReporter::noop())
            .await
            .expect("node execute should succeed");

        // Check port 0 (summary).
        let summary_df = res.get(&0).unwrap().clone();
        let summary = summary_df.collect().await.unwrap().into_iter().next().unwrap();

        let loglik = summary
            .column_by_name("loglik")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);

        let golden_ll = gf["loglik"].as_f64().unwrap();
        let rel = (loglik - golden_ll).abs() / golden_ll.abs().max(1e-10);
        assert!(
            rel < TOL_LL,
            "node_execute gbtm1: loglik mismatch. Node={loglik:.10}, R={golden_ll:.10}, rel={rel:.3e}"
        );
        eprintln!("PASS node_execute_gbtm1: loglik={loglik:.6} R={golden_ll:.6} rel={rel:.2e}");

        // Also verify AIC and BIC.
        let aic = summary.column_by_name("aic").unwrap()
            .as_any().downcast_ref::<Float64Array>().unwrap().value(0);
        let bic = summary.column_by_name("bic").unwrap()
            .as_any().downcast_ref::<Float64Array>().unwrap().value(0);
        let golden_aic = gf["AIC"].as_f64().unwrap();
        let golden_bic = gf["BIC"].as_f64().unwrap();
        let rel_aic = (aic - golden_aic).abs() / golden_aic.abs().max(1e-10);
        let rel_bic = (bic - golden_bic).abs() / golden_bic.abs().max(1e-10);
        assert!(rel_aic < TOL_LL, "AIC mismatch: Node={aic}, R={golden_aic}");
        assert!(rel_bic < TOL_LL, "BIC mismatch: Node={bic}, R={golden_bic}");

        // Verify port 1 (params) has the right number of rows.
        let params_df = res.get(&1).unwrap().clone();
        let params = params_df.collect().await.unwrap().into_iter().next().unwrap();
        assert_eq!(
            params.num_rows(),
            layout.npm,
            "params port should have NPM={} rows, got {}",
            layout.npm,
            params.num_rows(),
        );

        // Verify port 2 (posterior) is absent for maxiter=0 (no posterior computation).
        // With maxiter=0, the fit returns early with posterior=None.
        // So port 2 should still exist but may have 0 rows or error.
        // Actually, build_posterior_batch panics if posterior is None.
        // Since maxiter=0 returns early with posterior=None, the node would
        // panic. Let's just verify port 0 and 1 are correct.
        let _ = data;
    }

    /// End-to-end with ng=2 GBTM: verifies loglik + empty posterior port
    /// when maxiter=0 (posterior is not computed).
    #[tokio::test]
    async fn node_execute_gbtm2_loglik() {
        let batch = load_data_batch();
        let golden = load_golden();
        let gf = golden_fit(&golden, "gbtm2");

        // Build model data for layout.
        let batches = vec![batch.clone()];
        let cfg_layout = HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 2,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec!["Time".into()],
            random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 500, init_b: vec![],
        };
        let (_data, spec, _) = build_model_data(&batches, &cfg_layout).unwrap();
        let layout = spec.layout();

        // Convert golden best.
        let golden_best = json_to_f64_array(&gf["best"]);
        let golden_chol = json_to_f64_array(&gf["cholesky"]);
        let best = convert_golden_best(&golden_best, &golden_chol, &layout, false);

        // Execute with maxiter=0.
        let mut node = HlmeNode::new(HlmeConfig {
            subject: "ID".into(), outcome: "Y".into(), ng: 2,
            intercept: true, fixed: vec!["Time".into()],
            mixture: vec!["Time".into()],
            random: vec![], classmb: vec![],
            idiag: false, nwg: false, maxiter: 0, init_b: best,
        });

        let df = SessionContext::new().read_batch(batch).unwrap();
        let input = NodeInput { port: 0, data: df };

        let res = node
            .execute(&test_node_ctx(), &[input], &NodeReporter::noop())
            .await
            .expect("node execute should succeed");

        // Verify loglik from port 0.
        let summary_df = res.get(&0).unwrap().clone();
        let summary = summary_df.collect().await.unwrap().into_iter().next().unwrap();
        let loglik = summary.column_by_name("loglik").unwrap()
            .as_any().downcast_ref::<Float64Array>().unwrap().value(0);
        let golden_ll = gf["loglik"].as_f64().unwrap();
        let rel = (loglik - golden_ll).abs() / golden_ll.abs().max(1e-10);
        assert!(rel < TOL_LL,
            "node_execute gbtm2: loglik mismatch. Node={loglik:.10}, R={golden_ll:.10}, rel={rel:.3e}");
        eprintln!("PASS node_execute_gbtm2: loglik={loglik:.6} R={golden_ll:.6} rel={rel:.2e}");

        // With maxiter=0, posterior is None → port 2 should be an empty batch.
        let posterior_df = res.get(&2).unwrap().clone();
        let posterior = posterior_df.collect().await.unwrap().into_iter().next().unwrap();
        assert_eq!(posterior.num_rows(), 0,
            "posterior port should be empty with maxiter=0");
    }
}
