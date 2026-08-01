use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::DataType;
use async_trait::async_trait;
use datalake::Datalake;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;

#[tool(
    name = "query_iceberg",
    description = "Query the Iceberg data lake. \
                  Supports standard SQL SELECT queries against tables in the 'iceberg' catalog \
                  (e.g. 'SELECT * FROM iceberg.namespace.table LIMIT 10'). \
                  Also supports the special commands 'LIST TABLES' and 'SHOW TABLES' to list \
                  all available tables and namespaces in the data lake. \
                  Returns results as JSON with windowed pagination via `offset` + `max_rows`. \
                  The response includes `total_available`, `offset`, `row_count`, and `has_more` \
                  so you can page through large tables without re-running the query from scratch. \
\
        **IMPORTANT**: you need to add `iceberg` before the table ident (e.g. `SELECT * FROM iceberg.ns.tb`).
        "
)]
pub struct QueryIcebergInput {
    #[desc = "SQL query (SELECT ...) or 'LIST TABLES' / 'SHOW TABLES' to list all tables"]
    pub query: String,
    #[desc = "Maximum number of rows to return in this page. Defaults to 50."]
    pub max_rows: Option<usize>,
    #[desc = "Number of rows to skip before returning results (0-based offset for pagination). Defaults to 0."]
    pub offset: Option<usize>,
}

pub struct QueryIcebergTool {
    datalake: Arc<Datalake>,
}

impl QueryIcebergTool {
    pub fn new(datalake: Arc<Datalake>) -> Self {
        Self { datalake }
    }
}

#[async_trait]
impl ToolFunction for QueryIcebergTool {
    type Input = QueryIcebergInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let trimmed = input.query.trim();
        let max_rows = input.max_rows.unwrap_or(50);
        let offset = input.offset.unwrap_or(0);

        let upper = trimmed.to_uppercase();
        if upper.starts_with("LIST TABLES") || upper.starts_with("SHOW TABLES") {
            return self.list_tables().await;
        }

        self.run_sql(trimmed, max_rows, offset).await
    }
}

impl QueryIcebergTool {
    async fn list_tables(&self) -> Result<ToolResult, ToolError> {
        let tables =
            self.datalake
                .list_all_tables()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    source: format!("{e}").into(),
                })?;

        let json: Vec<serde_json::Value> = tables
            .iter()
            .map(|(ns, name)| {
                serde_json::json!({
                    "namespace": ns.join("."),
                    "table": name,
                })
            })
            .collect();

        Ok(ToolResult::success_json(serde_json::json!({
            "tables": json,
            "count": json.len(),
        })))
    }

    async fn run_sql(
        &self,
        query: &str,
        max_rows: usize,
        offset: usize,
    ) -> Result<ToolResult, ToolError> {
        let ctx = self
            .datalake
            .get_ctx()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: format!("Get ctx failed during executing SQL query: {e}").into(),
            })?;

        let df = ctx
            .sql(query)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: match e {
                    datafusion::error::DataFusionError::External(error) => {
                        format!("external error: {error}").into()
                    }
                    _ => e.to_string().into(),
                },
            })?;

        let batches: Vec<RecordBatch> =
            df.collect().await.map_err(|e| ToolError::ExecutionFailed {
                source: format!("{e}").into(),
            })?;

        // Total rows available from the query (before windowing).
        let total_available: usize = batches.iter().map(|b| b.num_rows()).sum();
        let schema = batches.first().map(|b| b.schema().clone());

        // Walk the batches, skipping `offset` rows, then collecting up to
        // `max_rows` rows into the result window.
        let mut rows: Vec<serde_json::Value> = Vec::new();
        let mut skipped: usize = 0;
        let mut taken: usize = 0;

        'outer: for batch in &batches {
            let batch_rows = batch.num_rows();
            // Determine the slice [start, end) of this batch to read.
            let mut start = 0usize;
            // Skip rows still owed from `offset`.
            if skipped < offset {
                let skip_here = (offset - skipped).min(batch_rows);
                skipped += skip_here;
                start = skip_here;
                if start >= batch_rows {
                    continue;
                }
            }
            let remaining = max_rows - taken;
            let end = (start + remaining).min(batch_rows);
            for row_idx in start..end {
                let mut row = serde_json::Map::new();
                for col_idx in 0..batch.num_columns() {
                    let col = batch.column(col_idx);
                    let name = batch.schema().field(col_idx).name().clone();
                    let val = scalar_to_json(col, row_idx);
                    row.insert(name, val);
                }
                rows.push(serde_json::Value::Object(row));
                taken += 1;
                if taken >= max_rows {
                    break 'outer;
                }
            }
        }

        let columns: Vec<serde_json::Value> = schema
            .as_ref()
            .map(|s| {
                s.fields()
                    .into_iter()
                    .map(|f| {
                        serde_json::json!({
                            "name": f.name(),
                            "type": f.data_type().to_string(),
                            "nullable": f.is_nullable(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // A next page exists if the current window does not reach the end.
        let window_end = offset.saturating_add(rows.len());
        let has_more = window_end < total_available;

        Ok(ToolResult::success_json(serde_json::json!({
            "rows": rows,
            "columns": columns,
            "row_count": rows.len(),
            "offset": offset,
            "total_available": total_available,
            "has_more": has_more,
        })))
    }
}

/// Extract a scalar value from an Arrow array at the given row index.
pub(crate) fn scalar_to_json(col: &dyn arrow_array::Array, row: usize) -> serde_json::Value {
    if col.is_null(row) {
        return serde_json::Value::Null;
    }
    match col.data_type() {
        DataType::Boolean => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::BooleanArray>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Int8 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Int8Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Int16 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Int16Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Int32 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Int32Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Int64 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Int64Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::UInt8 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::UInt8Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::UInt16 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::UInt16Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::UInt32 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::UInt32Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::UInt64 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::UInt64Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Float16 | DataType::Float32 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Float32Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Float64 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Float64Array>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Utf8 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::StringArray>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Utf8View => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::StringViewArray>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::LargeUtf8 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::LargeStringArray>()
                .unwrap();
            serde_json::json!(arr.value(row))
        }
        DataType::Date32 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Date32Array>()
                .unwrap();
            serde_json::json!(arr.value(row).to_string())
        }
        DataType::Date64 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Date64Array>()
                .unwrap();
            serde_json::json!(arr.value(row).to_string())
        }
        _ => serde_json::json!(format!("<{}>", col.data_type())),
    }
}

#[cfg(test)]
mod tests {
    use super::QueryIcebergTool;
    use datalake::datalake;
    use std::sync::Arc;

    // #[tokio::test]
    // async fn test_sql_query() {
    //     let dk = datalake::Datalake::default();
    //     let tool = QueryIcebergTool::new(Arc::new(dk));
    //     let df = tool
    //         .run_sql("SELECT * FROM iceberg.gwas.test", 10, 0)
    //         .await
    //         .unwrap();
    //     dbg!(df);
    // }
}
