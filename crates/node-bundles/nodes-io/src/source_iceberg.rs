//! Iceberg source node: brings an Iceberg table into the DAG as a `DataFrame`.
//!
//! An [`IcebergSourceNode`] has no inputs and produces exactly one output. The
//! table is resolved by identifier (`namespace.table`) through the `iceberg`
//! catalog registered on the engine context. Symmetric to
//! [`crate::sink_iceberg::IcebergSinkNode`] for the Iceberg case.

use async_trait::async_trait;
use datafusion::common::HashMap;
use datafusion::prelude::DataFrame;
use schemars::JsonSchema;
use serde::Deserialize;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::DagError,
    dag::graph::PortOutputs,
    registry::{NodeCtx, NodeFactory},
};

#[derive(Clone)]
pub struct IcebergSourceNode {
    meta: NodePorts,
    ident: String,
}

impl IcebergSourceNode {
    pub fn new(ident: String) -> Self {
        // A source has no inputs and a single output port.
        Self {
            meta: port_layout(),
            ident,
        }
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct IcebergSourceNodeSpec {
    /// An Iceberg table identifier (`namespace.table`), resolved through the
    /// `iceberg` catalog registered on the engine context.
    pub ident: String,
}

pub struct IcebergSourceNodeFactory {}

/// Static port layout for every [`IcebergSourceNode`]: no inputs, a single
/// untyped output port (schema discovered from the source at runtime).
fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for IcebergSourceNodeFactory {
    fn kind(&self) -> &'static str {
        "source_iceberg"
    }

    fn desc(&self) -> &'static str {
        "Reads an Iceberg table into the DAG as a DataFrame."
    }

