//! Bayesian Kernel Machine Regression DAG node (`bkmr`).
//!
//! Wraps the pure-Rust [`bkmr`] crate (a faithful port of the R `bkmr`
//! package, Bobb et al. 2015). Fits a Gaussian-process exposure-response
//! surface `h(Z)` with component-wise spike-and-slab variable selection via
//! MCMC, estimating the health effects of multi-pollutant mixtures.
//!
//! The node accepts a single upstream `DataFrame` containing:
//! - one outcome column (continuous, gaussian family),
//! - one or more exposure columns (`Z`),
//! - optional covariate columns (`X`).
//!
//! Output is a long-format table with posterior summaries for each parameter
//! (beta, sigsq.eps, lambda, r_m) and posterior inclusion probabilities
//! (PIPs) for each exposure.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
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
// Error
// =====================================================================

#[derive(Debug, Error)]
pub enum BkmrNodeError {
    #[error("bkmr computation failed: {0}")]
    Bkmr(#[from] bkmr::BkmrError),
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("datafusion error: {0}")]
    Df(#[from] datafusion::error::DataFusionError),
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
}

impl From<BkmrNodeError> for DagError {
    fn from(e: BkmrNodeError) -> Self {
        DagError::NodeError {
            node_type: BKMR_NODE_KIND.to_string(),
            msg: e.to_string(),
        }
    }
}

// =====================================================================
// Config
// =====================================================================

/// Configuration for the BKMR node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BkmrConfig {
    /// Name of the outcome column (continuous).
    pub outcome: String,
    /// Names of the exposure columns to include in `h(Z)`.
    pub exposures: Vec<String>,
    /// Names of covariate columns (optional).
    #[serde(default)]
    pub covariates: Vec<String>,
    /// Number of MCMC iterations. Default 1000.
    #[serde(default = "default_iter")]
    pub iter: usize,
    /// Whether to perform variable selection on exposures. Default true.
    #[serde(default = "default_true")]
    pub varsel: bool,
    /// Prior for the kernel range parameters: "gamma", "invunif", or "unif".
    /// Default "invunif".
    #[serde(default = "default_r_prior")]
    pub r_prior: String,
    /// Random seed for reproducible MCMC.
    #[serde(default = "default_seed")]
    pub seed: u32,
}

fn default_iter() -> usize { 1000 }
fn default_true() -> bool { true }
fn default_r_prior() -> String { "invunif".into() }
fn default_seed() -> u32 { 111 }

// =====================================================================
// Output schema
// =====================================================================

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("section", DataType::Utf8, false),
        Field::new("parameter", DataType::Utf8, true),
        Field::new("mean", DataType::Float64, true),
        Field::new("sd", DataType::Float64, true),
        Field::new("q025", DataType::Float64, true),
        Field::new("q50", DataType::Float64, true),
        Field::new("q975", DataType::Float64, true),
        Field::new("pip", DataType::Float64, true),
    ]))
}

// =====================================================================
// Column extraction helpers
// =====================================================================

