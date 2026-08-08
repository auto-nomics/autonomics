//! CMAverse-compatible causal mediation analysis — six specialized nodes.
//!
//! Each invocation strategy has its own node kind with a tight schema:
//!
//! | Node kind          | Method | Mediator count | Outcome type |
//! |--------------------|--------|----------------|--------------|
//! | `cmest`            | rb     | single         | continuous   |
//! | `cmest_multi`      | rb     | multiple (K)   | continuous   |
//! | `cmest_binary_y`   | rb     | single         | binary (0/1) |
//! | `cmest_binary_m`   | rb     | single (0/1)   | continuous   |
//! | `cmest_weighting`  | wb     | single         | continuous   |
//! | `cmest_gformula`   | gf     | single         | continuous   |
//!
//! All six share the same output schema (CDE, NDE, NIE, TE, PM, PE plus
//! bootstrap 95% CIs). Multi-mediator returns K rows.

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::arrow_util::{ColumnError, extract_numeric_lenient};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum CmestNodeError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for CmestNodeError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}
impl ::dag_core::dag::NodeError for CmestNodeError {
    fn node_type(&self) -> &str { "cmest" }
}

/// Build the standardized output RecordBatch for any cmest variant.
fn build_cmest_batch(
    out: &epi::cmest::CmestResult,
    n_mediators: usize,
    mediator_names: Option<&[&str]>,
) -> RecordBatch {
    let n_rows = n_mediators.max(1);
    let n = out.n_obs as i32;

    let cde = vec![out.cde; n_rows];
    let nde = vec![out.nde; n_rows];
    let mut ni = vec![out.nie; n_rows];
    let te = vec![out.te; n_rows];
    let mut pm = vec![out.prop_mediated; n_rows];
    let pe = vec![out.prop_eliminated; n_rows];
    let cde_lo = vec![out.cde_ci.0; n_rows];
    let cde_hi = vec![out.cde_ci.1; n_rows];
    let nde_lo = vec![out.nde_ci.0; n_rows];
    let nde_hi = vec![out.nde_ci.1; n_rows];
    let ni_lo = vec![out.nie_ci.0; n_rows];
    let ni_hi = vec![out.nie_ci.1; n_rows];
    let te_lo = vec![out.te_ci.0; n_rows];
    let te_hi = vec![out.te_ci.1; n_rows];
    let n_obs = vec![n; n_rows];
    let mut weights = vec![1.0_f64; n_rows];

    if n_mediators > 1 {
        for i in 0..n_mediators {
            ni[i] = out.nie_per_mediator.get(i).copied().unwrap_or(0.0);
            pm[i] = out
                .prop_mediated_per_mediator
                .get(i)
                .copied()
                .unwrap_or(0.0);
            weights[i] = out.mediator_weights.get(i).copied().unwrap_or(0.0);
        }
    }

    let mediator_col: Vec<String> = match mediator_names {
        Some(names) => names.iter().map(|s| s.to_string()).collect(),
        None => vec!["m0".to_string()],
    };

    let schema = Arc::new(Schema::new(vec![
        Field::new("mediator", DataType::Utf8, false),
        Field::new("cde", DataType::Float64, false),
        Field::new("nde", DataType::Float64, false),
        Field::new("nie", DataType::Float64, false),
        Field::new("te", DataType::Float64, false),
        Field::new("prop_mediated", DataType::Float64, true),
        Field::new("prop_eliminated", DataType::Float64, true),
        Field::new("cde_ci_lower", DataType::Float64, false),
        Field::new("cde_ci_upper", DataType::Float64, false),
        Field::new("nde_ci_lower", DataType::Float64, false),
        Field::new("nde_ci_upper", DataType::Float64, false),
        Field::new("nie_ci_lower", DataType::Float64, false),
        Field::new("nie_ci_upper", DataType::Float64, false),
        Field::new("te_ci_lower", DataType::Float64, false),
        Field::new("te_ci_upper", DataType::Float64, false),
        Field::new("n_obs", DataType::Int32, false),
        Field::new("mediator_weight", DataType::Float64, true),
    ]));



    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(mediator_col)),
            Arc::new(Float64Array::from(cde)),
            Arc::new(Float64Array::from(nde)),
            Arc::new(Float64Array::from(ni)),
            Arc::new(Float64Array::from(te)),
            Arc::new(Float64Array::from(pm)),
            Arc::new(Float64Array::from(pe)),
            Arc::new(Float64Array::from(cde_lo)),
            Arc::new(Float64Array::from(cde_hi)),
            Arc::new(Float64Array::from(nde_lo)),
            Arc::new(Float64Array::from(nde_hi)),
            Arc::new(Float64Array::from(ni_lo)),
            Arc::new(Float64Array::from(ni_hi)),
            Arc::new(Float64Array::from(te_lo)),
            Arc::new(Float64Array::from(te_hi)),
            Arc::new(Int32Array::from(n_obs)),
            Arc::new(Float64Array::from(weights)),
        ],
    )
    .expect("cmest output schema")
}

