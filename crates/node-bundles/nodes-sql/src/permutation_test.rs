//! Label-permutation test over an upstream DataFrame.
//!
//! The null model is a global shuffle of the group labels across all rows —
//! no spatial, donor/subject, or network-constrained background is modelled.

use arrow_array::{Float64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::sync::Arc;

use crate::table_transforms::utf8_values;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

pub const PERMUTATION_TEST_KIND: &str = "permutation_test";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermutationAlternative {
    Greater,
    Less,
    TwoSided,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PermutationTestSpec {
    pub group_column: String,
    /// Aggregate SQL statistic expression, for example `AVG(value)`.
    pub statistic_sql: String,
    #[serde(default = "default_n_permutations")]
    pub n_permutations: u32,
    #[serde(default)]
    pub random_state: u64,
    #[serde(default = "default_alternative")]
    pub alternative: PermutationAlternative,
}

fn default_n_permutations() -> u32 {
    1000
}
fn default_alternative() -> PermutationAlternative {
    PermutationAlternative::Greater
}

#[derive(Clone)]
pub struct PermutationTestNode {
    ports: NodePorts,
    spec: PermutationTestSpec,
}

fn port_layout() -> NodePorts {
    NodePorts::new().add_input_port(None).add_output_port(None)
}

fn node_error(message: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: PERMUTATION_TEST_KIND.into(),
        msg: message.into(),
    }
}

struct DeterministicRng(u64);

impl DeterministicRng {
    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.0 = state;
        state
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let swap = (self.next_u64() % (index as u64 + 1)) as usize;
            values.swap(index, swap);
        }
    }
}

impl PermutationTestNode {
    async fn scalar(
        &self,
        ctx: &NodeCtx,
        table: &str,
        frame: datafusion::prelude::DataFrame,
    ) -> Result<f64, DagError> {
        let session = ctx.session();
        session
            .register_table(table, frame.into_view())
            .map_err(|error| node_error(format!("cannot register permutation input: {error}")))?;
        let sql = format!(
            "SELECT ({}) AS statistic FROM {}",
            self.spec.statistic_sql, table
        );
        let batches = session
            .sql(&sql)
            .await
            .map_err(|error| node_error(format!("invalid statistic SQL: {error}")))?
            .collect()
            .await
            .map_err(|error| node_error(format!("cannot evaluate statistic: {error}")))?;
        let Some(batch) = batches.first() else {
            return Err(node_error("statistic SQL returned no rows"));
        };
        if batch.num_rows() != 1 {
            return Err(node_error(
                "statistic SQL must return exactly one row; use an aggregate expression",
            ));
        }
        let values = arrow_cast::cast(batch.column(0), &DataType::Float64)
            .map_err(|error| node_error(format!("statistic is not numeric: {error}")))?;
        values
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| node_error("statistic cast did not produce Float64"))?
            .value(0)
            .pipe(|value| {
                if value.is_finite() {
                    Ok(value)
                } else {
                    Err(node_error("statistic is missing or non-finite"))
                }
            })
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}

impl<T> Pipe for T {}

