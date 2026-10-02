//! Ordinal logistic regression transform node.
//!
//! Wraps [`statkit::regression::ordinal_logistic`] and fits an unweighted
//! proportional-odds (cumulative-logit) model. Numeric outcomes are ordered by
//! value; string outcomes are ordered lexically unless `outcome_levels`
//! supplies an explicit order.
//!
//! Output schema (one row per slope, followed by one row per threshold):
//! `term`, `coefficient`, `std_error`, `z_stat`, `p_value`, `odds_ratio`,
//! `or_ci_lower`, `or_ci_upper`, `log_likelihood`, `n_obs`, `n_levels`, and
//! `converged`.

use std::collections::BTreeSet;
use std::sync::Arc;

use arrow_array::{
    Array, BooleanArray, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
    RecordBatch, StringArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::arrow_util::{
    ColumnError, column_dtype, column_index, extract_numeric_lenient, extract_string_column,
};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum OrdinalLogisticRegressionError {
    #[error("{0}")]
    Column(String),
    #[error("no predictor columns specified")]
    NoPredictors,
    #[error("outcome column and predictor column '{0}' are the same")]
    OutcomeIsPredictor(String),
    #[error("ordinal logistic fit failed: {0}")]
    Fit(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
}

impl From<ColumnError> for OrdinalLogisticRegressionError {
    fn from(error: ColumnError) -> Self {
        Self::Column(error.to_string())
    }
}

impl ::dag_core::dag::NodeError for OrdinalLogisticRegressionError {
    fn node_type(&self) -> &str {
        "ordinal_logistic_regression"
    }
}

/// Spec for [`OrdinalLogisticRegressionNode`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OrdinalLogisticRegressionNodeSpec {
    /// Names of numeric predictor columns. At least one is required.
    pub predictors: Vec<String>,
    /// Name of the ordinal outcome column (numeric or string).
    pub outcome: String,
    /// Explicit ordered outcome levels. Recommended for string labels.
    #[serde(default)]
    pub outcome_levels: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct OrdinalLogisticRegressionNode {
    meta: NodePorts,
    predictors: Vec<String>,
    outcome: String,
    outcome_levels: Option<Vec<String>>,
}

impl OrdinalLogisticRegressionNode {
    pub fn new(
        predictors: Vec<String>,
        outcome: String,
        outcome_levels: Option<Vec<String>>,
    ) -> Self {
        Self {
            meta: port_layout(),
            predictors,
            outcome,
            outcome_levels,
        }
    }
}

pub struct OrdinalLogisticRegressionNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None).add_input_port(None)
}

impl NodeFactory for OrdinalLogisticRegressionNodeFactory {
    fn kind(&self) -> &'static str {
        "ordinal_logistic_regression"
    }

    fn desc(&self) -> &'static str {
        "Unweighted ordinal logistic regression (proportional odds)."
    }

    fn doc(&self) -> &'static str {
        "Fits an unweighted proportional-odds ordinal logistic regression. \
        Numeric outcomes are ordered by value; string outcomes are ordered \
        lexically unless the optional outcome_levels array supplies an explicit \
        order. Predictor columns must be numeric. Rows with a missing outcome \
        or predictor are removed using complete-case filtering. Slopes are \
        followed by ordered thresholds in the output summary."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OrdinalLogisticRegressionNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let parsed: OrdinalLogisticRegressionNodeSpec = serde_json::from_value(spec)?;
        if parsed.predictors.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "ordinal_logistic_regression".to_string(),
                reason: "predictors must be a non-empty array".to_string(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(
                    OrdinalLogisticRegressionNodeSpec
                ))
                .unwrap_or_default(),
            });
        }
        if parsed.predictors.contains(&parsed.outcome) {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "ordinal_logistic_regression".to_string(),
                reason: format!(
                    "outcome '{}' must not also appear in predictors",
                    parsed.outcome
                ),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(
                    OrdinalLogisticRegressionNodeSpec
                ))
                .unwrap_or_default(),
            });
        }
        Ok(Box::new(OrdinalLogisticRegressionNode::new(
            parsed.predictors,
            parsed.outcome,
            parsed.outcome_levels,
        )))
    }
}

