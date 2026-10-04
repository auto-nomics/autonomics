//! Hypergeometric ORA over two upstream DataFrames.

use arrow_array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use statrs::distribution::{DiscreteCDF, Hypergeometric};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::table_transforms::utf8_values;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

pub const HYPERGEOMETRIC_ORA_ONDF_KIND: &str = "hypergeometric_ora_ondf";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HypergeometricOraOnDfSpec {
    #[serde(default = "default_gene_column")]
    pub gene_column: String,
    #[serde(default = "default_set_id_column")]
    pub set_id_column: String,
    #[serde(default = "default_set_gene_column")]
    pub set_gene_column: String,
    /// Optional SQL predicate over input port 0, for example `pvalue < 0.05`.
    #[serde(default)]
    pub hit_filter_sql: Option<String>,
    #[serde(default = "default_min_set_size")]
    pub min_set_size: usize,
    #[serde(default = "default_min_hits")]
    pub min_hits: usize,
}

fn default_gene_column() -> String {
    "gene".into()
}
fn default_set_id_column() -> String {
    "set_id".into()
}
fn default_set_gene_column() -> String {
    "gene".into()
}
fn default_min_set_size() -> usize {
    2
}
fn default_min_hits() -> usize {
    1
}

#[derive(Clone)]
pub struct HypergeometricOraOnDfNode {
    ports: NodePorts,
    spec: HypergeometricOraOnDfSpec,
}

fn node_error(message: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: HYPERGEOMETRIC_ORA_ONDF_KIND.into(),
        msg: message.into(),
    }
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(
            None,
            dag_core::value::PortType::DataFrame,
            "aggregate_result",
        )
        .add_input_port_of_type_with_label(
            None,
            dag_core::value::PortType::DataFrame,
            "gene_set_mapping",
        )
        .add_output_port(None)
}

impl HypergeometricOraOnDfNode {
    pub fn new(spec: HypergeometricOraOnDfSpec) -> Self {
        Self {
            ports: port_layout(),
            spec,
        }
    }

    async fn query_genes(
        &self,
        ctx: &NodeCtx,
        frame: datafusion::prelude::DataFrame,
    ) -> Result<BTreeSet<String>, DagError> {
        let predicate = self
            .spec
            .hit_filter_sql
            .as_deref()
            .map(|value| format!(" WHERE ({value})"))
            .unwrap_or_default();
        let sql = format!(
            "SELECT DISTINCT \"{}\" AS gene FROM __ora_input{predicate}",
            self.spec.gene_column
        );
        let session = ctx.session();
        session
            .register_table("__ora_input", frame.into_view())
            .map_err(|error| node_error(format!("cannot register aggregate result: {error}")))?;
        let batches = session
            .sql(&sql)
            .await
            .map_err(|error| node_error(format!("invalid hit_filter_sql: {error}")))?
            .collect()
            .await
            .map_err(|error| node_error(format!("cannot evaluate hit filter: {error}")))?;
        let mut genes = BTreeSet::new();
        for batch in batches {
            for gene in utf8_values(batch.column(0))?.into_iter().flatten() {
                genes.insert(gene);
            }
        }
        Ok(genes)
    }

    async fn gene_sets(
        &self,
        batches: &[RecordBatch],
    ) -> Result<BTreeMap<String, BTreeSet<String>>, DagError> {
        let fields = batches
            .first()
            .ok_or_else(|| node_error("gene-set mapping has no batches"))?
            .schema()
            .fields()
            .clone();
        let (set_index, _) = fields.find(&self.spec.set_id_column).ok_or_else(|| {
            node_error(format!(
                "set column `{}` is absent",
                self.spec.set_id_column
            ))
        })?;
        let (gene_index, _) = fields.find(&self.spec.set_gene_column).ok_or_else(|| {
            node_error(format!(
                "set gene column `{}` is absent",
                self.spec.set_gene_column
            ))
        })?;
        let mut sets = BTreeMap::new();
        for batch in batches {
            let set_values = utf8_values(batch.column(set_index))?;
            let gene_values = utf8_values(batch.column(gene_index))?;
            for (set, gene) in set_values.into_iter().zip(gene_values) {
                if let (Some(set), Some(gene)) = (set, gene) {
                    sets.entry(set).or_insert_with(BTreeSet::new).insert(gene);
                }
            }
        }
        if sets.is_empty() {
            return Err(node_error("gene-set mapping has no usable rows"));
        }
        Ok(sets)
    }
}

