//! DataFrame transpose and unpivot transform nodes.

use arrow_array::{Array, ArrayRef, Float64Array, RecordBatch, StringArray};
use arrow_cast::cast;
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

pub const TABLE_TRANSPOSE_KIND: &str = "table_transpose";
pub const MELT_UNPIVOT_KIND: &str = "melt_unpivot";

fn node_error(kind: &'static str, message: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: kind.into(),
        msg: message.into(),
    }
}

pub(crate) fn utf8_values(array: &ArrayRef) -> Result<Vec<Option<String>>, DagError> {
    let array = cast(array, &DataType::Utf8)
        .map_err(|error| DagError::Schedule(format!("cannot cast identifier to Utf8: {error}")))?;
    let array = array
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| DagError::Schedule("identifier cast did not produce Utf8".into()))?;
    Ok((0..array.len())
        .map(|index| {
            if array.is_null(index) {
                None
            } else {
                Some(array.value(index).to_string())
            }
        })
        .collect())
}

pub(crate) fn float_values(array: &ArrayRef) -> Result<Vec<Option<f64>>, DagError> {
    let array = cast(array, &DataType::Float64).map_err(|error| {
        DagError::Schedule(format!("cannot cast matrix value to Float64: {error}"))
    })?;
    let array = array
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| DagError::Schedule("value cast did not produce Float64".into()))?;
    Ok((0..array.len())
        .map(|index| {
            if array.is_null(index) {
                None
            } else {
                Some(array.value(index))
            }
        })
        .collect())
}

fn safe_column_name(value: &str, fallback: usize) -> String {
    let mut name = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            name.push(character);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let name = name.trim_matches('_').to_string();
    if name.is_empty() {
        format!("column_{fallback}")
    } else {
        name
    }
}

fn unique_names(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let mut candidate = safe_column_name(&name, index);
            let mut suffix = 1;
            while !seen.insert(candidate.clone()) {
                candidate = format!("{candidate}_{suffix}");
                suffix += 1;
            }
            candidate
        })
        .collect()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TableTransposeSpec {
    pub id_column: String,
    #[serde(default)]
    pub keep_columns: Vec<String>,
}

#[derive(Clone)]
pub struct TableTransposeNode {
    ports: NodePorts,
    id_column: String,
    keep_columns: Vec<String>,
}

impl TableTransposeNode {
    pub fn new(id_column: String, keep_columns: Vec<String>) -> Self {
        Self {
            ports: port_layout(),
            id_column,
            keep_columns,
        }
    }
}

fn port_layout() -> NodePorts {
    NodePorts::new().add_input_port(None).add_output_port(None)
}