#[async_trait]
impl DagNode for OrdinalLogisticRegressionNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "ordinal_logistic_regression"
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
        let input = inputs.first().ok_or_else(|| {
            OrdinalLogisticRegressionError::Column("no input connected".to_string())
        })?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|error| OrdinalLogisticRegressionError::Collect(error.to_string()))?;
        column_index(&batches, &self.outcome)?;
        let outcome_dtype = column_dtype(&batches, &self.outcome)?;

        let mut predictor_columns = Vec::with_capacity(self.predictors.len());
        for predictor in &self.predictors {
            predictor_columns.push(
                extract_numeric_lenient(&batches, predictor)
                    .map_err(OrdinalLogisticRegressionError::from)?,
            );
        }

        enum OutcomeValues {
            Numeric(Vec<f64>),
            Text(Vec<String>),
        }
        let outcome_values = match outcome_dtype {
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float32
            | DataType::Float64 => OutcomeValues::Numeric(
                extract_numeric_lenient(&batches, &self.outcome)
                    .map_err(OrdinalLogisticRegressionError::from)?,
            ),
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => OutcomeValues::Text(
                extract_string_column(&batches, &self.outcome)
                    .map_err(OrdinalLogisticRegressionError::from)?,
            ),
            unexpected => {
                return Err(OrdinalLogisticRegressionError::Column(format!(
                    "outcome '{}' has type {}, expected numeric or string",
                    self.outcome, unexpected
                ))
                .into());
            }
        };

        let n = predictor_columns
            .first()
            .map(|column| column.len())
            .unwrap_or_default();
        let mut y_ord = Vec::with_capacity(n);
        let mut x_filtered = vec![Vec::with_capacity(n); self.predictors.len()];

        match &outcome_values {
            OutcomeValues::Numeric(values) => {
                let levels = numeric_outcome_levels(values, self.outcome_levels.as_deref())?;
                for row in 0..n {
                    if values[row].is_nan()
                        || predictor_columns.iter().any(|column| column[row].is_nan())
                    {
                        continue;
                    }
                    let level = levels
                        .iter()
                        .position(|&level| level == values[row])
                        .ok_or_else(|| {
                            OrdinalLogisticRegressionError::Column(format!(
                                "outcome '{}' contains a value outside outcome_levels",
                                self.outcome
                            ))
                        })?;
                    y_ord.push(level);
                    retain_row(&mut x_filtered, &predictor_columns, row);
                }
            }
            OutcomeValues::Text(values) => {
                let levels = text_outcome_levels(values, self.outcome_levels.as_deref())?;
                for row in 0..n {
                    if values[row].is_empty()
                        || predictor_columns.iter().any(|column| column[row].is_nan())
                    {
                        continue;
                    }
                    let level = levels
                        .iter()
                        .position(|level| level == &values[row])
                        .ok_or_else(|| {
                            OrdinalLogisticRegressionError::Column(format!(
                                "outcome '{}' contains a label outside outcome_levels",
                                self.outcome
                            ))
                        })?;
                    y_ord.push(level);
                    retain_row(&mut x_filtered, &predictor_columns, row);
                }
            }
        }

        if y_ord.is_empty() {
            return Err(OrdinalLogisticRegressionError::Column(
                "no complete-case rows after removing nulls".to_string(),
            )
            .into());
        }

        let x_slices: Vec<&[f64]> = x_filtered.iter().map(|column| column.as_slice()).collect();
        let fit = statkit::regression::ordinal_logistic(&x_slices, &y_ord)
            .map_err(|error| OrdinalLogisticRegressionError::Fit(error.to_string()))?;

        let outcome_levels = match &outcome_values {
            OutcomeValues::Numeric(values) => {
                let levels = numeric_outcome_levels(values, self.outcome_levels.as_deref())
                    .map_err(OrdinalLogisticRegressionError::from)?;
                self.outcome_levels
                    .clone()
                    .unwrap_or_else(|| levels.iter().map(|level| level.to_string()).collect())
            }
            OutcomeValues::Text(values) => {
                text_outcome_levels(values, self.outcome_levels.as_deref())
                    .map_err(OrdinalLogisticRegressionError::from)?
            }
        };
        let batch = build_ordinal_logistic_batch(&fit, &self.predictors, &outcome_levels);
        let dataframe = node_ctx
            .session()
            .read_batch(batch)
            .map_err(|error| OrdinalLogisticRegressionError::ReadBatch(error.to_string()))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