/// Shared: extract numeric columns + complete-case filter.
fn prepare_inputs(
    batches: &[arrow_array::RecordBatch],
    exposure: &str,
    mediators: &[&str],
    outcome: &str,
    covariates: &[&str],
) -> Result<(Vec<f64>, Vec<Vec<f64>>, Vec<f64>, Vec<Vec<f64>>), CmestNodeError> {
    let x = extract_numeric_lenient(batches, exposure)?;
    let m: Vec<Vec<f64>> = mediators
        .iter()
        .map(|c| extract_numeric_lenient(batches, c))
        .collect::<Result<_, _>>()?;
    let y = extract_numeric_lenient(batches, outcome)?;
    let cov: Vec<Vec<f64>> = covariates
        .iter()
        .map(|c| extract_numeric_lenient(batches, c))
        .collect::<Result<_, _>>()?;

    let n = y.len();
    let mut x_f = Vec::with_capacity(n);
    let mut m_f: Vec<Vec<f64>> = vec![Vec::with_capacity(n); mediators.len()];
    let mut y_f = Vec::with_capacity(n);
    let mut cov_f: Vec<Vec<f64>> = vec![Vec::with_capacity(n); covariates.len()];

    for i in 0..n {
        if x[i].is_nan() || y[i].is_nan() {
            continue;
        }
        let mut skip = false;
        for j in 0..mediators.len() {
            if m[j][i].is_nan() {
                skip = true;
                break;
            }
        }
        if skip {
            continue;
        }
        for j in 0..covariates.len() {
            if cov[j][i].is_nan() {
                skip = true;
                break;
            }
        }
        if skip {
            continue;
        }
        x_f.push(x[i]);
        for j in 0..mediators.len() {
            m_f[j].push(m[j][i]);
        }
        y_f.push(y[i]);
        for j in 0..covariates.len() {
            cov_f[j].push(cov[j][i]);
        }
    }

    if x_f.is_empty() {
        return Err(CmestNodeError::Column("no complete-case rows".to_string()));
    }
    Ok((x_f, m_f, y_f, cov_f))
}

fn default_n_boot() -> usize {
    1000
}
fn default_seed() -> u64 {
    42
}

// ═══════════════════════════════════════════════════════════════════════════
// Node 1: cmest (single continuous mediator, continuous outcome, rb)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CmestSpec {
    pub exposure_column: String,
    pub mediator_column: String,
    pub outcome_column: String,
    #[serde(default)]
    pub covariates: Vec<String>,
    #[serde(default)]
    pub interaction: bool,
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

#[derive(Clone)]
pub struct CmestNode {
    meta: NodePorts,
    spec: CmestSpec,
}
pub struct CmestNodeFactory {}