#[async_trait]
impl DagNode for TableTransposeNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        TABLE_TRANSPOSE_KIND
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
            node_error(TABLE_TRANSPOSE_KIND, "one upstream DataFrame is required")
        })?;
        let batches = input.dataframe()?.clone().collect().await?;
        let schema = input.dataframe()?.schema().inner().clone();
        let fields = schema.fields();
        let field_names = fields
            .iter()
            .map(|field| field.name().as_str())
            .collect::<Vec<_>>();
        let id_index = field_names
            .iter()
            .position(|name| *name == self.id_column)
            .ok_or_else(|| {
                node_error(
                    TABLE_TRANSPOSE_KIND,
                    format!("id column `{}` is absent", self.id_column),
                )
            })?;
        let keep_indexes = self
            .keep_columns
            .iter()
            .map(|name| {
                field_names
                    .iter()
                    .position(|field| field == name)
                    .ok_or_else(|| {
                        node_error(
                            TABLE_TRANSPOSE_KIND,
                            format!("keep column `{name}` is absent"),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let value_indexes = (0..fields.len())
            .filter(|index| *index != id_index && !keep_indexes.contains(index))
            .collect::<Vec<_>>();
        if value_indexes.is_empty() {
            return Err(node_error(
                TABLE_TRANSPOSE_KIND,
                "input has no value columns to transpose",
            ));
        }

        let mut row_ids = Vec::new();
        let mut keep_rows = vec![Vec::new(); keep_indexes.len()];
        let mut matrix = vec![Vec::new(); value_indexes.len()];
        for batch in &batches {
            let ids = utf8_values(batch.column(id_index))?;
            let row_count = ids.len();
            row_ids.extend(ids);
            for (output, index) in keep_rows.iter_mut().zip(&keep_indexes) {
                output.extend(utf8_values(batch.column(*index))?);
            }
            for (output, index) in matrix.iter_mut().zip(&value_indexes) {
                output.extend(float_values(batch.column(*index))?);
            }
            if row_count != batch.num_rows() {
                return Err(node_error(
                    TABLE_TRANSPOSE_KIND,
                    "internal row-count mismatch",
                ));
            }
        }
        if row_ids.is_empty() {
            return Err(node_error(TABLE_TRANSPOSE_KIND, "input has no rows"));
        }

        let output_column_names = unique_names(
            row_ids
                .iter()
                .map(|value| value.clone().unwrap_or_else(|| "null".into()))
                .collect(),
        );
        let mut output_fields = vec![Field::new("column", DataType::Utf8, true)];
        for name in &self.keep_columns {
            output_fields.push(Field::new(name, DataType::Utf8, true));
        }
        for name in &output_column_names {
            output_fields.push(Field::new(name, DataType::Float64, true));
        }
        let output_schema = Arc::new(Schema::new(output_fields));
        let mut columns: Vec<ArrayRef> =
            Vec::with_capacity(1 + self.keep_columns.len() + row_ids.len());
        columns.push(Arc::new(StringArray::from(
            value_indexes
                .iter()
                .map(|index| field_names[*index].to_string())
                .collect::<Vec<_>>(),
        )));
        for rows in &keep_rows {
            columns.push(Arc::new(StringArray::from(rows.clone())));
        }
        for row in 0..row_ids.len() {
            let values = matrix
                .iter()
                .map(|column| column.get(row).copied().flatten())
                .collect::<Vec<_>>();
            columns.push(Arc::new(Float64Array::from(values)));
        }
        let batch = RecordBatch::try_new(output_schema, columns).map_err(|error| {
            node_error(
                TABLE_TRANSPOSE_KIND,
                format!("cannot transpose table: {error}"),
            )
        })?;
        let output = ctx.session().read_batch(batch)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, output);
        Ok(outputs)
    }
}

pub struct TableTransposeNodeFactory;

impl NodeFactory for TableTransposeNodeFactory {
    fn kind(&self) -> &'static str {
        TABLE_TRANSPOSE_KIND
    }

    fn desc(&self) -> &'static str {
        "Transposes a DataFrame using one identifier column as the new header."
    }

    fn doc(&self) -> &'static str {
        "The id column becomes output columns. Every non-id, non-keep column \
        becomes one output row named in the `column` field. Values are cast to \
        Float64. keep_columns are repeated by row and cast to Utf8 metadata."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TableTransposeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: TableTransposeSpec = serde_json::from_value(spec)?;
        if spec.id_column.trim().is_empty() {
            return Err("id_column cannot be empty".into());
        }
        Ok(Box::new(TableTransposeNode::new(
            spec.id_column,
            spec.keep_columns,
        )))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MeltUnpivotSpec {
    pub id_cols: Vec<String>,
    #[serde(default = "default_var_name")]
    pub var_name: String,
    #[serde(default = "default_value_name")]
    pub value_name: String,
    #[serde(default)]
    pub column_filter_regex: Option<String>,
}

fn default_var_name() -> String {
    "variable".into()
}

fn default_value_name() -> String {
    "value".into()
}

#[derive(Clone)]
pub struct MeltUnpivotNode {
    ports: NodePorts,
    spec: MeltUnpivotSpec,
}

impl MeltUnpivotNode {
    pub fn new(spec: MeltUnpivotSpec) -> Self {
        Self {
            ports: port_layout(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for MeltUnpivotNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        MELT_UNPIVOT_KIND
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
            .ok_or_else(|| node_error(MELT_UNPIVOT_KIND, "one upstream DataFrame is required"))?;
        let dataframe = input.dataframe()?;
        let batches = dataframe.clone().collect().await?;
        let fields = dataframe.schema().fields();
        let filter = self
            .spec
            .column_filter_regex
            .as_deref()
            .map(regex::Regex::new)
            .transpose()
            .map_err(|error| node_error(MELT_UNPIVOT_KIND, format!("invalid regex: {error}")))?;
        let id_set = self.spec.id_cols.iter().cloned().collect::<HashSet<_>>();
        let value_indexes = (0..fields.len())
            .filter(|index| {
                let name = fields[*index].name().as_str();
                !id_set.contains(name) && filter.as_ref().is_none_or(|regex| regex.is_match(name))
            })
            .collect::<Vec<_>>();
        if value_indexes.is_empty() {
            return Err(node_error(MELT_UNPIVOT_KIND, "no value columns selected"));
        }
        let all_numeric = value_indexes
            .iter()
            .all(|index| fields[*index].data_type().is_numeric());
        let value_type = if all_numeric {
            DataType::Float64
        } else {
            DataType::Utf8
        };

        let mut id_values = vec![Vec::<Option<String>>::new(); self.spec.id_cols.len()];
        let mut variable_values = Vec::new();
        let mut float_value_values = Vec::new();
        let mut string_value_values = Vec::new();
        for batch in &batches {
            let mut ids = Vec::with_capacity(self.spec.id_cols.len());
            for name in &self.spec.id_cols {
                let (index, _) = fields.find(name).ok_or_else(|| {
                    node_error(MELT_UNPIVOT_KIND, format!("id column `{name}` is absent"))
                })?;
                ids.push(utf8_values(batch.column(index))?);
            }
            for row in 0..batch.num_rows() {
                for index in &value_indexes {
                    for (output, values) in id_values.iter_mut().zip(&ids) {
                        output.push(values.get(row).cloned().flatten());
                    }
                    variable_values.push(Some(fields[*index].name().to_string()));
                    if value_type == DataType::Float64 {
                        float_value_values.push(float_values(batch.column(*index))?[row]);
                    } else {
                        string_value_values.push(utf8_values(batch.column(*index))?[row].clone());
                    }
                }
            }
        }

        let mut output_fields = self
            .spec
            .id_cols
            .iter()
            .map(|name| Field::new(name, DataType::Utf8, true))
            .collect::<Vec<_>>();
        output_fields.push(Field::new(&self.spec.var_name, DataType::Utf8, true));
        output_fields.push(Field::new(&self.spec.value_name, value_type.clone(), true));
        let mut columns: Vec<ArrayRef> = id_values
            .into_iter()
            .map(|values| Arc::new(StringArray::from(values)) as ArrayRef)
            .collect();
        columns.push(Arc::new(StringArray::from(variable_values)));
        columns.push(if value_type == DataType::Float64 {
            Arc::new(Float64Array::from(float_value_values))
        } else {
            Arc::new(StringArray::from(string_value_values))
        });
        let batch = RecordBatch::try_new(Arc::new(Schema::new(output_fields)), columns).map_err(
            |error| node_error(MELT_UNPIVOT_KIND, format!("cannot unpivot table: {error}")),
        )?;
        let output = ctx.session().read_batch(batch)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, output);
        Ok(outputs)
    }
}

pub struct MeltUnpivotNodeFactory;

impl NodeFactory for MeltUnpivotNodeFactory {
    fn kind(&self) -> &'static str {
        MELT_UNPIVOT_KIND
    }

    fn desc(&self) -> &'static str {
        "Unpivots selected wide-table columns into id/variable/value rows."
    }

    fn doc(&self) -> &'static str {
        "All columns not listed in id_cols are melted. column_filter_regex \
        optionally restricts those value columns. Numeric selections produce a \
        Float64 value column; mixed selections produce Utf8."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MeltUnpivotSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let parsed: MeltUnpivotSpec = serde_json::from_value(spec)?;
        if parsed.id_cols.is_empty() {
            return Err("id_cols cannot be empty".into());
        }
        if parsed.var_name.trim().is_empty() || parsed.value_name.trim().is_empty() {
            return Err("var_name and value_name cannot be empty".into());
        }
        if parsed.id_cols.contains(&parsed.var_name)
            || parsed.id_cols.contains(&parsed.value_name)
            || parsed.var_name == parsed.value_name
        {
            return Err("id_cols, var_name, and value_name must be distinct".into());
        }
        if let Some(pattern) = parsed.column_filter_regex.as_deref() {
            regex::Regex::new(pattern)
                .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        }
        Ok(Box::new(MeltUnpivotNode::new(parsed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Int64Array;
    use datafusion::prelude::SessionContext;

    fn ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    fn wide_frame() -> datafusion::prelude::DataFrame {
        let session = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![
            Field::new("gene", DataType::Utf8, false),
            Field::new("pathway", DataType::Utf8, false),
            Field::new("cell_1", DataType::Int64, false),
            Field::new("cell_2", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["GENE1", "GENE2"])),
                Arc::new(StringArray::from(vec!["immune", "metabolic"])),
                Arc::new(Int64Array::from(vec![1, 3])),
                Arc::new(Int64Array::from(vec![2, 4])),
            ],
        )
        .unwrap();
        session.read_batch(batch).unwrap()
    }

    #[tokio::test]
    async fn transposes_matrix_and_repeats_metadata() {
        let mut node = TableTransposeNode::new("gene".into(), vec!["pathway".into()]);
        let outputs = node
            .execute(
                &ctx(),
                &[NodeInput::new_dataframe(0, wide_frame())],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let schema = outputs.dataframe(0).unwrap().schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, ["column", "pathway", "GENE1", "GENE2"]);
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn unpivots_with_regex_filter() {
        let mut node = MeltUnpivotNode::new(MeltUnpivotSpec {
            id_cols: vec!["gene".into()],
            var_name: "cell".into(),
            value_name: "count".into(),
            column_filter_regex: Some("^cell_".into()),
        });
        let outputs = node
            .execute(
                &ctx(),
                &[NodeInput::new_dataframe(0, wide_frame())],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            4
        );
    }
}
