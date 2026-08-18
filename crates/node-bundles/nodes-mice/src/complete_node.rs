//! `mice_complete` DAG node — extract completed dataset from a `mids` object.
//!
//! Reads a long-format table produced by the `mice` orchestrator and emits
//! a single completed DataFrame for the requested imputation index.

use std::sync::Arc;

use arrow_array::{Array, BooleanArray, Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::codegen::context::{CodegenCtx, CodegenError, NodeCodegen};
use dag_core::codegen::helpers::*;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::NodeFactory;
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::NodeCtx,
};

use crate::common::test_node_ctx;
use crate::error::MiceNodeError;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MiceCompleteNodeSpec {
    /// Which imputation to extract (1-indexed). Default 1.
    #[serde(default = "default_imputation")]
    pub imputation: usize,
}

fn default_imputation() -> usize {
    1
}

#[derive(Clone)]
pub struct MiceCompleteNode {
    meta: NodePorts,
    spec: MiceCompleteNodeSpec,
}

pub struct MiceCompleteNodeFactory;

fn port_layout() -> NodePorts {
    // Output schema is dynamic; we declare a single Float64 column with a
    // fixed name. Downstream nodes reading from the same `mice_complete`
    // node see the columns they wrote into the long-format input.
    let schema: SchemaRef = Arc::new(Schema::new(vec![Field::new(
        "first_row",
        DataType::Float64,
        true,
    )]));
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(schema))
}

impl NodeFactory for MiceCompleteNodeFactory {
    fn kind(&self) -> &'static str {
        "mice_complete"
    }
    fn desc(&self) -> &'static str {
        "Extract the requested imputation from a MICE long-format result."
    }
    fn doc(&self) -> &'static str {
        "Reads the long-format table produced by the `mice` orchestrator and \
        returns the requested imputation's row (e.g. the first completed \
        dataset for `imputation = 1`)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MiceCompleteNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: MiceCompleteNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MiceCompleteNode {
            meta: port_layout(),
            spec: s,
        }))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let s = parse_spec::<MiceCompleteNodeSpec>(spec, "mice_complete")?;
        let out = ctx.output_var.to_string();
        let input = input_0(ctx).to_string();
        let code = vec![
            "# Extract completed data".to_string(),
            format!("{out} <- {input}[{input}$.imp == {}, ]", s.imputation),
            format!("print({out})"),
        ];
        Ok(NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["mice".into()]
    }
}

#[async_trait]
impl DagNode for MiceCompleteNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "mice_complete"
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
            .ok_or_else(|| MiceNodeError::EmptyInput)
            .map_err(|e| DagError::NodeError {
                node_type: "mice_complete".into(),
                msg: e.to_string(),
            })?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| MiceNodeError::Collect(e.to_string()))
            .map_err(|e| DagError::NodeError {
                node_type: "mice_complete".into(),
                msg: e.to_string(),
            })?;
        if batches.is_empty() {
            return Err(MiceNodeError::EmptyInput).map_err(|e| DagError::NodeError {
                node_type: "mice_complete".into(),
                msg: e.to_string(),
            })?;
        }
        let schema = batches[0].schema();
        let imp_idx = schema
            .index_of("imp_num")
            .map_err(|_| MiceNodeError::MissingColumn {
                name: ".imp".into(),
            })
            .map_err(|e| DagError::NodeError {
                node_type: "mice_complete".into(),
                msg: e.to_string(),
            })?;
        // Collect .imp column and find first matching row count = (n rows in
        // this imputation) — we filter rows where .imp == imputation.
        let target = self.spec.imputation as i64;
        let mut out_rows: Vec<RecordBatch> = Vec::new();
        for batch in &batches {
            let imp_col = batch
                .column(imp_idx)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| MiceNodeError::InvalidSpec(".imp not Int64".into()))
                .map_err(|e| DagError::NodeError {
                    node_type: "mice_complete".into(),
                    msg: e.to_string(),
                })?;
            let mask_values: Vec<bool> = imp_col.iter().map(|v| v == Some(target)).collect();
            let mask = BooleanArray::from(mask_values);
            // Use arrow's filter via boolean mask
            let mut new_columns: Vec<Arc<dyn Array>> = Vec::new();
            for c in batch.columns() {
                let filtered = arrow_select::filter::filter(c.as_ref(), &mask).map_err(|e| {
                    DagError::NodeError {
                        node_type: "mice_complete".into(),
                        msg: format!("filter: {e}"),
                    }
                })?;
                new_columns.push(filtered);
            }
            let rb = RecordBatch::try_new(schema.clone(), new_columns).map_err(|e| {
                DagError::NodeError {
                    node_type: "mice_complete".into(),
                    msg: format!("Arrow: {e}"),
                }
            })?;
            out_rows.push(rb);
        }
        let merged = arrow::compute::concat_batches(&schema, &out_rows).map_err(|e| {
            DagError::NodeError {
                node_type: "mice_complete".into(),
                msg: format!("concat: {e}"),
            }
        })?;
        let _ = Float64Array::from(vec![1.0]);
        let ctx = node_ctx.session();
        let df = ctx.read_batch(merged).map_err(|e| DagError::NodeError {
            node_type: "mice_complete".into(),
            msg: format!("read_batch: {e}"),
        })?;
        let mut out: PortOutputs = PortOutputs::new();
        out.insert(0, df);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Float64Array;
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    fn make_long_batch() -> arrow_array::RecordBatch {
        // 6 rows: 3 ids × 2 imputations.
        let schema = Arc::new(Schema::new(vec![
            Field::new("imp_num", DataType::Int64, false),
            Field::new("id_num", DataType::Int64, false),
            Field::new("y", DataType::Float64, false),
        ]));
        let imp = vec![1, 1, 1, 2, 2, 2];
        let id = vec![1, 2, 3, 1, 2, 3];
        let y = vec![10.0, 20.0, 30.0, 11.0, 21.0, 31.0];
        arrow_array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(imp)),
                Arc::new(Int64Array::from(id)),
                Arc::new(Float64Array::from(y)),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_mice_complete_basic() {
        let batch = make_long_batch();
        let spec = MiceCompleteNodeSpec { imputation: 2 };
        let mut node = MiceCompleteNode {
            meta: port_layout(),
            spec,
        };
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(batch)
                .unwrap(),
        );
        let outs = node
            .execute(
                &test_node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outs.dataframe(0).unwrap().clone();
        let batches = df.collect().await.unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 3);
        // Check that .imp == 2 for all rows.
        for b in &batches {
            let imp_col = b.column(0).as_any().downcast_ref::<Int64Array>().unwrap();
            for v in imp_col.iter() {
                assert_eq!(v.unwrap(), 2);
            }
        }
    }
}