fn retain_row(filtered: &mut [Vec<f64>], raw: &[Vec<f64>], row: usize) {
    for (filtered_column, raw_column) in filtered.iter_mut().zip(raw) {
        filtered_column.push(raw_column[row]);
    }
}

fn numeric_outcome_levels(
    values: &[f64],
    explicit: Option<&[String]>,
) -> Result<Vec<f64>, ColumnError> {
    let levels = match explicit {
        Some(labels) => {
            if labels.len() < 2 {
                return Err(ColumnError::WrongType {
                    name: "outcome_levels".to_string(),
                    dtype: labels.len().to_string(),
                    expected: "at least two ordered levels",
                });
            }
            let mut parsed = Vec::with_capacity(labels.len());
            for label in labels {
                let value = label.parse::<f64>().map_err(|_| ColumnError::WrongType {
                    name: "outcome_levels".to_string(),
                    dtype: label.clone(),
                    expected: "numeric outcome levels",
                })?;
                if !value.is_finite() {
                    return Err(ColumnError::WrongType {
                        name: "outcome_levels".to_string(),
                        dtype: label.clone(),
                        expected: "finite numeric outcome levels",
                    });
                }
                parsed.push(value);
            }
            if parsed.windows(2).any(|window| window[0] >= window[1]) {
                return Err(ColumnError::WrongType {
                    name: "outcome_levels".to_string(),
                    dtype: "non-increasing numeric levels".to_string(),
                    expected: "strictly increasing ordered levels",
                });
            }
            parsed
        }
        None => {
            let unique: BTreeSet<i64> = values
                .iter()
                .filter(|value| value.is_finite() && value.fract() == 0.0)
                .map(|value| *value as i64)
                .collect();
            unique.into_iter().map(|value| value as f64).collect()
        }
    };
    if levels.len() < 2 {
        return Err(ColumnError::WrongType {
            name: "outcome".to_string(),
            dtype: levels.len().to_string(),
            expected: "an ordinal numeric outcome with at least two levels",
        });
    }
    Ok(levels)
}