impl NodeFactory for CmestNodeFactory {
    fn kind(&self) -> &'static str {
        "cmest"
    }
    fn desc(&self) -> &'static str {
        "CMAverse regression-based mediation (single continuous mediator, continuous outcome)."
    }
    fn doc(&self) -> &'static str {
        "regression-based causal mediation (single mediator, continuous outcome). Decomposes the total effect into CDE, NDE, NIE, PM, PE; reports Wald z and bootstrap CI. Matches CMAverse::cmest(model='rb') for standard regression."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CmestSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CmestSpec = serde_json::from_value(spec)?;
        if s.mediator_column == s.exposure_column || s.mediator_column == s.outcome_column {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "cmest".into(),
                reason: "mediator_column must differ from exposure and outcome columns".into(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(CmestSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(CmestNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<CmestSpec>(spec, "cmest")?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let code = vec![
            format!("# CMAverse cmest: regression-based causal mediation"),
            format!("set.seed({})", s.seed),
            format!("{out} <- cmest("),
            format!("  data = {input},"),
            format!("  exposure = \"{}\",", s.exposure_column),
            format!("  mediator = \"{}\",", s.mediator_column),
            format!("  outcome = \"{}\",", s.outcome_column),
            format!("  covariates = c(\"{}\"),", s.covariates.join("\", \"")),
            format!("  yreg = \"linear\", mreg = \"linear\","),
            format!("  estimation = \"imputation\", inference = \"bootstrap\","),
            format!("  nboot = {}", s.n_bootstrap),
            format!(")"),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["CMAverse".into()]
    }
}

#[async_trait]
impl DagNode for CmestNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "cmest"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(CmestNodeError::Column("no input".into()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| CmestNodeError::Collect(e.to_string()))?;
        let cov_strs: Vec<&str> = self.spec.covariates.iter().map(|s| s.as_str()).collect();
        let (x, m_vec, y, cov_owned) = prepare_inputs(
            &batches,
            &self.spec.exposure_column,
            &[&self.spec.mediator_column],
            &self.spec.outcome_column,
            &cov_strs,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "cmest".into(),
            msg: e.to_string(),
        })?;
        let cov: Vec<&[f64]> = cov_owned.iter().map(|v| v.as_slice()).collect();
        let m_ref = m_vec[0].as_slice();
        let opts = epi::cmest::CmestOptions {
            interaction: self.spec.interaction,
            n_bootstrap: self.spec.n_bootstrap,
            seed: self.spec.seed,
            ..Default::default()
        };
        let r = epi::cmest::cmest(&x, m_ref, &y, &cov, &opts)
            .map_err(|e| CmestNodeError::Fit(e.to_string()))?;
        let batch = build_cmest_batch(&r, 1, None);
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| CmestNodeError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Node 2: cmest_multi (multiple mediators, continuous outcome, rb)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CmestMultiSpec {
    pub exposure_column: String,
    pub mediator_columns: Vec<String>,
    pub outcome_column: String,
    #[serde(default)]
    pub covariates: Vec<String>,
    #[serde(default)]
    pub interaction: bool,
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

#[derive(Clone)]
pub struct CmestMultiNode {
    meta: NodePorts,
    spec: CmestMultiSpec,
}
pub struct CmestMultiNodeFactory {}

impl NodeFactory for CmestMultiNodeFactory {
    fn kind(&self) -> &'static str {
        "cmest_multi"
    }
    fn desc(&self) -> &'static str {
        "CMAverse regression-based mediation (multiple mediators)."
    }
    fn doc(&self) -> &'static str {
        "regression-based causal mediation with K mediators (VanderWeele 2014). Returns per-mediator NIE, weight, and proportion mediated. Output has K rows."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CmestMultiSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CmestMultiSpec = serde_json::from_value(spec)?;
        if s.mediator_columns.len() < 2 {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "cmest_multi".into(),
                reason: format!(
                    "mediator_columns must contain ≥ 2 entries, got {}",
                    s.mediator_columns.len()
                ),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(CmestMultiSpec))
                    .unwrap_or_default(),
            });
        }
        for m in &s.mediator_columns {
            if *m == s.exposure_column || *m == s.outcome_column {
                return Err(dag_core::registry::error::Error::SpecRejection {
                    kind: "cmest_multi".into(),
                    reason: format!("mediator '{}' must differ from exposure and outcome", m),
                    schema_pretty: serde_json::to_string_pretty(&schema_for!(CmestMultiSpec))
                        .unwrap_or_default(),
                });
            }
        }
        Ok(Box::new(CmestMultiNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<CmestMultiSpec>(spec, "cmest_multi")?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let med_cols = s
            .mediator_columns
            .iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let code = vec![
            format!("# CMAverse cmest: multiple mediators"),
            format!("set.seed({})", s.seed),
            format!("{out} <- cmest("),
            format!("  data = {input},"),
            format!("  exposure = \"{}\",", s.exposure_column),
            format!("  mediators = c({med_cols}),"),
            format!("  outcome = \"{}\",", s.outcome_column),
            format!("  covariates = c(\"{}\"),", s.covariates.join("\", \"")),
            format!("  yreg = \"linear\", mreg = \"linear\","),
            format!("  estimation = \"imputation\", inference = \"bootstrap\","),
            format!("  nboot = {}", s.n_bootstrap),
            format!(")"),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["CMAverse".into()]
    }
}

#[async_trait]
impl DagNode for CmestMultiNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "cmest_multi"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(CmestNodeError::Column("no input".into()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| CmestNodeError::Collect(e.to_string()))?;
        let med_strs: Vec<&str> = self
            .spec
            .mediator_columns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let cov_strs: Vec<&str> = self.spec.covariates.iter().map(|s| s.as_str()).collect();
        let (x, m_vec, y, cov_owned) = prepare_inputs(
            &batches,
            &self.spec.exposure_column,
            &med_strs,
            &self.spec.outcome_column,
            &cov_strs,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "cmest_multi".into(),
            msg: e.to_string(),
        })?;
        let cov: Vec<&[f64]> = cov_owned.iter().map(|v| v.as_slice()).collect();
        let m_slice: Vec<&[f64]> = m_vec.iter().map(|v| v.as_slice()).collect();
        let opts = epi::cmest::CmestOptions {
            interaction: self.spec.interaction,
            n_bootstrap: self.spec.n_bootstrap,
            seed: self.spec.seed,
            ..Default::default()
        };
        let r = epi::cmest::cmest_multi(&x, &m_slice, &y, &cov, &opts)
            .map_err(|e| CmestNodeError::Fit(e.to_string()))?;
        let k = m_slice.len();
        let names: Vec<&str> = med_strs.clone();
        let batch = build_cmest_batch(&r, k, Some(&names));
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| CmestNodeError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Node 3: cmest_binary_y (binary outcome, continuous mediator, rb OR scale)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CmestBinaryYSpec {
    pub exposure_column: String,
    pub mediator_column: String,
    pub outcome_column: String,
    #[serde(default)]
    pub covariates: Vec<String>,
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

#[derive(Clone)]
pub struct CmestBinaryYNode {
    meta: NodePorts,
    spec: CmestBinaryYSpec,
}
pub struct CmestBinaryYNodeFactory {}

impl NodeFactory for CmestBinaryYNodeFactory {
    fn kind(&self) -> &'static str {
        "cmest_binary_y"
    }
    fn desc(&self) -> &'static str {
        "CMAverse regression-based mediation (binary outcome, OR scale)."
    }
    fn doc(&self) -> &'static str {
        "Binary outcome (0/1) causal mediation. Effects on OR scale: CDE=exp(β₁), NIE=exp(β₂·α₁), TE=CDE×NIE. Matches CMAverse::cmest(model='rb') for logistic+y_linear+linear."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CmestBinaryYSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CmestBinaryYSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CmestBinaryYNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<CmestBinaryYSpec>(spec, "cmest_binary_y")?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let code = vec![
            format!("# CMAverse cmest: binary outcome"),
            format!("set.seed({})", s.seed),
            format!("{out} <- cmest("),
            format!(
                "  data = {input}, exposure = \"{}\", mediator = \"{}\", outcome = \"{}\",",
                s.exposure_column, s.mediator_column, s.outcome_column
            ),
            format!("  yreg = \"logistic\", mreg = \"linear\","),
            format!(
                "  estimation = \"imputation\", inference = \"bootstrap\", nboot = {}",
                s.n_bootstrap
            ),
            format!(")"),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["CMAverse".into()]
    }
}

