//! Confirmatory factor analysis (SEM) node.
//!
//! Wraps [`epi::sem::cfa`]. Estimates a CFA model by maximum likelihood
//! over indicator columns and emits free-parameter estimates plus fit
//! indices.

use std::sync::Arc;

use arrow_array::{BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use dag_core::arrow_util::{ColumnError, extract_numeric_lenient};
use dag_core::dag::DagError;
use dag_core::dag::graph::PortOutputs;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum EpiCfaError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Computation(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiCfaError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl dag_core::dag::NodeError for EpiCfaError {
    fn node_type(&self) -> &str {
        "epi_cfa"
    }
}

/// One loading entry: which indicator loads on which factor, optionally
/// fixed (e.g. `1.0` to identify the scale).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiCfaLoading {
    /// 0-based indicator index (position in `indicator_columns`).
    pub indicator: usize,
    /// 0-based latent factor index.
    pub factor: usize,
    /// Fix the loading to this value; omit to estimate freely.
    #[serde(default)]
    pub fixed: Option<f64>,
}

/// Spec for [`EpiCfaNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiCfaSpec {
    /// Observed indicator columns (order defines indicator indices).
    pub indicator_columns: Vec<String>,
    /// Loading specification: at least one loading per factor, each
    /// indicator–factor pair at most once.
    pub loadings: Vec<EpiCfaLoading>,
    /// Off-diagonal factor covariance pairs to freely estimate, e.g.
    /// `[[0, 1]]`.
    #[serde(default)]
    pub factor_covariances: Vec<Vec<usize>>,
}

/// Node estimating a CFA model by maximum likelihood.
#[derive(Clone)]
pub struct EpiCfaNode {
    meta: NodePorts,
    spec: EpiCfaSpec,
}

pub struct EpiCfaNodeFactory;