#[async_trait]
impl DagNode for PermutationTestNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        PERMUTATION_TEST_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs
            .first()
            .ok_or_else(|| node_error("permutation_test requires one upstream DataFrame"))?;
        let frame = input.dataframe()?.clone();
        let fields = frame.schema().fields().clone();
        let (group_index, _) = fields.find(&self.spec.group_column).ok_or_else(|| {
            node_error(format!(
                "group column `{}` is absent",
                self.spec.group_column
            ))
        })?;
        let batches = frame.clone().collect().await?;
        let mut groups = Vec::new();
        for batch in &batches {
            groups.extend(utf8_values(batch.column(group_index))?);
        }
        if groups.is_empty() {
            return Err(node_error("input has no rows"));
        }

        let observed = self
            .scalar(ctx, "__permutation_observed", frame.clone())
            .await?;
        let mut rng = DeterministicRng(if self.spec.random_state == 0 {
            0x9e3779b97f4a7c15
        } else {
            self.spec.random_state
        });
        let mut exceed = 0_u32;
        let mut below = 0_u32;
        for _ in 0..self.spec.n_permutations {
            let mut shuffled = groups.clone();
            rng.shuffle(&mut shuffled);
            let mut cursor = 0;
            let mut permuted_frame: Option<datafusion::prelude::DataFrame> = None;
            for batch in &batches {
                let mut columns = batch.columns().to_vec();
                let values =
                    shuffled[cursor..(cursor + batch.num_rows()).min(shuffled.len())].to_vec();
                columns[group_index] = Arc::new(StringArray::from(values));
                let schema = batch.schema();
                let fields = schema
                    .fields()
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        if index == group_index {
                            Field::new(field.name(), DataType::Utf8, true)
                        } else {
                            field.as_ref().clone()
                        }
                    })
                    .collect::<Vec<_>>();
                let permuted_batch =
                    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
                        .map_err(|error| node_error(format!("cannot permute labels: {error}")))?;
                let next = ctx.session().read_batch(permuted_batch)?;
                permuted_frame = Some(match permuted_frame {
                    Some(left) => left.union(next).map_err(|error| {
                        node_error(format!("cannot assemble permuted input: {error}"))
                    })?,
                    None => next,
                });
                cursor += batch.num_rows();
            }
            let frame = permuted_frame.ok_or_else(|| node_error("input has no rows to permute"))?;
            let null = self.scalar(ctx, "__permutation_input", frame).await?;
            if null >= observed {
                exceed += 1;
            }
            if null <= observed {
                below += 1;
            }
        }

        let pvalue = match self.spec.alternative {
            PermutationAlternative::Greater => {
                (exceed as f64 + 1.0) / (self.spec.n_permutations as f64 + 1.0)
            }
            PermutationAlternative::Less => {
                (below as f64 + 1.0) / (self.spec.n_permutations as f64 + 1.0)
            }
            PermutationAlternative::TwoSided => {
                (2.0 * below.min(exceed) as f64 + 1.0) / (self.spec.n_permutations as f64 + 1.0)
            }
        }
        .min(1.0);

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("statistic", DataType::Float64, false),
                Field::new("pvalue", DataType::Float64, false),
                Field::new("n_permutations", DataType::UInt32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![observed])),
                Arc::new(Float64Array::from(vec![pvalue])),
                Arc::new(UInt32Array::from(vec![self.spec.n_permutations])),
            ],
        )
        .map_err(|error| node_error(format!("cannot build result: {error}")))?;
        let output = ctx.session().read_batch(batch)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, output);
        Ok(outputs)
    }
}

pub struct PermutationTestNodeFactory;

impl NodeFactory for PermutationTestNodeFactory {
    fn kind(&self) -> &'static str {
        PERMUTATION_TEST_KIND
    }

    fn desc(&self) -> &'static str {
        "Global group-label permutation test for one aggregate SQL statistic \
         (no spatial/donor/network background model)."
    }

    fn doc(&self) -> &'static str {
        "The supplied statistic_sql must be a single aggregate expression such as \
        AVG(value), STDDEV(value), or SUM(CASE WHEN group = 'A' THEN value ELSE 0 END). \
        group_column values are shuffled across rows for each permutation. The add-one \
        p-value supports greater, less, and two-sided alternatives. The null model is a \
        global two-group label exchangeability assumption over all rows: it does NOT \
        include spatial, donor/subject, or network-constrained background nulls, so \
        clustered or stratified inputs will have anti-conservative p-values."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PermutationTestSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let parsed: PermutationTestSpec = serde_json::from_value(spec)?;
        if parsed.group_column.trim().is_empty() || parsed.statistic_sql.trim().is_empty() {
            return Err("group_column and statistic_sql cannot be empty".into());
        }
        if parsed.n_permutations == 0 || parsed.n_permutations > 100_000 {
            return Err("n_permutations must be between 1 and 100000".into());
        }
        if parsed.statistic_sql.contains(';') {
            return Err("statistic_sql must be an expression, not a statement".into());
        }
        Ok(Box::new(PermutationTestNode::new(parsed)))
    }
}

impl PermutationTestNode {
    pub fn new(spec: PermutationTestSpec) -> Self {
        Self {
            ports: port_layout(),
            spec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn permutes_labels_and_returns_single_row() {
        let session = datafusion::prelude::SessionContext::new();
        let schema = Arc::new(Schema::new(vec![
            Field::new("group", DataType::Utf8, false),
            Field::new("value", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["A", "A", "B", "B"])),
                Arc::new(Float64Array::from(vec![10.0, 12.0, 1.0, 3.0])),
            ],
        )
        .unwrap();
        let frame = session.read_batch(batch).unwrap();
        let spec = PermutationTestSpec {
            group_column: "group".into(),
            statistic_sql: "AVG(CASE WHEN group = 'A' THEN value ELSE -value END)".into(),
            n_permutations: 99,
            random_state: 2,
            alternative: PermutationAlternative::Greater,
        };
        let mut node = PermutationTestNode::new(spec);
        let outputs = node
            .execute(
                &NodeCtx::new(session.runtime_env(), None),
                &[NodeInput::new_dataframe(0, frame)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            1
        );
    }
}