#[async_trait]
impl DagNode for CmestBinaryYNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "cmest_binary_y"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(CmestNodeError::Column("no input".into()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| CmestNodeError::Collect(e.to_string()))?;
        let cov_strs: Vec<&str> = self.spec.covariates.iter().map(|s| s.as_str()).collect();
        let (x, m_vec, y, cov_owned) = prepare_inputs(
            &batches,
            &self.spec.exposure_column,
            &[&self.spec.mediator_column],
            &self.spec.outcome_column,
            &cov_strs,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "cmest_binary_y".into(),
            msg: e.to_string(),
        })?;
        let cov: Vec<&[f64]> = cov_owned.iter().map(|v| v.as_slice()).collect();
        for &v in &y {
            if v != 0.0 && v != 1.0 {
                return Err(CmestNodeError::Column(format!("outcome must be 0/1, got {v}")).into());
            }
        }
        let m_ref = m_vec[0].as_slice();
        let opts = epi::cmest::CmestOptions {
            interaction: false,
            n_bootstrap: self.spec.n_bootstrap,
            seed: self.spec.seed,
            ..Default::default()
        };
        let r = epi::cmest::cmest_binary_y(&x, m_ref, &y, &cov, &opts)
            .map_err(|e| CmestNodeError::Fit(e.to_string()))?;
        let batch = build_cmest_batch(&r, 1, None);
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| CmestNodeError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Node 4: cmest_binary_m (binary mediator, continuous outcome, rb)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CmestBinaryMSpec {
    pub exposure_column: String,
    pub mediator_column: String,
    pub outcome_column: String,
    #[serde(default)]
    pub covariates: Vec<String>,
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

#[derive(Clone)]
pub struct CmestBinaryMNode {
    meta: NodePorts,
    spec: CmestBinaryMSpec,
}
pub struct CmestBinaryMNodeFactory {}

impl NodeFactory for CmestBinaryMNodeFactory {
    fn kind(&self) -> &'static str {
        "cmest_binary_m"
    }
    fn desc(&self) -> &'static str {
        "CMAverse regression-based mediation (binary mediator, continuous outcome)."
    }
    fn doc(&self) -> &'static str {
        "Binary mediator (0/1) causal mediation. NIE = β₂ · (P(M=1|X=1) − P(M=1|X=0)). Mediator model is logistic, outcome model is OLS."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CmestBinaryMSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CmestBinaryMSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CmestBinaryMNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<CmestBinaryMSpec>(spec, "cmest_binary_m")?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let code = vec![
            format!("# CMAverse cmest: binary mediator"),
            format!("set.seed({})", s.seed),
            format!("{out} <- cmest("),
            format!(
                "  data = {input}, exposure = \"{}\", mediator = \"{}\", outcome = \"{}\",",
                s.exposure_column, s.mediator_column, s.outcome_column
            ),
            format!("  yreg = \"linear\", mreg = \"logistic\","),
            format!(
                "  estimation = \"imputation\", inference = \"bootstrap\", nboot = {}",
                s.n_bootstrap
            ),
            format!(")"),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["CMAverse".into()]
    }
}

