//! Group-based trajectory model (Nagin GBTM) node.
//!
//! Wraps [`epi::gbtm`]. Fits K polynomial group trajectories over repeated
//! measures and emits a group-summary table plus a per-observation
//! assignment table.

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
pub enum EpiGbtmError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Computation(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiGbtmError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl dag_core::dag::NodeError for EpiGbtmError {
    fn node_type(&self) -> &str {
        "epi_gbtm"
    }
}

fn default_seed() -> u64 {
    42
}

/// Spec for [`EpiGbtmNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiGbtmSpec {
    /// Subject identifier column; rows sharing an id form one trajectory.
    pub id_column: String,
    /// Time column (within-subject, any numeric scale).
    pub time_column: String,
    /// Outcome value column.
    pub value_column: String,
    /// Number of latent groups K (default 3).
    #[serde(default = "default_n_groups")]
    pub n_groups: usize,
    /// Polynomial degree per trajectory: 0 = intercept, 1 = linear,
    /// 2 = quadratic (default).
    #[serde(default = "default_poly_degree")]
    pub poly_degree: usize,
    /// Maximum EM iterations (default 500).
    #[serde(default = "default_max_iter")]
    pub max_iter: usize,
    /// Convergence tolerance on log-likelihood (default 1e-6).
    #[serde(default = "default_tol")]
    pub tol: f64,
    /// Random seed (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_n_groups() -> usize {
    3
}
fn default_poly_degree() -> usize {
    2
}
fn default_max_iter() -> usize {
    500
}
fn default_tol() -> f64 {
    1e-6
}

/// Node fitting group-based trajectories over repeated measures.
#[derive(Clone)]
pub struct EpiGbtmNode {
    meta: NodePorts,
    spec: EpiGbtmSpec,
}

pub struct EpiGbtmNodeFactory;