    fn doc(&self) -> &'static str {
        "An Iceberg data source node that reads an Iceberg table into the DAG \
        as a DataFrame, resolved by its `namespace.table` identifier through the \
        `iceberg` catalog registered on the engine context. No input ports; one \
        untyped output port."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schemars::schema_for!(IcebergSourceNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: IcebergSourceNodeSpec = serde_json::from_value(spec)?;
        let node = IcebergSourceNode::new(node_spec.ident);
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<IcebergSourceNodeSpec>(spec, "source_iceberg")?;
        let out = ctx.output_var.to_string();
        let code = vec![
            format!(
                "# NOTE: Iceberg table '{}' cannot be read directly from R.",
                s.ident
            ),
            format!("# Pre-export to CSV/Parquet and replace the line below:"),
            format!(
                r#"stop("export iceberg table '{}' to a file before running this script")"#,
                s.ident
            ),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
}

#[async_trait]
impl DagNode for IcebergSourceNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_iceberg"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &dag_core::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let ctx = node_ctx.session();
        // The iceberg catalog is registered under "iceberg"; qualify the
        // identifier so DataFusion resolves it through that catalog.
        let df = ctx
            .sql(&format!("SELECT * FROM iceberg.{}", self.ident))
            .await?;

        // Promote all Float32 columns to Float64. Iceberg tables may store
        // numeric columns at reduced precision (Float32); when a downstream
        // SQL node JOINs or UNIONs them with Float64 data from another source,
        // DataFusion's type coercion can fail with "Can't convert datum from
        // double type to float type" during `collect()`. Normalising at the
        // ingestion boundary prevents these errors throughout the pipeline.
        let df = promote_floats(df)?;

        let mut res: PortOutputs = HashMap::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Cast every Float32 column in `df` to Float64, leaving all other columns
/// unchanged. Returns the modified DataFrame.
///
/// Iceberg tables frequently store numeric columns as Float32 to save space.
/// When downstream SQL nodes JOIN or UNION these tables with Float64 data
/// from other sources (or introduce Float64 literals via COALESCE / CASE),
/// DataFusion's type coercion can fail during `collect()` with
/// "Can't convert datum from double type to float type." Promoting at the
/// source avoids the mismatch before it reaches any downstream node.
pub(crate) fn promote_floats(mut df: DataFrame) -> Result<DataFrame, DagError> {
    use arrow_schema::DataType;
    use datafusion::logical_expr::cast;
    use datafusion::prelude::col;

    let float32_cols: Vec<String> = df
        .schema()
        .fields()
        .iter()
        .filter(|f| matches!(f.data_type(), DataType::Float32))
        .map(|f| f.name().to_string())
        .collect();

    for name in &float32_cols {
        df = df.with_column(name, cast(col(name), DataType::Float64))?;
    }

    if !float32_cols.is_empty() {
        tracing::debug!(
            columns = ?float32_cols,
            "promoted {} Float32 column(s) to Float64 on source read",
            float32_cols.len(),
        );
    }

    Ok(df)
}

#[cfg(test)]
mod tests {
    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
            resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(
                std::path::PathBuf::from("."),
            )),
            global_sem: None,
        }
    }
    use super::*;
    use datalake::Datalake;

    #[tokio::test]
    #[ignore = "e2e test"]
    async fn test_load_from_iceberg() {
        let _ctx = Datalake::default().get_ctx().await.unwrap();
        let _provider = Datalake::default().get_provider().await.unwrap();
        let mut node = IcebergSourceNode::new("gwas.gwas_study".to_string());
        let res = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = res.get(&0).unwrap().clone();
        df.limit(0, Some(10)).unwrap().show().await.unwrap();
    }

    /// Verify that `promote_floats` widens Float32 columns to Float64 while
    /// leaving other types (Int32, Utf8, Float64) untouched.
    #[tokio::test]
    async fn promote_floats_widens_f32_only() {
        use arrow_array::{Float32Array, Float64Array, Int32Array, RecordBatch, StringArray};
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("f32_col", DataType::Float32, true),
            Field::new("f64_col", DataType::Float64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
                Arc::new(Float32Array::from(vec![1.0_f32, 2.0, 3.0])),
                Arc::new(Float64Array::from(vec![1.0_f64, 2.0, 3.0])),
            ],
        )
        .unwrap();

        let ctx = datafusion::prelude::SessionContext::new();
        let df = ctx.read_batch(batch).unwrap();

        // Before: f32_col is Float32.
        assert_eq!(
            df.schema()
                .field_with_name(None, "f32_col")
                .unwrap()
                .data_type(),
            &DataType::Float32,
        );

        let df = promote_floats(df).unwrap();

        // After: f32_col promoted to Float64.
        assert_eq!(
            df.schema()
                .field_with_name(None, "f32_col")
                .unwrap()
                .data_type(),
            &DataType::Float64,
        );
        // Other columns unchanged.
        assert_eq!(
            df.schema().field_with_name(None, "id").unwrap().data_type(),
            &DataType::Int32,
        );
        assert_eq!(
            df.schema()
                .field_with_name(None, "name")
                .unwrap()
                .data_type(),
            &DataType::Utf8,
        );
        assert_eq!(
            df.schema()
                .field_with_name(None, "f64_col")
                .unwrap()
                .data_type(),
            &DataType::Float64,
        );

        // Values survive the cast.
        let batches = df.collect().await.unwrap();
        let promoted = batches[0]
            .column_by_name("f32_col")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(promoted.values(), &[1.0_f64, 2.0, 3.0]);
    }

    /// `promote_floats` on a DataFrame with no Float32 columns is a no-op.
    #[tokio::test]
    async fn promote_floats_noop_when_already_f64() {
        use arrow_array::{Float64Array, Int64Array, RecordBatch};
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, false),
            Field::new("y", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(Float64Array::from(vec![1.0, 2.0])),
            ],
        )
        .unwrap();
        let ctx = datafusion::prelude::SessionContext::new();
        let df = ctx.read_batch(batch).unwrap();
        let df = promote_floats(df).unwrap();

        // All types unchanged.
        assert_eq!(
            df.schema().field_with_name(None, "x").unwrap().data_type(),
            &DataType::Int64,
        );
        assert_eq!(
            df.schema().field_with_name(None, "y").unwrap().data_type(),
            &DataType::Float64,
        );
    }
}