#[async_trait]
impl DagNode for CmestBinaryMNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "cmest_binary_m"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(CmestNodeError::Column("no input".into()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| CmestNodeError::Collect(e.to_string()))?;
        let cov_strs: Vec<&str> = self.spec.covariates.iter().map(|s| s.as_str()).collect();
        let (x, m_vec, y, cov_owned) = prepare_inputs(
            &batches,
            &self.spec.exposure_column,
            &[&self.spec.mediator_column],
            &self.spec.outcome_column,
            &cov_strs,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "cmest_binary_m".into(),
            msg: e.to_string(),
        })?;
        let cov: Vec<&[f64]> = cov_owned.iter().map(|v| v.as_slice()).collect();
        let m_ref = m_vec[0].as_slice();
        for &v in m_ref {
            if v != 0.0 && v != 1.0 {
                return Err(
                    CmestNodeError::Column(format!("mediator must be 0/1, got {v}")).into(),
                );
            }
        }
        let opts = epi::cmest::CmestOptions {
            interaction: false,
            n_bootstrap: self.spec.n_bootstrap,
            seed: self.spec.seed,
            ..Default::default()
        };
        let r = epi::cmest::cmest_binary_m(&x, m_ref, &y, &cov, &opts)
            .map_err(|e| CmestNodeError::Fit(e.to_string()))?;
        let batch = build_cmest_batch(&r, 1, None);
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| CmestNodeError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Node 5: cmest_weighting (weighting-based, IPTW)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CmestWeightingSpec {
    pub exposure_column: String,
    pub mediator_column: String,
    pub outcome_column: String,
    pub covariates: Vec<String>,
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

#[derive(Clone)]
pub struct CmestWeightingNode {
    meta: NodePorts,
    spec: CmestWeightingSpec,
}
pub struct CmestWeightingNodeFactory {}

impl NodeFactory for CmestWeightingNodeFactory {
    fn kind(&self) -> &'static str {
        "cmest_weighting"
    }
    fn desc(&self) -> &'static str {
        "CMAverse weighting-based mediation (IPTW)."
    }
    fn doc(&self) -> &'static str {
        "weighting-based (VanderWeele 2014) causal mediation. Uses IPTW regression to estimate NDE and NIE. Requires covariates for the propensity model."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CmestWeightingSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CmestWeightingSpec = serde_json::from_value(spec)?;
        if s.covariates.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "cmest_weighting".into(),
                reason: "covariates must be non-empty for weighting-based method".into(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(CmestWeightingSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(CmestWeightingNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<CmestWeightingSpec>(spec, "cmest_weighting")?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let code = vec![
            format!("# CMAverse cmest: IPW weighting estimation"),
            format!("set.seed({})", s.seed),
            format!("{out} <- cmest("),
            format!(
                "  data = {input}, exposure = \"{}\", mediator = \"{}\", outcome = \"{}\",",
                s.exposure_column, s.mediator_column, s.outcome_column
            ),
            format!("  covariates = c(\"{}\"),", s.covariates.join("\", \"")),
            format!("  yreg = \"linear\", mreg = \"linear\","),
            format!("  estimation = \"imputation\","),
            format!(
                "  weighting = \"IPW\", inference = \"bootstrap\", nboot = {}",
                s.n_bootstrap
            ),
            format!(")"),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["CMAverse".into()]
    }
}

#[async_trait]
impl DagNode for CmestWeightingNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "cmest_weighting"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(CmestNodeError::Column("no input".into()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| CmestNodeError::Collect(e.to_string()))?;
        let cov_strs: Vec<&str> = self.spec.covariates.iter().map(|s| s.as_str()).collect();
        let (x, m_vec, y, cov_owned) = prepare_inputs(
            &batches,
            &self.spec.exposure_column,
            &[&self.spec.mediator_column],
            &self.spec.outcome_column,
            &cov_strs,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "cmest_weighting".into(),
            msg: e.to_string(),
        })?;
        let cov: Vec<&[f64]> = cov_owned.iter().map(|v| v.as_slice()).collect();
        let m_ref = m_vec[0].as_slice();
        let opts = epi::cmest::CmestOptions {
            interaction: false,
            n_bootstrap: self.spec.n_bootstrap,
            seed: self.spec.seed,
            ..Default::default()
        };
        let r = epi::cmest::cmest_weighting(&x, m_ref, &y, &cov, &opts)
            .map_err(|e| CmestNodeError::Fit(e.to_string()))?;
        let batch = build_cmest_batch(&r, 1, None);
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| CmestNodeError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Node 6: cmest_gformula (g-computation)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CmestGformulaSpec {
    pub exposure_column: String,
    pub mediator_column: String,
    pub outcome_column: String,
    pub covariates: Vec<String>,
    #[serde(default = "default_n_boot")]
    pub n_bootstrap: usize,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

#[derive(Clone)]
pub struct CmestGformulaNode {
    meta: NodePorts,
    spec: CmestGformulaSpec,
}
pub struct CmestGformulaNodeFactory {}

impl NodeFactory for CmestGformulaNodeFactory {
    fn kind(&self) -> &'static str {
        "cmest_gformula"
    }
    fn desc(&self) -> &'static str {
        "CMAverse g-formula (parametric g-computation)."
    }
    fn doc(&self) -> &'static str {
        "g-formula (Robins 1986) causal mediation. Requires covariates for the parametric simulation. Estimates NDE/NIE by simulating counterfactual outcomes under X=1 vs X=0."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(CmestGformulaSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: CmestGformulaSpec = serde_json::from_value(spec)?;
        if s.covariates.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "cmest_gformula".into(),
                reason: "covariates must be non-empty for g-formula (need parametric simulation)"
                    .into(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(CmestGformulaSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(CmestGformulaNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<CmestGformulaSpec>(spec, "cmest_gformula")?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let code = vec![
            format!("# CMAverse cmest: G-formula estimation"),
            format!("set.seed({})", s.seed),
            format!("{out} <- cmest("),
            format!(
                "  data = {input}, exposure = \"{}\", mediator = \"{}\", outcome = \"{}\",",
                s.exposure_column, s.mediator_column, s.outcome_column
            ),
            format!("  covariates = c(\"{}\"),", s.covariates.join("\", \"")),
            format!("  yreg = \"linear\", mreg = \"linear\","),
            format!(
                "  estimation = \"paramfunc\", inference = \"bootstrap\", nboot = {}",
                s.n_bootstrap
            ),
            format!(")"),
            format!("print(summary({out}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["CMAverse".into()]
    }
}

#[async_trait]
impl DagNode for CmestGformulaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "cmest_gformula"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or(CmestNodeError::Column("no input".into()))?;
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| CmestNodeError::Collect(e.to_string()))?;
        let cov_strs: Vec<&str> = self.spec.covariates.iter().map(|s| s.as_str()).collect();
        let (x, m_vec, y, cov_owned) = prepare_inputs(
            &batches,
            &self.spec.exposure_column,
            &[&self.spec.mediator_column],
            &self.spec.outcome_column,
            &cov_strs,
        )
        .map_err(|e| DagError::NodeError {
            node_type: "cmest_gformula".into(),
            msg: e.to_string(),
        })?;
        let cov: Vec<&[f64]> = cov_owned.iter().map(|v| v.as_slice()).collect();
        let m_ref = m_vec[0].as_slice();
        let opts = epi::cmest::CmestOptions {
            interaction: false,
            n_bootstrap: self.spec.n_bootstrap,
            seed: self.spec.seed,
            ..Default::default()
        };
        let r = epi::cmest::cmest_gformula(&x, m_ref, &y, &cov, &opts)
            .map_err(|e| CmestNodeError::Fit(e.to_string()))?;
        let batch = build_cmest_batch(&r, 1, None);
        let ctx = node_ctx.session();
        let df = ctx
            .read_batch(batch)
            .map_err(|e| CmestNodeError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
