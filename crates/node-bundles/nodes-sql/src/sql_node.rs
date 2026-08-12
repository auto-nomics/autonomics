use async_trait::async_trait;
use datafusion::common::HashMap;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};

use dag_core::{
    codegen::context::{CodegenCtx, CodegenError, NodeCodegen},
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

#[derive(Debug, Error)]
pub enum SqlNodeError {
    #[error("Invalid input: {message}")]
    InvalidInput { message: String },
    #[error("register upstream DataFrame as view failed")]
    RegisterView(#[source] datafusion::error::DataFusionError),
}

impl SqlNodeError {
    pub fn to_dag_error(self) -> DagError {
        match self {
            SqlNodeError::RegisterView(source) => DagError::DataFusion(source),
            SqlNodeError::InvalidInput { message } => DagError::Schedule(message),
        }
    }
}

impl ::dag_core::dag::NodeError for SqlNodeError {
    fn node_type(&self) -> &str {
        "sql"
    }
}

/// A transform node: registers each upstream input as a named table and runs a
/// SQL query over them. Single output port, variadic input.
///
/// Each upstream input arriving on port `N` is registered as the table
/// `port_{N}`, so the SQL references it as e.g. `FROM port_0` (or
/// `JOIN port_1` for a multi-input node). Input port count is not fixed
/// (`set_fixed_input(false)`); the ports are whatever the wiring connects.
#[derive(Clone)]
pub struct SqlNode {
    meta: NodePorts,
    sql_query: String,
}

#[derive(Debug, JsonSchema, Deserialize)]
pub struct SqlNodeSpec {
    sql_query: String,
}

pub struct SqlNodeFactory {}

/// Static port layout for every [`SqlNode`]: a single untyped output port and
/// a variadic input (input count not fixed, so the scheduler skips
/// over/under-connectivity validation).
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None)
        .set_fixed_input(false)
}