fn text_outcome_levels(
    values: &[String],
    explicit: Option<&[String]>,
) -> Result<Vec<String>, ColumnError> {
    let levels = match explicit {
        Some(levels) => levels.to_vec(),
        None => values
            .iter()
            .filter(|value| !value.is_empty())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    if levels.len() < 2 {
        return Err(ColumnError::WrongType {
            name: "outcome".to_string(),
            dtype: levels.len().to_string(),
            expected: "an ordinal string outcome with at least two levels",
        });
    }
    Ok(levels)
}

fn build_ordinal_logistic_batch(
    fit: &statkit::regression::OrdinalLogisticResult,
    predictors: &[String],
    outcome_levels: &[String],
) -> RecordBatch {
    let n = fit.coefficients.len() + fit.thresholds.len();
    let all_coefficients = fit.all_coefficients();
    let terms = predictors
        .iter()
        .map(|predictor| predictor.to_string())
        .chain((0..outcome_levels.len() - 1).map(|index| {
            format!(
                "threshold[{}]|[{}]",
                outcome_levels[index],
                outcome_levels[index + 1]
            )
        }))
        .collect::<Vec<String>>();

    let repeat = |value: f64| vec![value; n];
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("term", DataType::Utf8, false),
            Field::new("coefficient", DataType::Float64, false),
            Field::new("std_error", DataType::Float64, false),
            Field::new("z_stat", DataType::Float64, false),
            Field::new("p_value", DataType::Float64, false),
            Field::new("odds_ratio", DataType::Float64, false),
            Field::new("or_ci_lower", DataType::Float64, false),
            Field::new("or_ci_upper", DataType::Float64, false),
            Field::new("log_likelihood", DataType::Float64, false),
            Field::new("n_obs", DataType::Int32, false),
            Field::new("n_levels", DataType::Int32, false),
            Field::new("converged", DataType::Boolean, false),
        ])),
        vec![
            Arc::new(StringArray::from(terms)),
            Arc::new(Float64Array::from(all_coefficients)),
            Arc::new(Float64Array::from(fit.std_errors.clone())),
            Arc::new(Float64Array::from(fit.z_stats.clone())),
            Arc::new(Float64Array::from(fit.p_values.clone())),
            Arc::new(Float64Array::from(fit.odds_ratios.clone())),
            Arc::new(Float64Array::from(fit.odds_ratio_ci_lower.clone())),
            Arc::new(Float64Array::from(fit.odds_ratio_ci_upper.clone())),
            Arc::new(Float64Array::from(repeat(fit.log_likelihood))),
            Arc::new(Int32Array::from(vec![fit.n_obs as i32; n])),
            Arc::new(Int32Array::from(vec![fit.n_levels as i32; n])),
            Arc::new(BooleanArray::from(vec![fit.converged; n])),
        ],
    )
    .expect("schema mismatch in build_ordinal_logistic_batch")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_logistic_regression;
    use arrow_array::{Float64Array, StringArray};

    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    fn make_batch(columns: Vec<(&str, Vec<f64>)>, outcome: (&str, Vec<&str>)) -> RecordBatch {
        let mut fields = columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Float64, false))
            .collect::<Vec<_>>();
        fields.push(Field::new(outcome.0, DataType::Utf8, false));
        let mut arrays = columns
            .into_iter()
            .map(|(_, values)| Arc::new(Float64Array::from(values)) as Arc<dyn Array>)
            .collect::<Vec<_>>();
        arrays.push(Arc::new(StringArray::from(outcome.1)));
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    #[tokio::test]
    async fn fits_string_outcome_with_explicit_level_order() {
        let batch = make_batch(
            vec![("x", vec![-2.0, -1.0, 0.0, 1.0, 2.0, 3.0])],
            (
                "severity",
                vec!["low", "low", "medium", "high", "high", "high"],
            ),
        );
        let mut node = OrdinalLogisticRegressionNode::new(
            vec!["x".to_string()],
            "severity".to_string(),
            Some(vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
            ]),
        );
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(batch)
                .unwrap(),
        );
        let outputs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let rows = outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(rows.iter().map(|batch| batch.num_rows()).sum::<usize>(), 3);
        let terms = rows
            .iter()
            .flat_map(|batch| {
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .iter()
                    .map(|value| value.unwrap().to_string())
            })
            .collect::<Vec<_>>();
        assert_eq!(terms[0], "x");
        assert_eq!(terms[1], "threshold[low]|[medium]");
        assert_eq!(terms[2], "threshold[medium]|[high]");
    }

    #[test]
    fn factory_uses_unambiguous_kind() {
        assert_eq!(
            OrdinalLogisticRegressionNodeFactory {}.kind(),
            "ordinal_logistic_regression"
        );
        assert_eq!(
            binary_logistic_regression::BinaryLogisticRegressionNodeFactory {}.kind(),
            "binary_logistic_regression"
        );
    }
}