fn numeric_values(col: &dyn Array) -> Vec<f64> {
    let mut out = Vec::with_capacity(col.len());
    macro_rules! cast {
        ($T:ty) => {
            if let Some(a) = col.as_any().downcast_ref::<$T>() {
                for v in a.iter() {
                    out.push(match v { Some(x) => x as f64, None => f64::NAN });
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

fn extract_col(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, BkmrNodeError> {
    let schema = batches
        .first()
        .ok_or(BkmrNodeError::EmptyInput)?
        .schema();
    let idx = schema
        .index_of(name)
        .map_err(|_| BkmrNodeError::MissingColumn { name: name.into() })?;
    let mut out = Vec::new();
    for batch in batches {
        out.extend(numeric_values(batch.column(idx)));
    }
    Ok(out)
}

// =====================================================================
// Posterior summary helpers
// =====================================================================

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let n = sorted.len();
    let pos = q * (n - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        sorted[lo.min(n - 1)]
    } else {
        let frac = pos - lo as f64;
        sorted[lo] * (1.0 - frac) + sorted[hi] * frac
    }
}

fn summarize(samples: &[f64]) -> (f64, f64, f64, f64, f64) {
    let n = samples.len();
    let mean = samples.iter().sum::<f64>() / n as f64;
    let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    let sd = var.sqrt();
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q025 = quantile(&sorted, 0.025);
    let q50 = quantile(&sorted, 0.5);
    let q975 = quantile(&sorted, 0.975);
    (mean, sd, q025, q50, q975)
}

// =====================================================================
// Node
// =====================================================================

const BKMR_NODE_KIND: &str = "bkmr";

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(output_schema()))
}

#[derive(Clone)]
pub struct BkmrNode {
    meta: NodePorts,
    config: BkmrConfig,
}

impl BkmrNode {
    pub fn new(config: BkmrConfig) -> Self {
        Self {
            meta: port_layout(),
            config,
        }
    }
}

pub struct BkmrNodeFactory {}

impl NodeFactory for BkmrNodeFactory {
    fn kind(&self) -> &'static str {
        BKMR_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Bayesian Kernel Machine Regression (bkmr): GP exposure-response surface + variable selection."
    }

    fn doc(&self) -> &'static str {
        "Bayesian Kernel Machine Regression (Bobb et al. 2015, Biostatistics). \
        Fits a Gaussian-process exposure-response function h(Z) with \
        spike-and-slab variable selection via MCMC. Estimates the joint \
        health effects of multi-pollutant mixtures, producing posterior \
        means, credible intervals, and posterior inclusion probabilities (PIPs)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BkmrConfig)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: BkmrConfig = serde_json::from_value(spec)?;
        Ok(Box::new(BkmrNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let cfg = parse_spec::<BkmrConfig>(spec, "bkmr")?;
        let input = ctx.input_vars.first().map(|s| s.as_str()).unwrap_or("__missing_input");
        let out = ctx.output_var.to_string();

        let z_cols = cfg.exposures.iter()
            .map(|c| format!("{input}${c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let x_part = if cfg.covariates.is_empty() {
            "NULL".to_string()
        } else {
            let x_cols = cfg.covariates.iter()
                .map(|c| format!("{input}${c}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("cbind({x_cols})")
        };

        let code = vec![
            "# bkmr: Bayesian Kernel Machine Regression".to_string(),
            "library(bkmr)".to_string(),
            format!("set.seed({})", cfg.seed),
            format!("{out} <- kmbayes("),
            format!("  y = {input}${},", r_str(&cfg.outcome)),
            format!("  Z = cbind({z_cols}),"),
            format!("  X = {x_part},"),
            format!("  iter = {},", cfg.iter),
            format!("  varsel = {},", cfg.varsel),
            format!("  verbose = FALSE,"),
            format!("  control.params = list(r.prior = {})", r_str(&cfg.r_prior)),
            ")".to_string(),
            format!("print(summary({out}))"),
        ];

        Ok(crate::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["bkmr".into()]
    }
}

#[async_trait]
impl DagNode for BkmrNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        BKMR_NODE_KIND
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
        let input = inputs.first().ok_or(BkmrNodeError::EmptyInput)?;
        let batches: Vec<RecordBatch> = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: BKMR_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;
        if batches.is_empty() || batches.iter().map(|b| b.num_rows()).sum::<usize>() == 0 {
            return Err(BkmrNodeError::EmptyInput.into());
        }

        let cfg = &self.config;

        // Extract data
        let y = extract_col(&batches, &cfg.outcome)?;
        let m = cfg.exposures.len();
        let n = y.len();
        let mut z = faer::Mat::zeros(n, m);
        for (j, col) in cfg.exposures.iter().enumerate() {
            let vals = extract_col(&batches, col)?;
            for i in 0..n {
                z[(i, j)] = vals[i];
            }
        }
        let k = cfg.covariates.len();
        // When no covariates: X has 0 columns (beta update skipped, mirroring R's
        // `missingX` path where beta.update is not called).
        let mut x = faer::Mat::zeros(n, k);
        for (j, col) in cfg.covariates.iter().enumerate() {
            let vals = extract_col(&batches, col)?;
            for i in 0..n {
                x[(i, j)] = vals[i];
            }
        }

        let r_prior: bkmr::RPrior = cfg.r_prior.parse().unwrap_or(bkmr::RPrior::Invunif);
        let opts = bkmr::KmbayesOptions {
            iter: cfg.iter,
            varsel: cfg.varsel,
            rmethod: bkmr::RMethod::Varying,
            r_prior,
            starting_values: bkmr::StartingValues {
                beta: Some(vec![0.0; k.max(1)]),
                sigsq_eps: Some(0.5),
                r: Some(vec![1.0; m]),
                lambda: Some(vec![10.0]),
                delta: Some(vec![1.0; m]),
                h_hat: Some(vec![1.0; n]),
            },
            control_params: bkmr::ControlParams {
                r_prior,
                ..bkmr::ControlParams::default()
            },
            ztest: None,
        };

        let mut rng = bkmr::Rng::new(cfg.seed);
        let fit =
            bkmr::kmbayes(&mut rng, &y, z.as_ref(), x.as_ref(), &opts).map_err(BkmrNodeError::Bkmr)?;

        let batch = build_result_batch(&fit, m, k)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(BkmrNodeError::Df)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Build the summary output batch from the MCMC chain.
fn build_result_batch(fit: &bkmr::BkmrFit, m: usize, k: usize) -> Result<RecordBatch, BkmrNodeError> {
    // Use second half of the chain for posterior summaries (burn-in = first half)
    let burn = fit.iter / 2;
    let sel = burn..fit.iter;

    let mut sections: Vec<String> = Vec::new();
    let mut params: Vec<Option<String>> = Vec::new();
    let mut means: Vec<Option<f64>> = Vec::new();
    let mut sds: Vec<Option<f64>> = Vec::new();
    let mut q025s: Vec<Option<f64>> = Vec::new();
    let mut q50s: Vec<Option<f64>> = Vec::new();
    let mut q975s: Vec<Option<f64>> = Vec::new();
    let mut pips: Vec<Option<f64>> = Vec::new();

    // beta coefficients (0 rows if no covariates)
    for j in 0..k {
        let samples: Vec<f64> = sel.clone().map(|s| fit.beta[(s, j)]).collect();
        let (mean, sd, q025, q50, q975) = summarize(&samples);
        sections.push("beta".into());
        params.push(Some(format!("beta{}", j + 1)));
        means.push(Some(mean));
        sds.push(Some(sd));
        q025s.push(Some(q025));
        q50s.push(Some(q50));
        q975s.push(Some(q975));
        pips.push(None);
    }

    // sigsq.eps
    {
        let samples: Vec<f64> = sel.clone().map(|s| fit.sigsq_eps[s]).collect();
        let (mean, sd, q025, q50, q975) = summarize(&samples);
        sections.push("sigsq.eps".into());
        params.push(Some("sigsq.eps".into()));
        means.push(Some(mean));
        sds.push(Some(sd));
        q025s.push(Some(q025));
        q50s.push(Some(q50));
        q975s.push(Some(q975));
        pips.push(None);
    }

    // lambda
    {
        let samples: Vec<f64> = sel.clone().map(|s| fit.lambda[(s, 0)]).collect();
        let (mean, sd, q025, q50, q975) = summarize(&samples);
        sections.push("lambda".into());
        params.push(Some("lambda".into()));
        means.push(Some(mean));
        sds.push(Some(sd));
        q025s.push(Some(q025));
        q50s.push(Some(q50));
        q975s.push(Some(q975));
        pips.push(None);
    }

    // r_m and PIPs
    for j in 0..m {
        let samples: Vec<f64> = sel.clone().map(|s| fit.r[(s, j)]).collect();
        let (mean, sd, q025, q50, q975) = summarize(&samples);
        sections.push("r".into());
        params.push(Some(format!("r{}", j + 1)));
        means.push(Some(mean));
        sds.push(Some(sd));
        q025s.push(Some(q025));
        q50s.push(Some(q50));
        q975s.push(Some(q975));
        pips.push(None);
    }

    // PIPs (only if varsel)
    if fit.varsel {
        for j in 0..m {
            let pip: f64 = sel.clone().map(|s| fit.delta[(s, j)]).sum::<f64>() / sel.len() as f64;
            sections.push("pip".into());
            params.push(Some(format!("z{}", j + 1)));
            means.push(None);
            sds.push(None);
            q025s.push(None);
            q50s.push(None);
            q975s.push(None);
            pips.push(Some(pip));
        }
    }

    let batch = RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(StringArray::from(sections)),
            Arc::new(StringArray::from(params)),
            Arc::new(Float64Array::from(means)),
            Arc::new(Float64Array::from(sds)),
            Arc::new(Float64Array::from(q025s)),
            Arc::new(Float64Array::from(q50s)),
            Arc::new(Float64Array::from(q975s)),
            Arc::new(Float64Array::from(pips)),
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
    use datafusion::prelude::SessionContext;

    fn node_ctx() -> NodeCtx {
        NodeCtx {
            runtime_env: SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }

    fn make_input_batch() -> RecordBatch {
        // Small synthetic dataset: 30 observations, 3 exposures, 1 covariate.
        let n = 30;
        let mut rng = bkmr::Rng::new(42);
        let dat = bkmr::sim_data(&mut rng, n, 3, 0.5, 2.0, 1, "norm", &[1]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("y", DataType::Float64, false),
            Field::new("z1", DataType::Float64, false),
            Field::new("z2", DataType::Float64, false),
            Field::new("z3", DataType::Float64, false),
            Field::new("x1", DataType::Float64, false),
        ]));
        let mut z1 = vec![0.0f64; n];
        let mut z2 = vec![0.0f64; n];
        let mut z3 = vec![0.0f64; n];
        for i in 0..n {
            z1[i] = dat.z[(i, 0)];
            z2[i] = dat.z[(i, 1)];
            z3[i] = dat.z[(i, 2)];
        }
        let x1: Vec<f64> = (0..n).map(|i| dat.x[(i, 0)]).collect();
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(dat.y)),
                Arc::new(Float64Array::from(z1)),
                Arc::new(Float64Array::from(z2)),
                Arc::new(Float64Array::from(z3)),
                Arc::new(Float64Array::from(x1)),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn runs_bkmr_node_varsel() {
        let mut node = BkmrNode::new(BkmrConfig {
            outcome: "y".into(),
            exposures: vec!["z1".into(), "z2".into(), "z3".into()],
            covariates: vec!["x1".into()],
            iter: 50,
            varsel: true,
            r_prior: "invunif".into(),
            seed: 42,
        });

        let batch = make_input_batch();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();
        let input = NodeInput { port: 0, data: df };

        let res = node
            .execute(
                &node_ctx(),
                &[input],
                &crate::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let outputs = res.get(&0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();
        // 4 beta (1 covariate + sigsq + lambda) = actually nk=1, plus sigsq, lambda, 3 r, 3 PIPs = 9
        assert_eq!(batch.num_rows(), 9);
    }

    #[tokio::test]
    async fn runs_bkmr_node_novarsel() {
        let mut node = BkmrNode::new(BkmrConfig {
            outcome: "y".into(),
            exposures: vec!["z1".into(), "z2".into(), "z3".into()],
            covariates: vec![],
            iter: 50,
            varsel: false,
            r_prior: "gamma".into(),
            seed: 42,
        });

        let batch = make_input_batch();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();
        let input = NodeInput { port: 0, data: df };

        let res = node
            .execute(
                &node_ctx(),
                &[input],
                &crate::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let outputs = res.get(&0).unwrap().clone();
        let batch = outputs.collect().await.unwrap().into_iter().next().unwrap();
        // X is empty: sigsq.eps, lambda, r1, r2, r3 = 5 rows (no PIPs)
        assert_eq!(batch.num_rows(), 5);
    }
}