#[async_trait]
impl DagNode for HypergeometricOraOnDfNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        HYPERGEOMETRIC_ORA_ONDF_KIND
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
        if inputs.len() != 2 || inputs[0].port != 0 || inputs[1].port != 1 {
            return Err(node_error(
                "hypergeometric_ora_ondf requires aggregate_result on port 0 and gene_set_mapping on port 1",
            ));
        }
        let query = self
            .query_genes(ctx, inputs[0].dataframe()?.clone())
            .await?;
        let mapping_batches = inputs[1].dataframe()?.clone().collect().await?;
        let sets = self.gene_sets(&mapping_batches).await?;
        let background = sets.values().flatten().cloned().collect::<BTreeSet<_>>();
        let population = background.len();
        let draws = query.len();
        if population == 0 || draws == 0 {
            return Err(node_error(
                "query genes and annotation background cannot be empty",
            ));
        }

        let mut rows = Vec::new();
        for (set_id, genes) in sets {
            let set_n = genes.len();
            if set_n < self.spec.min_set_size {
                continue;
            }
            let hits = query.intersection(&genes).count();
            if hits < self.spec.min_hits {
                continue;
            }
            let distribution = Hypergeometric::new(population as u64, set_n as u64, draws as u64)
                .map_err(|error| {
                node_error(format!("invalid hypergeometric parameters: {error}"))
            })?;
            let pvalue = if hits == 0 {
                1.0
            } else {
                1.0 - distribution.cdf(hits as u64 - 1)
            };
            let expected = draws as f64 * set_n as f64 / population as f64;
            rows.push((
                set_id,
                hits,
                set_n,
                expected,
                // Fold enrichment = observed / expected (was inverted: node
                // audit 2026-10-04, P1 — hits=2, expected=1 reported 0.5).
                hits as f64 / expected,
                pvalue,
            ));
        }

        let mut indexed = rows
            .iter()
            .enumerate()
            .map(|(index, row)| (index, row.5))
            .collect::<Vec<_>>();
        indexed.sort_by(|left, right| left.1.total_cmp(&right.1));
        let mut adjusted = vec![None::<f64>; rows.len()];
        for rank in (1..=indexed.len()).rev() {
            let position = rank - 1;
            let (index, pvalue) = indexed[position];
            let value = (pvalue * rows.len() as f64 / rank as f64).min(1.0);
            let value = if position + 1 < indexed.len() {
                value.min(adjusted[indexed[position + 1].0].unwrap_or(1.0))
            } else {
                value
            };
            adjusted[index] = Some(value);
        }

        let fields = vec![
            Field::new("set_id", DataType::Utf8, false),
            Field::new("hit_n", DataType::UInt32, false),
            Field::new("set_n", DataType::UInt32, false),
            Field::new("query_n", DataType::UInt32, false),
            Field::new("background_n", DataType::UInt32, false),
            Field::new("expected_hits", DataType::Float64, false),
            Field::new("fold_enrichment", DataType::Float64, false),
            Field::new("pvalue", DataType::Float64, false),
            Field::new("pvalue_adj", DataType::Float64, false),
        ];
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(
                rows.iter().map(|row| row.0.clone()).collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|row| row.1 as u32).collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|row| row.2 as u32).collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(vec![draws as u32; rows.len()])),
            Arc::new(UInt32Array::from(vec![population as u32; rows.len()])),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.3).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.4).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.5).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                adjusted
                    .into_iter()
                    .map(|value| value.unwrap_or(f64::NAN))
                    .collect::<Vec<_>>(),
            )),
        ];
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .map_err(|error| node_error(format!("cannot build ORA result: {error}")))?;
        let output = ctx.session().read_batch(batch)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, output);
        Ok(outputs)
    }
}

pub struct HypergeometricOraOnDfNodeFactory;

