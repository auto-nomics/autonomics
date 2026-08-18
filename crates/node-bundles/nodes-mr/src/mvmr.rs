//! MVMR node — multivariable Mendelian randomisation.
//!
//! Wraps the pure-Rust [`mvmr`] crate (a faithful port of the R `MVMR` package,
//! Sanderson et al. 2019). The node takes a single upstream `DataFrame` of
//! per-instrument summary statistics — outcome effect + SE and one or more
//! exposure effects + SEs — and runs the requested analyses:
//!
//! * **IVW** — the multivariable inverse-variance-weighted estimator (always).
//! * **Strength** — conditional F-statistics for instrument strength
//!   ([`mvmr::strength_mvmr`] / [`mvmr::strhet_mvmr`]).
//! * **Pleiotropy** — Cochran's Q statistic for instrument validity
//!   ([`mvmr::pleiotropy_mvmr`]).
//! * **qhet** — weak-instrument-adjusted estimator via Q-minimisation
//!   ([`mvmr::qhet_mvmr`]); requires a phenotypic correlation matrix.
//!
//! Output is a long-format table tagged by a `section` column.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum MvmrNodeError {
    #[error("MVMR computation failed: {0}")]
    Mvmr(#[from] mvmr::MvmrError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("column '{name}' is not numeric (got {dtype})")]
    WrongColumnType { name: String, dtype: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
    #[error("column length mismatch for '{name}'")]
    LengthMismatch { name: String },
}

impl ::dag_core::dag::NodeError for MvmrNodeError {
    fn node_type(&self) -> &str {
        MVMR_NODE_KIND
    }
}

// =====================================================================
// Config
// =====================================================================

/// Configuration for the MVMR node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MvmrConfig {
    /// Column name of the outcome instrument-effect (`betaYG`).
    pub beta_yg: String,
    /// Column name of the outcome standard error (`sebetaYG`).
    pub sebeta_yg: String,
    /// Column names of the exposure instrument-effects (`betaX1..betaXp`).
    pub beta_xg: Vec<String>,
    /// Column names of the exposure standard errors (`sebetaX1..sebetaXp`).
    pub sebeta_xg: Vec<String>,
    /// Column name of the per-row instrument label (e.g. SNP rsID). Optional.
    #[serde(default)]
    pub label_column: Option<String>,
    /// Scalar covariance between genetic effects on exposures (`gencov`).
    /// Default 0 (recommended for two-sample summary data).
    #[serde(default)]
    pub gencov: f64,
    /// Run the conditional F-statistic strength test (`strength_mvmr`).
    /// Default true.
    #[serde(default = "default_true")]
    pub strength: bool,
    /// Run the IRLS conditional F-statistic strength test (`strhet_mvmr`).
    /// Default true.
    #[serde(default = "default_true")]
    pub strhet: bool,
    /// Run the pleiotropy / instrument-validity Q test (`pleiotropy_mvmr`).
    /// Default true.
    #[serde(default = "default_true")]
    pub pleiotropy: bool,
    /// Run the weak-instrument-adjusted estimator (`qhet_mvmr`). Requires
    /// `pcor`. Default false.
    #[serde(default = "default_false")]
    pub qhet: bool,
    /// Optional `p × p` phenotypic correlation matrix for `qhet` (row-major,
    /// `p = beta_xg.len()`). Required when `qhet = true`.
    #[serde(default)]
    pub pcor: Vec<Vec<f64>>,
}

fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}

impl Default for MvmrConfig {
    fn default() -> Self {
        Self {
            beta_yg: String::new(),
            sebeta_yg: String::new(),
            beta_xg: vec![],
            sebeta_xg: vec![],
            label_column: None,
            gencov: 0.0,
            strength: true,
            strhet: true,
            pleiotropy: true,
            qhet: false,
            pcor: vec![],
        }
    }
}

// =====================================================================
// Output schema
// =====================================================================

/// Long-format output table:
///
/// * `ivw` — p rows: `exposure`, `estimate`, `se`, `t_stat`, `pvalue`,
///   `sigma`.
/// * `strength` — p rows: `exposure`, `f_statistic`, `q_statistic`.
/// * `strhet` — p rows: `exposure`, `f_statistic`, `q_statistic`.
/// * `pleiotropy` — 1 row: `q_statistic`, `pvalue`, `df`.
/// * `qhet` — p rows: `exposure`, `estimate`, `tau2`.
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("section", DataType::Utf8, false),
        Field::new("exposure", DataType::Utf8, true),
        Field::new("estimate", DataType::Float64, true),
        Field::new("se", DataType::Float64, true),
        Field::new("t_stat", DataType::Float64, true),
        Field::new("pvalue", DataType::Float64, true),
        Field::new("sigma", DataType::Float64, true),
        Field::new("f_statistic", DataType::Float64, true),
        Field::new("q_statistic", DataType::Float64, true),
        Field::new("df", DataType::Float64, true),
        Field::new("tau2", DataType::Float64, true),
    ]))
}