impl NodeFactory for EpiCfaNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_cfa"
    }

    fn desc(&self) -> &'static str {
        "Confirmatory factor analysis (SEM): ML loadings, covariances, fit indices."
    }

    fn doc(&self) -> &'static str {
        "Fits a confirmatory factor model over `indicator_columns` by \
         maximum likelihood. `loadings` declares the measurement model — \
         each entry is `{indicator, factor, fixed?}` (fix one loading per \
         factor, typically 1.0, to identify the scale); \
         `factor_covariances` lists free off-diagonal factor covariance \
         pairs like `[[0, 1]]`. Rows with any missing indicator drop.\n\n\
         Port 0 — free-parameter estimates (long): `estimate_type` \
         (`loading` | `covariance` | `variance`), `label`, `est`, `se`, \
         `p`.\n\
         Port 1 — fit indices, one row: `f_min, chisq, df, chisq_p, cfi, \
         rmsea, srmr, aic, bic, n_obs, n_free, converged`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiCfaSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None)
            .add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EpiCfaSpec = serde_json::from_value(spec)?;
        if spec.indicator_columns.is_empty() || spec.loadings.is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "epi_cfa requires indicator_columns and at least one loading".into(),
            ));
        }
        Ok(Box::new(EpiCfaNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(None)
                .add_output_port(None),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EpiCfaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "epi_cfa"
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
        let input = inputs
            .first()
            .ok_or_else(|| EpiCfaError::Column("no input connected".into()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| EpiCfaError::Collect(e.to_string()))?;

        let n_indicators = self.spec.indicator_columns.len();
        let cols: Vec<Vec<f64>> = self
            .spec
            .indicator_columns
            .iter()
            .map(|col| extract_numeric_lenient(&batches, col))
            .collect::<Result<_, _>>()?;
        let n_rows = cols.first().map(Vec::len).unwrap_or(0);

        // Complete-case rows over all indicators.
        let mut data: Vec<Vec<f64>> = Vec::new();
        for i in 0..n_rows {
            let mut row = Vec::with_capacity(n_indicators);
            let mut complete = true;
            for col in &cols {
                let v = col[i];
                if !v.is_nan() {
                    row.push(v);
                } else {
                    complete = false;
                    break;
                }
            }
            if complete {
                data.push(row);
            }
        }
        if data.is_empty() {
            return Err(EpiCfaError::Column("no complete-case rows".into()).into());
        }

        let n_factors = self
            .spec
            .loadings
            .iter()
            .map(|l| l.factor + 1)
            .max()
            .unwrap_or(0);
        let spec = epi::sem::CfaSpec {
            loadings: self
                .spec
                .loadings
                .iter()
                .map(|l| epi::sem::LoadingSpec {
                    indicator: l.indicator,
                    factor: l.factor,
                    fixed: l.fixed,
                })
                .collect(),
            factor_covariances: self
                .spec
                .factor_covariances
                .iter()
                .filter_map(|pair| {
                    if pair.len() == 2 {
                        Some((pair[0], pair[1]))
                    } else {
                        None
                    }
                })
                .collect(),
            n_indicators,
            n_factors,
        };
        let result =
            epi::sem::cfa(&data, &spec).map_err(|e| EpiCfaError::Computation(e.to_string()))?;

        let session = node_ctx.session();
        let mut outputs = PortOutputs::new();

        // Port 0: free-parameter estimates, stacked by type.
        let mut est_type: Vec<Option<String>> = Vec::new();
        let mut label: Vec<Option<String>> = Vec::new();
        let mut est: Vec<Option<f64>> = Vec::new();
        let mut se: Vec<Option<f64>> = Vec::new();
        let mut p: Vec<Option<f64>> = Vec::new();
        for (lbl, e, s, pv) in &result.loading_estimates {
            est_type.push(Some("loading".into()));
            label.push(Some(lbl.clone()));
            est.push(Some(*e));
            se.push(Some(*s));
            p.push(Some(*pv));
        }
        for (lbl, e, s, pv) in &result.covariance_estimates {
            est_type.push(Some("covariance".into()));
            label.push(Some(lbl.clone()));
            est.push(Some(*e));
            se.push(Some(*s));
            p.push(Some(*pv));
        }
        for (lbl, e, s, pv) in &result.variance_estimates {
            est_type.push(Some("variance".into()));
            label.push(Some(lbl.clone()));
            est.push(Some(*e));
            se.push(Some(*s));
            p.push(Some(*pv));
        }
        let estimates_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("estimate_type", DataType::Utf8, true),
                Field::new("label", DataType::Utf8, true),
                Field::new("est", DataType::Float64, true),
                Field::new("se", DataType::Float64, true),
                Field::new("p", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(
                    est_type.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    label.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(est)),
                Arc::new(Float64Array::from(se)),
                Arc::new(Float64Array::from(p)),
            ],
        )
        .map_err(|e| EpiCfaError::Computation(format!("estimates batch: {e}")))?;
        outputs.insert(
            0,
            session
                .read_batch(estimates_batch)
                .map_err(|e| EpiCfaError::ReadBatch(e.to_string()))?,
        );

        // Port 1: fit indices.
        let fit_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("f_min", DataType::Float64, true),
                Field::new("chisq", DataType::Float64, true),
                Field::new("df", DataType::Int64, true),
                Field::new("chisq_p", DataType::Float64, true),
                Field::new("cfi", DataType::Float64, true),
                Field::new("rmsea", DataType::Float64, true),
                Field::new("srmr", DataType::Float64, true),
                Field::new("aic", DataType::Float64, true),
                Field::new("bic", DataType::Float64, true),
                Field::new("n_obs", DataType::Int64, true),
                Field::new("n_free", DataType::Int64, true),
                Field::new("converged", DataType::Boolean, true),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![result.f_min])),
                Arc::new(Float64Array::from(vec![result.chisq])),
                Arc::new(Int64Array::from(vec![result.df as i64])),
                Arc::new(Float64Array::from(vec![result.chisq_p])),
                Arc::new(Float64Array::from(vec![result.cfi])),
                Arc::new(Float64Array::from(vec![result.rmsea])),
                Arc::new(Float64Array::from(vec![result.srmr])),
                Arc::new(Float64Array::from(vec![result.aic])),
                Arc::new(Float64Array::from(vec![result.bic])),
                Arc::new(Int64Array::from(vec![result.n_obs as i64])),
                Arc::new(Int64Array::from(vec![result.n_free as i64])),
                Arc::new(BooleanArray::from(vec![result.converged])),
            ],
        )
        .map_err(|e| EpiCfaError::Computation(format!("fit batch: {e}")))?;
        outputs.insert(
            1,
            session
                .read_batch(fit_batch)
                .map_err(|e| EpiCfaError::ReadBatch(e.to_string()))?,
        );

        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    /// Two correlated latent factors with three indicators each.
    fn cfa_input() -> Vec<NodeInput> {
        let mut cols: Vec<Vec<Option<f64>>> = vec![Vec::new(); 6];
        let mut seed = 12345_u64;
        let mut rand = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as f64 / (u64::MAX >> 33) as f64
        };
        for _ in 0..200 {
            let f0 = rand();
            let f1 = 0.5 * f0 + 0.5 * rand();
            for (j, col) in cols.iter_mut().enumerate() {
                let factor = if j < 3 { f0 } else { f1 };
                col.push(Some(4.0 * factor + 0.8 * (rand() - 0.5)));
            }
        }
        let schema = Arc::new(Schema::new(
            (0..6)
                .map(|j| Field::new(format!("x{j}"), DataType::Float64, true))
                .collect::<Vec<_>>(),
        ));
        let batch = RecordBatch::try_new(
            schema,
            cols.into_iter()
                .map(|c| Arc::new(Float64Array::from(c)) as Arc<dyn arrow_array::Array>)
                .collect(),
        )
        .unwrap();
        let ctx = SessionContext::new();
        vec![NodeInput::new_dataframe(0, ctx.read_batch(batch).unwrap())]
    }

    #[tokio::test]
    async fn two_factor_model_estimates_and_fits() {
        use arrow_array::Array;
        let inputs = cfa_input();
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let mut node = EpiCfaNodeFactory
            .build(
                serde_json::json!({
                    "indicator_columns": ["x0", "x1", "x2", "x3", "x4", "x5"],
                    "loadings": [
                        { "indicator": 0, "factor": 0, "fixed": 1.0 },
                        { "indicator": 1, "factor": 0 },
                        { "indicator": 2, "factor": 0 },
                        { "indicator": 3, "factor": 1, "fixed": 1.0 },
                        { "indicator": 4, "factor": 1 },
                        { "indicator": 5, "factor": 1 }
                    ],
                    "factor_covariances": [[0, 1]]
                }),
                ctx.clone(),
            )
            .unwrap();
        let outputs = node
            .execute(
                &ctx,
                &inputs,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        // Estimates: 4 free loadings + 1 covariance + 6 variances.
        let estimates = outputs
            .get(&0)
            .and_then(|v| v.as_dataframe().ok())
            .expect("estimates table");
        let batches = estimates.clone().collect().await.unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 11, "4 loadings + 1 covariance + 6 variances");

        // Fit row present with df ≥ 0 and finite chi-square.
        let fit = outputs
            .get(&1)
            .and_then(|v| v.as_dataframe().ok())
            .expect("fit table");
        let rows = fit.clone().collect().await.unwrap();
        assert_eq!(rows[0].num_rows(), 1);
        let chisq = rows[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(chisq.value(0).is_finite());
    }
}
