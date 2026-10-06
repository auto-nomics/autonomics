//! Latent class analysis node.
//!
//! Wraps [`epi::lca`]. Fits K latent classes over binary indicators and
//! emits a class table plus per-subject assignment.

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
pub enum EpiLcaError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Computation(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiLcaError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl dag_core::dag::NodeError for EpiLcaError {
    fn node_type(&self) -> &str {
        "epi_lca"
    }
}

fn default_n_classes() -> usize {
    2
}

/// Spec for [`EpiLcaNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiLcaSpec {
    /// Binary (0/1) indicator columns, one per observed item.
    pub indicator_columns: Vec<String>,
    /// Optional subject id column carried through to the assignment port.
    #[serde(default)]
    pub id_column: Option<String>,
    /// Number of latent classes K (default 2).
    #[serde(default = "default_n_classes")]
    pub n_classes: usize,
    /// Maximum EM iterations (default 1000).
    #[serde(default = "default_max_iter")]
    pub max_iter: usize,
    /// Convergence tolerance on log-likelihood (default 1e-7).
    #[serde(default = "default_tol")]
    pub tol: f64,
    /// Random seed (default 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_max_iter() -> usize {
    1000
}
fn default_tol() -> f64 {
    1e-7
}
fn default_seed() -> u64 {
    42
}

/// Node fitting latent classes over binary indicators.
#[derive(Clone)]
pub struct EpiLcaNode {
    meta: NodePorts,
    spec: EpiLcaSpec,
}

pub struct EpiLcaNodeFactory;