impl NodeFactory for SqlNodeFactory {
    fn kind(&self) -> &'static str {
        "sql"
    }

    fn desc(&self) -> &'static str {
        "Registers upstream DataFrames as named tables (port_0, port_1, …) and executes a SQL query."
    }

    fn doc(&self) -> &'static str {
        "A transform node that registers each upstream input as a named table \
        (port_0, port_1, …) and runs a user-supplied SQL query over them. \
        Supports variadic inputs for multi-table joins and set operations. \
        Single untyped output port."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SqlNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: SqlNodeSpec = serde_json::from_value(spec)?;
        let sql_node = SqlNode::new(node_spec.sql_query);
        Ok(Box::new(sql_node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let node_spec: SqlNodeSpec =
            serde_json::from_value(spec.clone()).map_err(|e| CodegenError::BadSpec {
                kind: "sql".into(),
                source: e,
            })?;

        let out = ctx.output_var.to_string();
        let mut code = vec!["# SQL node — replicating query via sqldf".to_string()];

        // Alias each input variable as port_N so sqldf can resolve table names.
        for (i, input_var) in ctx.input_vars.iter().enumerate() {
            code.push(format!("port_{i} <- {input_var}"));
        }

        // Escape double quotes in the SQL for embedding in an R string.
        let escaped = node_spec
            .sql_query
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        code.push(format!(r#"{out} <- sqldf("{escaped}")"#));

        Ok(NodeCodegen {
            code,
            output_vars: vec![out],
            extra_packages: vec!["sqldf".into()],
        })
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["data.table".into()]
    }
}

impl SqlNode {
    pub fn new(query: String) -> Self {
        Self {
            meta: port_layout(),
            sql_query: query,
        }
    }

    /// Create a [`SqlNode`] from a pre-built [`NodePorts`] (useful for
    /// multi-input join nodes that declare several input ports).
    pub fn from_ports(ports: NodePorts, query: String) -> Self {
        Self {
            meta: ports,
            sql_query: query,
        }
    }

    pub fn set_sql_query(&mut self, query: &str) {
        self.sql_query = query.to_string();
    }
}

#[async_trait]
impl DagNode for SqlNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "sql"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if inputs.is_empty() {
            return Err(SqlNodeError::InvalidInput {
                message: "SqlNode requires at least one upstream input".to_string(),
            }
            .into());
        }

        // Build a fresh, isolated context per execution — no shared CatalogList,
        // so concurrent SqlNodes never collide on `port_N` registrations.
        let ctx = node_ctx.session();

        for inp in inputs {
            // Register each upstream DataFrame under `port_{port}`.
            let view = inp.data.clone().into_view();
            ctx.register_table(format!("port_{}", inp.port), view)
                .map_err(SqlNodeError::RegisterView)?;
        }
        let out = ctx.sql(&self.sql_query).await?;
        let mut res: PortOutputs = HashMap::new();
        res.insert(0, out);
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
            resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(std::path::PathBuf::from("."))),
            global_sem: None,
        }
    }
    use super::*;

    use arrow_array::{ArrayRef, Int32Array, RecordBatch, StringArray, StructArray};
    use arrow_schema::{DataType, Field, Fields, Schema};
    use datafusion::prelude::{DataFrame, SessionContext};
    use std::sync::Arc;

    /// Create a [`SessionContext`] with a simple int32 column `x` (values `[1, 2, 3]`)
    /// registered as table `"src"`, and a [`SqlNode`] wired to the given `sql` query.
    fn setup_test_node(sql: &str) -> (SessionContext, SqlNode, DataFrame) {
        let ctx = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int32, false)]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(Int32Array::from(vec![1, 2, 3]))]).unwrap();
        let df = ctx.read_batch(batch).unwrap();
        ctx.register_table("src", df.clone().into_view()).unwrap();
        let node = SqlNode::new(sql.into());
        (ctx, node, df)
    }

    #[tokio::test]
    async fn test_smoke_register_table() {
        // Sanity check: a DataFrame can be turned into a TableProvider and
        // registered — this exercises the API path used by `execute`. The
        // DAG plumbing / schema wiring lives in higher-level integration
        // tests.
        let (ctx, node, _df) = setup_test_node("SELECT * FROM src WHERE x > 1");
        let result = ctx
            .sql(&node.sql_query)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(result[0].num_rows(), 2);
    }

    #[tokio::test]
    async fn test_data_intake() {
        let (_ctx, mut node, df) = setup_test_node("SELECT * FROM port_0 WHERE x > 1");
        let input = NodeInput { port: 0, data: df };

        // Verify SQL node input mapping: it should use 'port_0' to reference data.
        let output = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        dbg!(output);
    }

    /// Build a RecordBatch carrying a nested `Struct` column
    /// (`info { name: utf8, age: int32 }`) next to a scalar `id`, hand it
    /// to SqlNode, and run a SQL query that filters on one struct
    /// sub-field and projects another.
    ///
    /// Two companion tests cover the two halves of the end-to-end path:
    ///
    /// * `test_struct_df_construction` — locks down the data plumbing:
    ///   a RecordBatch with a Struct column becomes a DataFrame with
    ///   `info: Struct { name, age }`.
    /// * `test_struct_subfield_query` — runs a `WHERE info['age'] > 28`
    ///   / `SELECT info['name']` query through SqlNode and pins the
    ///   correct output. This is the regression test for the original
    ///   gap where SqlNode's per-execution `SessionState` didn't carry
    ///   DataFusion's default features, so the bracket-syntax lowering
    ///   (`RawFieldAccessExpr` → `get_field()`) and the `array_element`
    ///   UDF for list indexing were unreachable. The fix is
    ///   `SessionStateBuilder::new().with_default_features()` in
    ///   `new_isolated_ctx` (shared by all node kinds).
    fn build_struct_dataframe(ctx: &SessionContext) -> DataFrame {
        let info_fields = Fields::from(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("age", DataType::Int32, false),
        ]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("info", DataType::Struct(info_fields.clone()), false),
        ]));

        let name_array: ArrayRef = Arc::new(StringArray::from(vec!["Alice", "Bob", "Charlie"]));
        let age_array: ArrayRef = Arc::new(Int32Array::from(vec![30, 25, 35]));
        let info_array =
            StructArray::try_new(info_fields, vec![name_array, age_array], None).unwrap();

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(info_array),
            ],
        )
        .unwrap();

        ctx.read_batch(batch).unwrap()
    }

    #[tokio::test]
    async fn test_struct_df_construction() {
        let ctx = SessionContext::new();
        let df = build_struct_dataframe(&ctx);
        let schema = df.schema();

        assert_eq!(schema.fields().len(), 2);
        let info_field = schema
            .field_with_name(None, "info")
            .expect("info column must exist");
        assert!(
            matches!(info_field.data_type(), DataType::Struct(_)),
            "info must be a Struct column, got {:?}",
            info_field.data_type(),
        );
        let DataType::Struct(info_fields) = info_field.data_type() else {
            unreachable!("asserted above");
        };
        assert_eq!(
            info_fields
                .iter()
                .map(|f| f.name().as_str())
                .collect::<Vec<_>>(),
            vec!["name", "age"],
        );
    }

    #[tokio::test]
    async fn test_struct_subfield_query() {
        let ctx = SessionContext::new();
        let df = build_struct_dataframe(&ctx);

        // Filter on `info['age']` and project `info['name']`. Bracket
        // syntax parses to `RawFieldAccessExpr`, which `FieldAccessPlanner`
        // lowers to `get_field(...)`; both live behind DataFusion's
        // `with_default_features()`, which `new_isolated_ctx` must
        // install into the per-execution SessionState — see the
        // `.with_default_features()` call in `registry::new_isolated_ctx`.
        let sql = "SELECT id, info.name AS name \
                   FROM port_0 \
                   WHERE info['age'] > 28 \
                   ORDER BY id";
        let mut node = SqlNode::new(sql.into());
        let input = NodeInput { port: 0, data: df };
        let outputs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let batches = outputs.get(&0).unwrap().clone().collect().await.unwrap();

        assert_eq!(batches.len(), 1, "expected a single RecordBatch");
        let rb = &batches[0];
        assert_eq!(rb.num_rows(), 2, "expected 2 rows where info.age > 28");

        let ids = rb.column(0).as_any().downcast_ref::<Int32Array>().unwrap();
        assert_eq!(ids.values(), &[1, 3]);

        let names = rb.column(1).as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(names.value(0), "Alice");
        assert_eq!(names.value(1), "Charlie");
    }

    /// Create two SqlNodes that each produce a DataFrame, then feed both
    /// outputs into a third SqlNode that JOINs them. This validates the
    /// full multi-input → SQL join path: each upstream result arrives on a
    /// distinct port (`port_0`, `port_1`) and the downstream query can
    /// reference both.
    #[tokio::test]
    async fn test_two_sql_nodes_into_one() {
        let ctx = SessionContext::new();

        // --- upstream node A: produces an `id, name` table ---
        let a_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let a_batch = RecordBatch::try_new(
            a_schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["Alice", "Bob", "Charlie"])),
            ],
        )
        .unwrap();
        let a_df = ctx.read_batch(a_batch).unwrap();

        let mut node_a = SqlNode::new("SELECT * FROM port_0 WHERE id <= 2".into());
        let a_out = node_a
            .execute(
                &node_ctx(),
                &[NodeInput {
                    port: 0,
                    data: a_df,
                }],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let a_result: DataFrame = a_out.get(&0).unwrap().clone();

        // --- upstream node B: produces an `id, score` table ---
        let b_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("score", DataType::Int32, false),
        ]));
        let b_batch = RecordBatch::try_new(
            b_schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 4])),
                Arc::new(Int32Array::from(vec![90, 85, 70])),
            ],
        )
        .unwrap();
        let b_df = ctx.read_batch(b_batch).unwrap();

        let mut node_b = SqlNode::new("SELECT * FROM port_0 WHERE score > 80".into());
        let b_out = node_b
            .execute(
                &node_ctx(),
                &[NodeInput {
                    port: 0,
                    data: b_df,
                }],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let b_result: DataFrame = b_out.get(&0).unwrap().clone();

        // --- downstream node C: JOINs both outputs ---
        let mut node_c = SqlNode::new(
            "SELECT a.id, a.name, b.score \
             FROM port_0 AS a JOIN port_1 AS b ON a.id = b.id \
             ORDER BY a.id"
                .into(),
        );
        let c_out = node_c
            .execute(
                &node_ctx(),
                &[
                    NodeInput {
                        port: 0,
                        data: a_result,
                    },
                    NodeInput {
                        port: 1,
                        data: b_result,
                    },
                ],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let batches = c_out.get(&0).unwrap().clone().collect().await.unwrap();
        assert_eq!(batches.len(), 1);

        let rb = &batches[0];
        // node_a filtered id <= 2 → [1, 2], node_b filtered score > 80 → [1(90), 2(85)]
        // JOIN on id yields both rows.
        assert_eq!(rb.num_rows(), 2);

        let ids = rb.column(0).as_any().downcast_ref::<Int32Array>().unwrap();
        assert_eq!(ids.values(), &[1, 2]);

        let names = rb.column(1).as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(names.value(0), "Alice");
        assert_eq!(names.value(1), "Bob");

        let scores = rb.column(2).as_any().downcast_ref::<Int32Array>().unwrap();
        assert_eq!(scores.values(), &[90, 85]);
    }
}