impl NodeFactory for HypergeometricOraOnDfNodeFactory {
    fn kind(&self) -> &'static str {
        HYPERGEOMETRIC_ORA_ONDF_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs hypergeometric over-representation analysis directly on DataFrames."
    }

    fn doc(&self) -> &'static str {
        "Port 0 is any SQL aggregate result containing a gene column; \
        hit_filter_sql selects hits (for example `pvalue < 0.05`). Port 1 is a \
        long set mapping containing set_id and gene. The annotation universe is \
        the union of mapped genes; p-values are one-sided greater and BH-adjusted."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HypergeometricOraOnDfSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let parsed: HypergeometricOraOnDfSpec = serde_json::from_value(spec)?;
        if parsed.gene_column.trim().is_empty()
            || parsed.set_id_column.trim().is_empty()
            || parsed.set_gene_column.trim().is_empty()
        {
            return Err("gene, set_id, and set gene columns cannot be empty".into());
        }
        if let Some(filter) = parsed.hit_filter_sql.as_deref()
            && filter.contains(';')
        {
            return Err("hit_filter_sql must be an expression, not a statement".into());
        }
        Ok(Box::new(HypergeometricOraOnDfNode::new(parsed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_ora_from_dataframes() {
        let session = datafusion::prelude::SessionContext::new();
        let result_schema = Arc::new(Schema::new(vec![
            Field::new("gene", DataType::Utf8, false),
            Field::new("pvalue", DataType::Float64, false),
        ]));
        let result_batch = RecordBatch::try_new(
            result_schema,
            vec![
                Arc::new(StringArray::from(vec!["A", "B", "C"])),
                Arc::new(Float64Array::from(vec![0.01, 0.02, 0.5])),
            ],
        )
        .unwrap();
        let set_schema = Arc::new(Schema::new(vec![
            Field::new("set_id", DataType::Utf8, false),
            Field::new("gene", DataType::Utf8, false),
        ]));
        let set_batch = RecordBatch::try_new(
            set_schema,
            vec![
                Arc::new(StringArray::from(vec!["S", "S", "S"])),
                Arc::new(StringArray::from(vec!["A", "B", "D"])),
            ],
        )
        .unwrap();
        let spec = HypergeometricOraOnDfSpec {
            gene_column: "gene".into(),
            set_id_column: "set_id".into(),
            set_gene_column: "gene".into(),
            hit_filter_sql: Some("pvalue < 0.05".into()),
            min_set_size: 2,
            min_hits: 1,
        };
        let mut node = HypergeometricOraOnDfNode::new(spec);
        let outputs = node
            .execute(
                &NodeCtx::new(session.runtime_env(), None),
                &[
                    NodeInput::new_dataframe(0, session.read_batch(result_batch).unwrap()),
                    NodeInput::new_dataframe(1, session.read_batch(set_batch).unwrap()),
                ],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            1
        );
    }

    /// Fold enrichment must be observed/expected. Fixture: query {A, B} of a
    /// background {A, B, C, D, E, F, G, H}, set S1 = {A, B, D} →
    /// expected = 2·3/8 = 0.75, hits = 2, fold = 2/0.75 ≈ 2.667. The
    /// pre-fix code emitted expected/hits = 0.375 (node audit 2026-10-04).
    #[tokio::test]
    async fn fold_enrichment_is_observed_over_expected() {
        use arrow_array::Float64Array;
        let session = datafusion::prelude::SessionContext::new();
        let result_schema = Arc::new(Schema::new(vec![
            Field::new("gene", DataType::Utf8, false),
            Field::new("pvalue", DataType::Float64, false),
        ]));
        let result_batch = RecordBatch::try_new(
            result_schema,
            vec![
                Arc::new(StringArray::from(vec!["A", "B", "C"])),
                Arc::new(Float64Array::from(vec![0.01, 0.02, 0.9])),
            ],
        )
        .unwrap();
        let set_schema = Arc::new(Schema::new(vec![
            Field::new("set_id", DataType::Utf8, false),
            Field::new("gene", DataType::Utf8, false),
        ]));
        let set_batch = RecordBatch::try_new(
            set_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["S1", "S1", "S1"])),
                Arc::new(StringArray::from(vec!["A", "B", "D"])),
            ],
        )
        .unwrap();
        // A second set supplies the rest of the background so the
        // hypergeometric population is 8, not 3.
        let set2_batch = RecordBatch::try_new(
            set_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["S2", "S2", "S2", "S2", "S2"])),
                Arc::new(StringArray::from(vec!["C", "E", "F", "G", "H"])),
            ],
        )
        .unwrap();
        let sets_df = session
            .read_batch(set_batch)
            .unwrap()
            .union(session.read_batch(set2_batch).unwrap())
            .unwrap();

        let spec = HypergeometricOraOnDfSpec {
            gene_column: "gene".into(),
            set_id_column: "set_id".into(),
            set_gene_column: "gene".into(),
            hit_filter_sql: Some("pvalue < 0.05".into()),
            min_set_size: 2,
            min_hits: 1,
        };
        let mut node = HypergeometricOraOnDfNode::new(spec);
        let outputs = node
            .execute(
                &NodeCtx::new(session.runtime_env(), None),
                &[
                    NodeInput::new_dataframe(0, session.read_batch(result_batch).unwrap()),
                    NodeInput::new_dataframe(1, sets_df),
                ],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let batches = outputs.dataframe(0).unwrap().clone().collect().await.unwrap();
        let s1 = batches
            .iter()
            .flat_map(|b| {
                let ids = b
                    .column_by_name("set_id")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                    .unwrap();
                let folds = b
                    .column_by_name("fold_enrichment")
                    .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
                    .unwrap();
                (0..b.num_rows())
                    .filter(|&i| ids.value(i) == "S1")
                    .map(|i| folds.value(i))
                    .collect::<Vec<_>>()
            })
            .next()
            .unwrap();
        let want = 2.0f64 / 0.75f64;
        assert!(
            (s1 - want).abs() < 1e-12,
            "fold_enrichment must be hits/expected: got {s1}, want {want}"
        );
    }
}