impl NodeFactory for EpiLcaNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_lca"
    }

    fn desc(&self) -> &'static str {
        "Latent class analysis over binary indicators: classes, item probabilities, assignment."
    }

    fn doc(&self) -> &'static str {
        "Fits a latent class model (EM) over `indicator_columns` (binary \
         0/1 rows). Rows with any missing indicator are dropped.\n\n\
         Port 0 — class table: `class_id, prevalence, n_assigned, \
         log_likelihood, bic, aic, converged`.\n\
         Port 1 — item-response table: `class_id, indicator, \
         p_response` (`P(indicator = 1 | class)`, indicator named by its \
         0-based index).\n\
         Port 2 — per-subject assignment: `id` (or row index when \
         `id_column` is omitted), `assigned_class`, `posterior_max`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiLcaSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None)
            .add_output_port(None)
            .add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EpiLcaSpec = serde_json::from_value(spec)?;
        if spec.indicator_columns.is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "epi_lca requires at least one indicator column".into(),
            ));
        }
        Ok(Box::new(EpiLcaNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(None)
                .add_output_port(None)
                .add_output_port(None),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EpiLcaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "epi_lca"
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
            .ok_or_else(|| EpiLcaError::Column("no input connected".into()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| EpiLcaError::Collect(e.to_string()))?;

        let indicator_raw: Vec<Vec<f64>> = self
            .spec
            .indicator_columns
            .iter()
            .map(|col| extract_numeric_lenient(&batches, col))
            .collect::<Result<_, _>>()?;
        let ids = match &self.spec.id_column {
            Some(col) => Some(dag_core::arrow_util::extract_string_column(&batches, col)?),
            None => None,
        };

        // Complete cases over every indicator; validate binary-ness.
        let n_rows = indicator_raw.first().map(Vec::len).unwrap_or(0);
        let mut indicators: Vec<Vec<u64>> = Vec::new();
        let mut kept: Vec<usize> = Vec::new();
        for i in 0..n_rows {
            let mut row = Vec::with_capacity(self.spec.indicator_columns.len());
            let mut complete = true;
            for col_data in &indicator_raw {
                let v = col_data[i];
                if v.is_nan() {
                    complete = false;
                    break;
                }
                if v != 0.0 && v != 1.0 {
                    return Err(EpiLcaError::Column(format!(
                        "indicator values must be binary (0/1), found {v}"
                    ))
                    .into());
                }
                row.push(v as u64);
            }
            if complete {
                indicators.push(row);
                kept.push(i);
            }
        }
        if indicators.is_empty() {
            return Err(EpiLcaError::Column("no complete-case rows".into()).into());
        }

        let opts = epi::lca::LcaOptions {
            n_classes: self.spec.n_classes,
            max_iter: self.spec.max_iter,
            tol: self.spec.tol,
            seed: self.spec.seed,
        };
        // `epi::lca` takes indicator-major data (`indicators[j]` = column of
        // indicator j, length N); we collected subject-major rows.
        let n_subjects = indicators.len();
        let j_items = self.spec.indicator_columns.len();
        let mut indicator_major: Vec<Vec<u64>> = vec![Vec::with_capacity(n_subjects); j_items];
        for row in &indicators {
            for (j, value) in row.iter().enumerate() {
                indicator_major[j].push(*value);
            }
        }
        let result = epi::lca::lca(&indicator_major, &opts)
            .map_err(|e| EpiLcaError::Computation(e.to_string()))?;

        let session = node_ctx.session();
        let mut outputs = PortOutputs::new();

        // Port 0: class summary.
        let class_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("class_id", DataType::UInt64, true),
                Field::new("prevalence", DataType::Float64, true),
                Field::new("n_assigned", DataType::Int64, true),
                Field::new("log_likelihood", DataType::Float64, true),
                Field::new("bic", DataType::Float64, true),
                Field::new("aic", DataType::Float64, true),
                Field::new("converged", DataType::Boolean, true),
            ])),
            vec![
                Arc::new(UInt64Array::from(
                    (0..result.class_prevalence.len())
                        .map(|k| Some(k as u64))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    result
                        .class_prevalence
                        .iter()
                        .map(|v| Some(*v))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Int64Array::from(
                    (0..result.class_prevalence.len())
                        .map(|k| {
                            Some(result.class_assignment.iter().filter(|g| **g == k).count() as i64)
                        })
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(vec![
                    Some(result.log_likelihood);
                    result.class_prevalence.len()
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(result.bic);
                    result.class_prevalence.len()
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(result.aic);
                    result.class_prevalence.len()
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(result.converged);
                    result.class_prevalence.len()
                ])),
            ],
        )
        .map_err(|e| EpiLcaError::Computation(format!("class batch: {e}")))?;
        outputs.insert(
            0,
            session
                .read_batch(class_batch)
                .map_err(|e| EpiLcaError::ReadBatch(e.to_string()))?,
        );

        // Port 1: item-response probabilities (long).
        let mut class_col: Vec<Option<u64>> = Vec::new();
        let mut item_col: Vec<Option<u64>> = Vec::new();
        let mut p_col: Vec<Option<f64>> = Vec::new();
        for (j, item) in result.item_probabilities.iter().enumerate() {
            for (k, p) in item.iter().enumerate() {
                class_col.push(Some(k as u64));
                item_col.push(Some(j as u64));
                p_col.push(Some(*p));
            }
        }
        let item_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("class_id", DataType::UInt64, true),
                Field::new("indicator", DataType::UInt64, true),
                Field::new("p_response", DataType::Float64, true),
            ])),
            vec![
                Arc::new(UInt64Array::from(class_col)),
                Arc::new(UInt64Array::from(item_col)),
                Arc::new(Float64Array::from(p_col)),
            ],
        )
        .map_err(|e| EpiLcaError::Computation(format!("item batch: {e}")))?;
        outputs.insert(
            1,
            session
                .read_batch(item_batch)
                .map_err(|e| EpiLcaError::ReadBatch(e.to_string()))?,
        );

        // Port 2: per-subject assignment.
        let id_strings: Vec<Option<String>> = match &ids {
            Some(ids) => kept.iter().map(|&i| Some(ids[i].clone())).collect(),
            None => (0..kept.len()).map(|i| Some(i.to_string())).collect(),
        };
        let assignment_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Utf8, true),
                Field::new("assigned_class", DataType::UInt64, true),
                Field::new("posterior_max", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(
                    id_strings.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
                )),
                Arc::new(UInt64Array::from(
                    result
                        .class_assignment
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
            ],
        )
        .map_err(|e| EpiLcaError::Computation(format!("assignment batch: {e}")))?;
        outputs.insert(
            2,
            session
                .read_batch(assignment_batch)
                .map_err(|e| EpiLcaError::ReadBatch(e.to_string()))?,
        );

        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    /// 8 subjects in two crisp response patterns over 3 indicators.
    fn lca_input() -> Vec<NodeInput> {
        let patterns: [[f64; 3]; 2] = [[1.0, 1.0, 1.0], [0.0, 0.0, 0.0]];
        let mut cols: [Vec<Option<f64>>; 3] = Default::default();
        for subject in 0..8 {
            let pattern = patterns[subject % 2];
            for j in 0..3 {
                cols[j].push(Some(pattern[j]));
            }
        }
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("i0", DataType::Float64, true),
                Field::new("i1", DataType::Float64, true),
                Field::new("i2", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Float64Array::from(cols[0].clone())),
                Arc::new(Float64Array::from(cols[1].clone())),
                Arc::new(Float64Array::from(cols[2].clone())),
            ],
        )
        .unwrap();
        let ctx = SessionContext::new();
        vec![NodeInput::new_dataframe(0, ctx.read_batch(batch).unwrap())]
    }

    #[tokio::test]
    async fn two_classes_recover_two_patterns() {
        let inputs = lca_input();
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let mut node = EpiLcaNodeFactory
            .build(
                serde_json::json!({
                    "indicator_columns": ["i0", "i1", "i2"],
                    "n_classes": 2
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
        // 3 ports present.
        for port in 0..3 {
            assert!(outputs.get(&port).is_some(), "port {port} missing");
        }
        let classes = outputs
            .get(&0)
            .and_then(|v| v.as_dataframe().ok())
            .expect("class table");
        let batches = classes.clone().collect().await.unwrap();
        assert_eq!(batches[0].num_rows(), 2);
    }

    #[tokio::test]
    async fn nonbinary_indicator_fails() {
        let mut cols: Vec<Vec<Option<f64>>> = vec![vec![], vec![], vec![]];
        for _ in 0..4 {
            for col in cols.iter_mut() {
                col.push(Some(2.5));
            }
        }
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("i0", DataType::Float64, true),
                Field::new("i1", DataType::Float64, true),
                Field::new("i2", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Float64Array::from(cols[0].clone())),
                Arc::new(Float64Array::from(cols[1].clone())),
                Arc::new(Float64Array::from(cols[2].clone())),
            ],
        )
        .unwrap();
        let session = SessionContext::new();
        let inputs = vec![NodeInput::new_dataframe(
            0,
            session.read_batch(batch).unwrap(),
        )];
        let ctx = NodeCtx::new(session.runtime_env(), None);
        let mut node = EpiLcaNodeFactory
            .build(
                serde_json::json!({
                    "indicator_columns": ["i0", "i1", "i2"]
                }),
                ctx.clone(),
            )
            .unwrap();
        let err = node
            .execute(
                &ctx,
                &inputs,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("binary"), "{err}");
    }
}