// =====================================================================
// Column extraction
// =====================================================================

fn column_index(batches: &[RecordBatch], name: &str) -> Result<usize, MvmrNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or(MvmrNodeError::EmptyInput)?;
    schema
        .index_of(name)
        .map_err(|_| MvmrNodeError::MissingColumn { name: name.into() })
}

fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, MvmrNodeError> {
    let idx = column_index(batches, name)?;
    let mut out = Vec::new();
    for batch in batches {
        out.extend(numeric_values(batch.column(idx)));
    }
    Ok(out)
}

/// Numeric values from a single column array (null → NaN), across all
/// physical numeric types.
fn numeric_values(col: &dyn Array) -> Vec<f64> {
    let mut out = Vec::with_capacity(col.len());
    macro_rules! cast {
        ($T:ty) => {
            if let Some(a) = col.as_any().downcast_ref::<$T>() {
                for v in a.iter() {
                    out.push(match v {
                        Some(x) => x as f64,
                        None => f64::NAN,
                    });
                }
                return out;
            }
        };
    }
    cast!(arrow_array::Int8Array);
    cast!(arrow_array::Int16Array);
    cast!(arrow_array::Int32Array);
    cast!(arrow_array::Int64Array);
    cast!(arrow_array::UInt8Array);
    cast!(arrow_array::UInt16Array);
    cast!(arrow_array::UInt32Array);
    cast!(arrow_array::UInt64Array);
    cast!(arrow_array::Float32Array);
    cast!(arrow_array::Float64Array);
    for _ in 0..col.len() {
        out.push(f64::NAN);
    }
    out
}

// =====================================================================
// Node
// =====================================================================

const MVMR_NODE_KIND: &str = "mvmr";

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(output_schema()))
}

#[derive(Clone)]
pub struct MvmrNode {
    meta: NodePorts,
    config: MvmrConfig,
}

