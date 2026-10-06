//! Multi-state Markov model node.
//!
//! Wraps [`epi::multistate`] (Aalen-Johansen estimator). Consumes one row
//! per transition/censoring event and emits the estimated transition
//! probability matrix at each distinct event time in long format.

use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array, RecordBatch, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use dag_core::arrow_util::{ColumnError, extract_numeric_lenient};
use dag_core::dag::DagError;
use dag_core::dag::graph::PortOutputs;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

/// Errors surfaced by [`EpiMultistateNode`].
#[derive(Debug, thiserror::Error)]
pub enum EpiMultistateError {
    #[error("{0}")]
    Column(String),
    #[error("{0}")]
    Computation(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for EpiMultistateError {
    fn from(e: ColumnError) -> Self {
        Self::Column(e.to_string())
    }
}

impl dag_core::dag::NodeError for EpiMultistateError {
    fn node_type(&self) -> &str {
        "epi_multistate"
    }
}

/// Spec for [`EpiMultistateNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EpiMultistateSpec {
    /// Transition or censoring time column.
    pub time_column: String,
    /// Integer from-state column (0-based labels).
    pub from_state_column: String,
    /// Integer to-state column (0-based labels).
    pub to_state_column: String,
    /// Total number of states (≥ 2).
    pub n_states: usize,
    /// Optional state-entry time column; omitted means every subject enters
    /// at baseline 0.
    #[serde(default)]
    pub entry_time_column: Option<String>,
}

/// Node emitting Aalen-Johansen transition probabilities in long format.
#[derive(Clone)]
pub struct EpiMultistateNode {
    meta: NodePorts,
    spec: EpiMultistateSpec,
}

pub struct EpiMultistateNodeFactory;

impl NodeFactory for EpiMultistateNodeFactory {
    fn kind(&self) -> &'static str {
        "epi_multistate"
    }