impl NodeFactory for EpiGbtmNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_gbtm"
    }

    fn desc(&self) -> &'static str {
        "Group-based trajectory model (Nagin): latent group trajectories over repeated measures."
    }

    fn doc(&self) -> &'static str {
        "Fits a group-based trajectory mixture (Nagin GBTM) over repeated \
         measures: rows sharing `id_column` form one subject's trajectory \
         over (`time_column`, `value_column`). EM assigns each subject to \
         one of `n_groups` polynomial trajectories of `poly_degree`.\n\n\
         Port 0 — group summary: `group_id, pi, sigma, n_assigned, \
         coef_0, coef_1, coef_2` (unused polynomial coefficients are \
         null; `pi` is the mixing proportion).\n\
         Port 1 — per-subject assignment: `id, assigned_group, \
         posterior_max, log_likelihood, bic, converged` (fit columns are \
         repeated on every row so downstream SQL can filter)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiGbtmSpec)
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
        let spec: EpiGbtmSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EpiGbtmNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(None)
                .add_output_port(None),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EpiGbtmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "epi_gbtm"
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
            .ok_or_else(|| EpiGbtmError::Column("no input connected".into()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| EpiGbtmError::Collect(e.to_string()))?;

        let ids = dag_core::arrow_util::extract_string_column(&batches, &self.spec.id_column)?;
        let times = extract_numeric_lenient(&batches, &self.spec.time_column)?;
        let values = extract_numeric_lenient(&batches, &self.spec.value_column)?;

        // Group rows by subject id (complete cases only), preserving the
        // first-seen order so output assignment rows are deterministic.
        let mut order: Vec<String> = Vec::new();
        let mut series: std::collections::HashMap<String, Vec<(f64, f64)>> =
            std::collections::HashMap::new();
        for i in 0..ids.len() {
            let (t, v) = (times[i], values[i]);
            if t.is_nan() || v.is_nan() {
                continue;
            }
            let id = ids[i].clone();
            if !series.contains_key(&id) {
                order.push(id.clone());
            }
            series.entry(id).or_default().push((t, v));
        }
        if series.is_empty() {
            return Err(EpiGbtmError::Column("no complete-case rows".into()).into());
        }
        // Within a subject, sort by time — the model expects ascending t.
        let data: Vec<Vec<(f64, f64)>> = order
            .iter()
            .map(|id| {
                let mut points = series[id].clone();
                points.sort_by(|a, b| a.0.total_cmp(&b.0));
                points
            })
            .collect();

        let opts = epi::gbtm::GbtmOptions {
            n_groups: self.spec.n_groups,
            poly_degree: self.spec.poly_degree,
            max_iter: self.spec.max_iter,
            tol: self.spec.tol,
            seed: self.spec.seed,
        };
        let result =
            epi::gbtm::gbtm(&data, &opts).map_err(|e| EpiGbtmError::Computation(e.to_string()))?;

        // Port 0: group summary.
        let deg = self.spec.poly_degree;
        let group_rows = (0..result.pi.len())
            .map(|k| {
                let n_assigned = result.group_assignment.iter().filter(|g| **g == k).count();
                (k, n_assigned)
            })
            .collect::<Vec<_>>();
        let group_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("group_id", DataType::UInt64, true),
                Field::new("pi", DataType::Float64, true),
                Field::new("sigma", DataType::Float64, true),
                Field::new("n_assigned", DataType::Int64, true),
                Field::new("coef_0", DataType::Float64, true),
                Field::new("coef_1", DataType::Float64, true),
                Field::new("coef_2", DataType::Float64, true),
            ])),
            vec![
                Arc::new(UInt64Array::from(
                    group_rows
                        .iter()
                        .map(|(k, _)| Some(*k as u64))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    result.pi.iter().map(|v| Some(*v)).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    result.sigmas.iter().map(|v| Some(*v)).collect::<Vec<_>>(),
                )),
                Arc::new(Int64Array::from(
                    group_rows
                        .iter()
                        .map(|(_, n)| Some(*n as i64))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    result
                        .betas
                        .iter()
                        .map(|b| b.first().copied())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    result
                        .betas
                        .iter()
                        .map(|b| if deg >= 1 { b.get(1).copied() } else { None })
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    result
                        .betas
                        .iter()
                        .map(|b| if deg >= 2 { b.get(2).copied() } else { None })
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .map_err(|e| EpiGbtmError::Computation(format!("group batch: {e}")))?;

        // Port 1: per-subject assignment (order preserved).
        let ids_out: Vec<Option<&str>> = order.iter().map(|s| Some(s.as_str())).collect();
        let assignment_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Utf8, true),
                Field::new("assigned_group", DataType::UInt64, true),
                Field::new("posterior_max", DataType::Float64, true),
                Field::new("log_likelihood", DataType::Float64, true),
                Field::new("bic", DataType::Float64, true),
                Field::new("converged", DataType::Boolean, true),
            ])),
            vec![
                Arc::new(StringArray::from(ids_out)),
                Arc::new(UInt64Array::from(
                    result
                        .group_assignment
                        .iter()
                        .map(|g| Some(*g as u64))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    result
                        .posterior
                        .iter()
                        .map(|p| p.iter().cloned().fold(f64::NAN, f64::max))
                        .map(Some)
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(vec![
                    Some(result.log_likelihood);
                    result.group_assignment.len()
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(result.bic);
                    result.group_assignment.len()
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(result.converged);
                    result.group_assignment.len()
                ])),
            ],
        )
        .map_err(|e| EpiGbtmError::Computation(format!("assignment batch: {e}")))?;

        let session = node_ctx.session();
        let df0 = session
            .read_batch(group_batch)
            .map_err(|e| EpiGbtmError::ReadBatch(e.to_string()))?;
        let df1 = session
            .read_batch(assignment_batch)
            .map_err(|e| EpiGbtmError::ReadBatch(e.to_string()))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df0);
        outputs.insert(1, df1);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    /// 6 subjects × 4 time points following two clearly separated slopes.
    fn panel_input() -> Vec<NodeInput> {
        let mut ids: Vec<Option<String>> = Vec::new();
        let mut ts: Vec<Option<f64>> = Vec::new();
        let mut ys: Vec<Option<f64>> = Vec::new();
        for subject in 0..6 {
            for t in 0..4_i64 {
                let slope = if subject < 3 { 1.0 } else { 5.0 };
                ids.push(Some(format!("s{subject}")));
                ts.push(Some(t as f64));
                ys.push(Some(slope * t as f64 + (subject % 2) as f64 * 0.1));
            }
        }
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Utf8, true),
                Field::new("t", DataType::Float64, true),
                Field::new("y", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(ids)),
                Arc::new(Float64Array::from(ts)),
                Arc::new(Float64Array::from(ys)),
            ],
        )
        .unwrap();
        let ctx = SessionContext::new();
        vec![NodeInput::new_dataframe(0, ctx.read_batch(batch).unwrap())]
    }

    #[tokio::test]
    async fn separates_two_slopes_into_two_groups() {
        let inputs = panel_input();
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let mut node = EpiGbtmNodeFactory
            .build(
                serde_json::json!({
                    "id_column": "id",
                    "time_column": "t",
                    "value_column": "y",
                    "n_groups": 2,
                    "poly_degree": 1,
                    "seed": 42
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
        let groups = outputs
            .get(&0)
            .and_then(|v| v.as_dataframe().ok())
            .expect("group table");
        let batches = groups.clone().collect().await.unwrap();
        assert_eq!(batches[0].num_rows(), 2, "two requested groups");
        // Assignment covers all 6 subjects.
        let assignments = outputs
            .get(&1)
            .and_then(|v| v.as_dataframe().ok())
            .expect("assignment table");
        let rows = assignments.clone().collect().await.unwrap();
        assert_eq!(rows[0].num_rows(), 6);
    }
}