impl MvmrNode {
    pub fn new(config: MvmrConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

pub struct MvmrNodeFactory {}

impl NodeFactory for MvmrNodeFactory {
    fn kind(&self) -> &'static str {
        MVMR_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "MVMR: multivariable Mendelian randomisation (IVW, strength, pleiotropy, qhet)."
    }

    fn doc(&self) -> &'static str {
        "Multivariable Mendelian randomisation from two-sample summary \
        statistics. Estimates the direct causal effect of each exposure on \
        the outcome conditional on the other exposures, using the IVW \
        estimator. Includes conditional F-statistics for instrument strength, \
        Cochran's Q for instrument validity, and the weak-instrument-adjusted \
        Q-minimisation estimator (Sanderson et al. 2019, IJE)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MvmrConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: MvmrConfig = serde_json::from_value(spec)?;
        Ok(Box::new(MvmrNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let cfg = parse_spec::<MvmrConfig>(spec, "mvmr")?;
        let input = ctx
            .input_vars
            .first()
            .map(|s| s.as_str())
            .unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        let p = cfg.beta_xg.len();

        // Build format_mvmr() call from the upstream data frame.
        let bxgs = cfg
            .beta_xg
            .iter()
            .map(|c| format!("{input}${c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sebxgs = cfg
            .sebeta_xg
            .iter()
            .map(|c| format!("{input}${c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let label = cfg
            .label_column
            .as_deref()
            .map(|c| format!("{input}${c}"))
            .unwrap_or_else(|| format!("seq_len(nrow({input}))"));

        let mvmr_dat = ctx.fresh_var("mvmr_dat");
        let ivw_out = ctx.fresh_var("ivw_res");
        let str_out = ctx.fresh_var("str_res");
        let strhet_out = ctx.fresh_var("strhet_res");
        let pleio_out = ctx.fresh_var("pleio_res");

        let mut code = vec![
            format!("# MVMR: multivariable Mendelian randomisation"),
            format!("library(MVMR)"),
            format!("{mvmr_dat} <- format_mvmr("),
            format!("  BXGs = as.matrix(data.frame({bxgs})),"),
            format!("  BYG = {input}${},", cfg.beta_yg),
            format!("  seBXGs = as.matrix(data.frame({sebxgs})),"),
            format!("  seBYG = {input}${},", cfg.sebeta_yg),
            format!("  RSID = {label}"),
            format!(")"),
            String::new(),
            format!("# IVW multivariable MR"),
            format!("{ivw_out} <- ivw_mvmr({mvmr_dat})"),
            format!("print({ivw_out})"),
        ];

        if cfg.strength {
            code.push(String::new());
            code.push("# Conditional F-statistics (strength_mvmr)".to_string());
            code.push(format!(
                "{str_out} <- strength_mvmr({mvmr_dat}, gencov = {})",
                cfg.gencov
            ));
            code.push(format!("print({str_out})"));
        }

        if cfg.strhet {
            code.push(String::new());
            code.push("# Conditional F-statistics (strhet_mvmr)".to_string());
            code.push(format!(
                "{strhet_out} <- strhet_mvmr({mvmr_dat}, gencov = {})",
                cfg.gencov
            ));
            code.push(format!("print({strhet_out})"));
        }

        if cfg.pleiotropy {
            code.push(String::new());
            code.push("# Instrument validity Q statistic".to_string());
            code.push(format!(
                "{pleio_out} <- pleiotropy_mvmr({mvmr_dat}, gencov = {})",
                cfg.gencov
            ));
        }

        // Build a combined result data frame matching the Rust output schema.
        let mut combine = vec![
            String::new(),
            format!("# Assemble long-format result table"),
            format!("{out} <- data.frame("),
            format!("  section = character(),"),
            format!("  exposure = character(),"),
            format!("  estimate = numeric(),"),
            format!("  se = numeric(),"),
            format!("  t_stat = numeric(),"),
            format!("  pvalue = numeric(),"),
            format!("  sigma = numeric(),"),
            format!("  f_statistic = numeric(),"),
            format!("  q_statistic = numeric(),"),
            format!("  df = numeric(),"),
            format!("  tau2 = numeric()"),
            format!(")"),
        ];

        // IVW rows.
        combine.push(format!(
            "{out} <- rbind({out}, data.frame(section = \"ivw\", exposure = paste0(\"exposure\", 1:{p}), estimate = {ivw_out}[, 1], se = {ivw_out}[, 2], t_stat = {ivw_out}[, 3], pvalue = {ivw_out}[, 4], sigma = NA, f_statistic = NA, q_statistic = NA, df = NA, tau2 = NA))"
        ));

        if cfg.strength {
            combine.push(format!(
                "{out} <- rbind({out}, data.frame(section = \"strength\", exposure = paste0(\"exposure\", 1:{p}), estimate = NA, se = NA, t_stat = NA, pvalue = NA, sigma = NA, f_statistic = as.numeric({str_out}[1, ]), q_statistic = as.numeric({str_out}[1, ]) * nrow({mvmr_dat}), df = NA, tau2 = NA))"
            ));
        }
        if cfg.strhet {
            combine.push(format!(
                "{out} <- rbind({out}, data.frame(section = \"strhet\", exposure = paste0(\"exposure\", 1:{p}), estimate = NA, se = NA, t_stat = NA, pvalue = NA, sigma = NA, f_statistic = as.numeric({strhet_out}[1, ]), q_statistic = as.numeric({strhet_out}[1, ]) * nrow({mvmr_dat}), df = NA, tau2 = NA))"
            ));
        }
        if cfg.pleiotropy {
            combine.push(format!(
                "{out} <- rbind({out}, data.frame(section = \"pleiotropy\", exposure = NA, estimate = NA, se = NA, t_stat = NA, pvalue = {pleio_out}$Qpval, sigma = NA, f_statistic = NA, q_statistic = {pleio_out}$Qstat, df = nrow({mvmr_dat}) - {p} - 1, tau2 = NA))"
            ));
        }
        if cfg.qhet && !cfg.pcor.is_empty() {
            // Serialize pcor as an R matrix literal.
            let rows: Vec<String> = (0..p)
                .map(|i| {
                    let vals: Vec<String> = (0..p).map(|j| format!("{}", cfg.pcor[i][j])).collect();
                    format!("c({})", vals.join(", "))
                })
                .collect();
            let pcor_literal = format!(
                "matrix(c({}), nrow = {p}, ncol = {p}, byrow = TRUE)",
                rows.join(", ")
            );
            combine.push(format!(
                "{{ qhet_res <- qhet_mvmr({mvmr_dat}, pcor = {pcor_literal}, CI = FALSE); {out} <- rbind({out}, data.frame(section = \"qhet\", exposure = paste0(\"exposure\", 1:{p}), estimate = qhet_res[, 1], se = NA, t_stat = NA, pvalue = NA, sigma = NA, f_statistic = NA, q_statistic = NA, df = NA, tau2 = NA)) }}"
            ));
        }

        code.extend(combine);
        code.push(format!("print({out})"));

        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["MVMR".into()]
    }
}

#[async_trait]
impl DagNode for MvmrNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        MVMR_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(MvmrNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> =
            input
                .dataframe()?
                .clone()
                .collect()
                .await
                .map_err(|e| DagError::NodeError {
                    node_type: MVMR_NODE_KIND.into(),
                    msg: format!("collect failed: {e}"),
                })?;
        if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            return Err(MvmrNodeError::EmptyInput.into());
        }

        let p = self.config.beta_xg.len();
        if p == 0 || self.config.sebeta_xg.len() != p {
            return Err(MvmrNodeError::LengthMismatch {
                name: "beta_xg/sebeta_xg".into(),
            }
            .into());
        }

        // Extract outcome + exposures + SEs.
        let beta_yg = extract_f64(&batches, &self.config.beta_yg)?;
        let sebeta_yg = extract_f64(&batches, &self.config.sebeta_yg)?;
        let mut beta_xg = Vec::with_capacity(p);
        let mut sebeta_xg = Vec::with_capacity(p);
        for i in 0..p {
            beta_xg.push(extract_f64(&batches, &self.config.beta_xg[i])?);
            sebeta_xg.push(extract_f64(&batches, &self.config.sebeta_xg[i])?);
        }

        let n = beta_yg.len();
        if sebeta_yg.len() != n {
            return Err(MvmrNodeError::LengthMismatch {
                name: "sebeta_yg".into(),
            }
            .into());
        }
        for (i, v) in beta_xg.iter().enumerate() {
            if v.len() != n || sebeta_xg[i].len() != n {
                return Err(MvmrNodeError::LengthMismatch {
                    name: format!("beta_xg/sebeta_xg[{i}]"),
                }
                .into());
            }
        }

        // Optional SNP column.
        let snp: Vec<String> = if let Some(lbl) = &self.config.label_column {
            let idx = column_index(&batches, lbl)?;
            let dtype = batches[0].schema().field(idx).data_type().clone();
            let mut out = Vec::new();
            for batch in &batches {
                let col = batch.column(idx);
                match dtype {
                    DataType::Utf8 => {
                        for v in col.as_any().downcast_ref::<StringArray>().unwrap().iter() {
                            out.push(v.unwrap_or("").to_string());
                        }
                    }
                    DataType::Utf8View => {
                        for v in col
                            .as_any()
                            .downcast_ref::<arrow_array::StringViewArray>()
                            .unwrap()
                            .iter()
                        {
                            out.push(v.unwrap_or("").to_string());
                        }
                    }
                    _ => {
                        for v in col
                            .as_any()
                            .downcast_ref::<arrow_array::LargeStringArray>()
                            .unwrap()
                            .iter()
                        {
                            out.push(v.unwrap_or("").to_string());
                        }
                    }
                }
            }
            out
        } else {
            Vec::new()
        };

        let mvmr_input = mvmr::MvmrInput {
            beta_yg,
            sebeta_yg,
            beta_xg,
            sebeta_xg,
            snp,
        };

        // Run IVW (always).
        let ivw = mvmr::ivw_mvmr(&mvmr_input).map_err(MvmrNodeError::Mvmr)?;

        // Run optional analyses.
        let strength_res = if self.config.strength {
            Some(
                mvmr::strength_mvmr(&mvmr_input, self.config.gencov)
                    .map_err(MvmrNodeError::Mvmr)?,
            )
        } else {
            None
        };
        let strhet_res = if self.config.strhet {
            Some(mvmr::strhet_mvmr(&mvmr_input, None).map_err(MvmrNodeError::Mvmr)?)
        } else {
            None
        };
        let pleiotropy_res = if self.config.pleiotropy {
            Some(
                mvmr::pleiotropy_mvmr(&mvmr_input, self.config.gencov)
                    .map_err(MvmrNodeError::Mvmr)?,
            )
        } else {
            None
        };
        let qhet_res = if self.config.qhet && !self.config.pcor.is_empty() {
            Some(mvmr::qhet_mvmr(&mvmr_input, &self.config.pcor).map_err(MvmrNodeError::Mvmr)?)
        } else {
            None
        };

        let batch = build_result_batch(
            &ivw,
            strength_res.as_ref(),
            strhet_res.as_ref(),
            pleiotropy_res.as_ref(),
            qhet_res.as_ref(),
        )?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(MvmrNodeError::ReadBatch)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build the long-format output `RecordBatch` from the MVMR results.
#[allow(clippy::too_many_arguments)]
fn build_result_batch(
    ivw: &mvmr::IvwResult,
    strength: Option<&mvmr::StrengthResult>,
    strhet: Option<&mvmr::StrhetResult>,
    pleiotropy: Option<&mvmr::PleiotropyResult>,
    qhet: Option<&mvmr::QhetResult>,
) -> Result<RecordBatch, MvmrNodeError> {
    let mut section: Vec<&str> = Vec::new();
    let mut exposure: Vec<Option<String>> = Vec::new();
    let mut estimate: Vec<Option<f64>> = Vec::new();
    let mut se: Vec<Option<f64>> = Vec::new();
    let mut t_stat: Vec<Option<f64>> = Vec::new();
    let mut pvalue: Vec<Option<f64>> = Vec::new();
    let mut sigma: Vec<Option<f64>> = Vec::new();
    let mut f_statistic: Vec<Option<f64>> = Vec::new();
    let mut q_statistic: Vec<Option<f64>> = Vec::new();
    let mut df: Vec<Option<f64>> = Vec::new();
    let mut tau2: Vec<Option<f64>> = Vec::new();

    // IVW rows.
    for i in 0..ivw.n_exposures() {
        section.push("ivw");
        exposure.push(Some(format!("exposure{}", i + 1)));
        estimate.push(finite(ivw.estimate[i]));
        se.push(finite(ivw.se[i]));
        t_stat.push(finite(ivw.t_stat[i]));
        pvalue.push(finite(ivw.pvalue[i]));
        sigma.push(finite(ivw.sigma));
        f_statistic.push(None);
        q_statistic.push(None);
        df.push(None);
        tau2.push(None);
    }

    // Strength rows.
    if let Some(str_res) = strength {
        for i in 0..str_res.f_statistic.len() {
            section.push("strength");
            exposure.push(Some(format!("exposure{}", i + 1)));
            estimate.push(None);
            se.push(None);
            t_stat.push(None);
            pvalue.push(None);
            sigma.push(None);
            f_statistic.push(finite(str_res.f_statistic[i]));
            q_statistic.push(finite(str_res.q_statistic[i]));
            df.push(None);
            tau2.push(None);
        }
    }

    // Strhet rows.
    if let Some(strhet_res) = strhet {
        for i in 0..strhet_res.f_statistic.len() {
            section.push("strhet");
            exposure.push(Some(format!("exposure{}", i + 1)));
            estimate.push(None);
            se.push(None);
            t_stat.push(None);
            pvalue.push(None);
            sigma.push(None);
            f_statistic.push(finite(strhet_res.f_statistic[i]));
            q_statistic.push(finite(strhet_res.q_statistic[i]));
            df.push(None);
            tau2.push(None);
        }
    }

    // Pleiotropy row.
    if let Some(pleio) = pleiotropy {
        section.push("pleiotropy");
        exposure.push(None);
        estimate.push(None);
        se.push(None);
        t_stat.push(None);
        pvalue.push(finite(pleio.q_pval));
        sigma.push(None);
        f_statistic.push(None);
        q_statistic.push(finite(pleio.q_stat));
        df.push(finite(pleio.df));
        tau2.push(None);
    }

    // Qhet rows.
    if let Some(qhet) = qhet {
        for i in 0..qhet.estimate.len() {
            section.push("qhet");
            exposure.push(Some(format!("exposure{}", i + 1)));
            estimate.push(finite(qhet.estimate[i]));
            se.push(None);
            t_stat.push(None);
            pvalue.push(None);
            sigma.push(None);
            f_statistic.push(None);
            q_statistic.push(None);
            df.push(None);
            tau2.push(finite(qhet.tau2));
        }
    }

    let batch = RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(StringArray::from(section)),
            Arc::new(StringArray::from(exposure)),
            Arc::new(Float64Array::from(estimate)),
            Arc::new(Float64Array::from(se)),
            Arc::new(Float64Array::from(t_stat)),
            Arc::new(Float64Array::from(pvalue)),
            Arc::new(Float64Array::from(sigma)),
            Arc::new(Float64Array::from(f_statistic)),
            Arc::new(Float64Array::from(q_statistic)),
            Arc::new(Float64Array::from(df)),
            Arc::new(Float64Array::from(tau2)),
        ],
    )?;
    Ok(batch)
}

/// Convert an f64 to `Option<f64>`, mapping NaN → None (arrow null).
fn finite(x: f64) -> Option<f64> {
    if x.is_nan() { None } else { Some(x) }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn node_ctx() -> NodeCtx {
        NodeCtx {
            runtime_env: SessionContext::new().runtime_env(),
            opendal: None,
            global_sem: None,
        }
    }

    /// Build a small MVMR input batch: 8 instruments, two exposures.
    fn make_input_batch() -> RecordBatch {
        let snp = StringArray::from(vec!["rs1", "rs2", "rs3", "rs4", "rs5", "rs6", "rs7", "rs8"]);
        let beta_yg = Float64Array::from(vec![0.10, 0.12, 0.09, 0.15, 0.20, 0.14, 0.25, 0.30]);
        let se_yg = Float64Array::from(vec![0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02]);
        let bx1 = Float64Array::from(vec![0.20, 0.22, 0.19, 0.30, 0.40, 0.28, 0.50, 0.60]);
        let sex1 = Float64Array::from(vec![0.03, 0.03, 0.03, 0.03, 0.03, 0.03, 0.03, 0.03]);
        let bx2 = Float64Array::from(vec![0.05, 0.08, 0.01, 0.12, 0.15, 0.09, 0.18, 0.22]);
        let sex2 = Float64Array::from(vec![0.04, 0.04, 0.04, 0.04, 0.04, 0.04, 0.04, 0.04]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("snp", DataType::Utf8, false),
            Field::new("beta_yg", DataType::Float64, false),
            Field::new("se_yg", DataType::Float64, false),
            Field::new("bx1", DataType::Float64, false),
            Field::new("sex1", DataType::Float64, false),
            Field::new("bx2", DataType::Float64, false),
            Field::new("sex2", DataType::Float64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(snp),
                Arc::new(beta_yg),
                Arc::new(se_yg),
                Arc::new(bx1),
                Arc::new(sex1),
                Arc::new(bx2),
                Arc::new(sex2),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn runs_and_emits_all_sections() {
        let mut node = MvmrNode::new(MvmrConfig {
            beta_yg: "beta_yg".into(),
            sebeta_yg: "se_yg".into(),
            beta_xg: vec!["bx1".into(), "bx2".into()],
            sebeta_xg: vec!["sex1".into(), "sex2".into()],
            label_column: Some("snp".into()),
            gencov: 0.0,
            strength: true,
            strhet: true,
            pleiotropy: true,
            qhet: false,
            pcor: vec![],
        });

        let batch = make_input_batch();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();
        let input = NodeInput::new_dataframe(0, df);

        let res = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let outputs = res.dataframe(0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();
        assert!(batch.num_rows() >= 1);

        let section = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let mut has_ivw = false;
        let mut has_strength = false;
        let mut has_pleiotropy = false;
        for i in 0..section.len() {
            match section.value(i) {
                "ivw" => has_ivw = true,
                "strength" => has_strength = true,
                "pleiotropy" => has_pleiotropy = true,
                _ => {}
            }
        }
        assert!(has_ivw, "missing ivw section");
        assert!(has_strength, "missing strength section");
        assert!(has_pleiotropy, "missing pleiotropy section");
    }

    #[tokio::test]
    async fn runs_qhet_with_pcor() {
        let mut node = MvmrNode::new(MvmrConfig {
            beta_yg: "beta_yg".into(),
            sebeta_yg: "se_yg".into(),
            beta_xg: vec!["bx1".into(), "bx2".into()],
            sebeta_xg: vec!["sex1".into(), "sex2".into()],
            label_column: Some("snp".into()),
            gencov: 0.0,
            strength: false,
            strhet: false,
            pleiotropy: false,
            qhet: true,
            pcor: vec![vec![1.0, 0.3], vec![0.3, 1.0]],
        });

        let batch = make_input_batch();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();
        let input = NodeInput::new_dataframe(0, df);

        let res = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let outputs = res.dataframe(0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();

        let section = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let mut has_qhet = false;
        for i in 0..section.len() {
            if section.value(i) == "qhet" {
                has_qhet = true;
            }
        }
        assert!(has_qhet, "missing qhet section");
    }
}
