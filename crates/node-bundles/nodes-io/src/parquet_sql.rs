//! DataFusion SQL node with Parquet File inputs and outputs.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileRef, PortType};
use datafusion::dataframe::DataFrameWriteOptions;
use datafusion::prelude::ParquetReadOptions;

pub const DATAFUSION_SQL_KIND: &str = "datafusion_sql";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DataFusionSqlSpec {
    /// SQL query. The input Parquet file is registered as `table_name`.
    pub sql: String,
    #[serde(default = "default_table_name")]
    pub table_name: String,
    /// Absolute local, `file://`, or `vfs://` output Parquet path.
    pub output_path: String,
}

fn default_table_name() -> String {
    "input".into()
}

pub struct DataFusionSqlNode {
    ports: NodePorts,
    sql: String,
    table_name: String,
    output_path: String,
}

impl DataFusionSqlNode {
    pub fn new(sql: String, table_name: String, output_path: String) -> Self {
        Self {
            ports: port_layout(),
            sql,
            table_name,
            output_path,
        }
    }
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::File, "parquet")
        .add_output_port_of_type(None, PortType::File)
}

pub fn validate(spec: &DataFusionSqlSpec) -> Result<(), String> {
    if spec.sql.trim().is_empty() {
        return Err("sql cannot be empty".into());
    }
    if spec.table_name.is_empty()
        || !spec
            .table_name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        || spec
            .table_name
            .chars()
            .next()
            .is_none_or(|c| c.is_ascii_digit())
    {
        return Err("table_name must be a nonempty SQL identifier".into());
    }
    if !spec.output_path.starts_with('/')
        && !spec.output_path.starts_with("vfs://")
        && !spec.output_path.starts_with("file://")
    {
        return Err("output_path must be an absolute local, file://, or vfs:// path".into());
    }
    if !spec.output_path.to_ascii_lowercase().ends_with(".parquet") {
        return Err("output_path must end in .parquet".into());
    }
    Ok(())
}

#[async_trait]
impl DagNode for DataFusionSqlNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            sql: self.sql.clone(),
            table_name: self.table_name.clone(),
            output_path: self.output_path.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        DATAFUSION_SQL_KIND
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
        let input = inputs.first().ok_or_else(|| {
            DagError::Schedule(format!("{DATAFUSION_SQL_KIND} requires a Parquet input"))
        })?;
        let file = input.file_value()?;
        if let Some(format) = file.format.as_deref()
            && !format.eq_ignore_ascii_case("parquet")
        {
            return Err(DagError::Schedule(
                "datafusion_sql requires a Parquet File input".into(),
            ));
        }
        let input_path = crate::file_to_dataframe::source_path(
            ctx,
            &crate::file_to_dataframe::normalize_path(&file.path),
        );
        let output_path = crate::file_to_dataframe::normalize_path(&self.output_path);
        let session = ctx.session();
        session
            .register_parquet(&self.table_name, &input_path, ParquetReadOptions::default())
            .await
            .map_err(|error| {
                DagError::Schedule(format!(
                    "cannot register Parquet input `{input_path}`: {error}"
                ))
            })?;
        let result = session.sql(&self.sql).await.map_err(DagError::DataFusion)?;
        result
            .write_parquet(
                &output_path,
                DataFrameWriteOptions::new().with_single_file_output(true),
                None::<datafusion::config::TableParquetOptions>,
            )
            .await
            .map_err(|error| {
                DagError::Schedule(format!(
                    "cannot write Parquet output `{output_path}`: {error}"
                ))
            })?;
        let output_file = FileRef::local(&output_path, Some("parquet".into()))
            .unwrap_or_else(|_| FileRef::new(output_path.clone(), Some("parquet".into())));
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, output_file);
        Ok(outputs)
    }
}

pub struct DataFusionSqlNodeFactory {}

impl NodeFactory for DataFusionSqlNodeFactory {
    fn kind(&self) -> &'static str {
        DATAFUSION_SQL_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs DataFusion SQL over an upstream Parquet File and writes one Parquet File."
    }

    fn doc(&self) -> &'static str {
        "The input File port must refer to Parquet and is registered with the configured \
        table name (`input` by default). The SQL result is materialized as a single-file \
        Parquet output. Use this node after h5ad_obs_to_parquet, not on the opaque H5AD main path."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DataFusionSqlSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: DataFusionSqlSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(Box::new(DataFusionSqlNode::new(
            spec.sql,
            spec.table_name,
            spec.output_path,
        )))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: DataFusionSqlSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_identifier_table_names() {
        let spec = DataFusionSqlSpec {
            sql: "SELECT 1".into(),
            table_name: "bad-name".into(),
            output_path: "/out.parquet".into(),
        };
        assert!(validate(&spec).is_err());
    }
}