    fn desc(&self) -> &'static str {
        "Multi-state Markov model (Aalen-Johansen): transition probabilities over time."
    }

    fn doc(&self) -> &'static str {
        "Fits a multi-state Markov model over subject transition rows \
         (time, from_state, to_state) and emits one row per \
         (time, from_state, to_state) with the Aalen-Johansen transition \
         probability P(to | from, time), the cumulative hazard, and the \
         number still at risk. `n_states` counts the state space \
         (labels are 0-based); `entry_time_column` optionally marks when \
         each subject entered its from-state.\n\n\
         Output schema: `time, from_state, to_state, transition_prob, \
         cum_hazard, n_at_risk`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EpiMultistateSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EpiMultistateSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EpiMultistateNode {
            meta: NodePorts::new().add_input_port(None).add_output_port(None),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EpiMultistateNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "epi_multistate"
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
            .ok_or_else(|| EpiMultistateError::Column("no input connected".into()))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| EpiMultistateError::Collect(e.to_string()))?;

        let time_raw = extract_numeric_lenient(&batches, &self.spec.time_column)?;
        let from_raw = extract_numeric_lenient(&batches, &self.spec.from_state_column)?;
        let to_raw = extract_numeric_lenient(&batches, &self.spec.to_state_column)?;
        let entry_raw = match &self.spec.entry_time_column {
            Some(col) => Some(extract_numeric_lenient(&batches, col)?),
            None => None,
        };

        // Complete-case filter (NaN time or state labels drop).
        let mut time = Vec::new();
        let mut from = Vec::new();
        let mut to = Vec::new();
        let mut entry: Vec<f64> = Vec::new();
        for i in 0..time_raw.len() {
            if time_raw[i].is_nan() || from_raw[i].is_nan() || to_raw[i].is_nan() {
                continue;
            }
            if let Some(entry_data) = &entry_raw {
                if entry_data[i].is_nan() {
                    continue;
                }
                entry.push(entry_data[i]);
            }
            time.push(time_raw[i]);
            from.push(from_raw[i] as u64);
            to.push(to_raw[i] as u64);
        }
        if time.is_empty() {
            return Err(EpiMultistateError::Column("no complete-case rows".into()).into());
        }

        let result = epi::multistate::multistate(
            &time,
            &from,
            &to,
            self.spec.n_states,
            if entry_raw.is_some() {
                Some(&entry)
            } else {
                None
            },
        )
        .map_err(|e| EpiMultistateError::Computation(e.to_string()))?;

        // Long format: one row per (time, from, to).
        let mut times: Vec<Option<f64>> = Vec::new();
        let mut fs: Vec<Option<u64>> = Vec::new();
        let mut ts: Vec<Option<u64>> = Vec::new();
        let mut probs: Vec<Option<f64>> = Vec::new();
        let mut hazards: Vec<Option<f64>> = Vec::new();
        let mut at_risk: Vec<Option<i64>> = Vec::new();
        for (ti, t) in result.times.iter().enumerate() {
            let counts = &result.state_counts[ti];
            for r in 0..result.n_states {
                for s in 0..result.n_states {
                    times.push(Some(*t));
                    fs.push(Some(r as u64));
                    ts.push(Some(s as u64));
                    probs.push(Some(result.p_matrices[ti][r][s]));
                    hazards.push(Some(result.cum_hazards[ti][r][s]));
                    at_risk.push(Some(counts[r] as i64));
                }
            }
        }

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("time", DataType::Float64, true),
                Field::new("from_state", DataType::UInt64, true),
                Field::new("to_state", DataType::UInt64, true),
                Field::new("transition_prob", DataType::Float64, true),
                Field::new("cum_hazard", DataType::Float64, true),
                Field::new("n_at_risk", DataType::Int64, true),
            ])),
            vec![
                Arc::new(Float64Array::from(times)),
                Arc::new(UInt64Array::from(fs)),
                Arc::new(UInt64Array::from(ts)),
                Arc::new(Float64Array::from(probs)),
                Arc::new(Float64Array::from(hazards)),
                Arc::new(Int64Array::from(at_risk)),
            ],
        )
        .map_err(|e| EpiMultistateError::Computation(format!("batch build: {e}")))?;

        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(|e| EpiMultistateError::ReadBatch(e.to_string()))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn input_with(rows: &[(f64, f64, f64)]) -> Vec<NodeInput> {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("t", DataType::Float64, true),
                Field::new("from", DataType::Float64, true),
                Field::new("to", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Float64Array::from(
                    rows.iter().map(|r| Some(r.0)).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    rows.iter().map(|r| Some(r.1)).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    rows.iter().map(|r| Some(r.2)).collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap();
        let ctx = SessionContext::new();
        let df = ctx.read_batch(batch).unwrap();
        vec![NodeInput::new_dataframe(0, df)]
    }

    #[tokio::test]
    async fn healthy_and_sick_rows_produce_probability_long_table() {
        // Two subjects: one 0→1 at t=1, one 1→2 at t=2.
        let inputs = input_with(&[(1.0, 0.0, 1.0), (2.0, 1.0, 2.0)]);
        let mut node = EpiMultistateNodeFactory
            .build(
                serde_json::json!({
                    "time_column": "t",
                    "from_state_column": "from",
                    "to_state_column": "to",
                    "n_states": 3
                }),
                NodeCtx::new(SessionContext::new().runtime_env(), None),
            )
            .unwrap();
        let outputs = node
            .execute(
                &NodeCtx::new(SessionContext::new().runtime_env(), None),
                &inputs,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outputs
            .get(&0)
            .and_then(|v| v.as_dataframe().ok())
            .expect("dataframe");
        let batches = df.clone().collect().await.unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        // 2 distinct times × 3×3 state pairs.
        assert_eq!(total, 18);
    }

    #[tokio::test]
    async fn state_labels_outside_n_states_fail() {
        let inputs = input_with(&[(1.0, 0.0, 5.0)]);
        let mut node = EpiMultistateNodeFactory
            .build(
                serde_json::json!({
                    "time_column": "t",
                    "from_state_column": "from",
                    "to_state_column": "to",
                    "n_states": 2
                }),
                NodeCtx::new(SessionContext::new().runtime_env(), None),
            )
            .unwrap();
        let err = node
            .execute(
                &NodeCtx::new(SessionContext::new().runtime_env(), None),
                &inputs,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exceeds n_states"), "{err}");
    }
}
