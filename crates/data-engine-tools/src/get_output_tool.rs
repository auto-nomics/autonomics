use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use arrow::datatypes::DataType;
use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Date64Type, Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type,
    UInt8Type, UInt16Type, UInt32Type, UInt64Type,
};
use arrow_array::{Array, OffsetSizeTrait, RecordBatch};
use datafusion::prelude::DataFrame;
use serde_json::json;

use crate::ExecError;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

#[tool(
    name = "get_output",
    description = "Get the output DataFrames of a node after a DAG run. \
                  The node must have been executed (status Success). \
                  Returns the actual data rows, with optional offset and limit \
                  to control pagination. If the DataFrame is large, use limit \
                  to avoid returning excessive data. \
                  \
                  Each output entry reports `total_rows` (full row count, from \
                  COUNT(*)), `returned_rows` (rows actually materialized in \
                  this page), and `data`. The `data` body is a compact 2D \
                  matrix: `{ columns: [...names], rows: [[...values], ...] }` — \
                  each inner array is one record's values positionally aligned \
                  with `columns`. If materializing the page fails (e.g. a \
                  column cast/decode error), the entry carries a \
                  `collect_error` string instead of silently reporting 0 rows. \
                  The row count is hard-clamped to at most `max_limit` rows \
                  regardless of the requested `limit`; use `offset` to page."
)]
pub struct GetOutputInput {
    /// The node id to query output for.
    pub id: String,
    /// Number of rows to skip from the beginning. Defaults to 0.
    pub offset: Option<usize>,
    /// Maximum number of rows to return. Defaults to 100 (`DEFAULT_LIMIT`). \
    /// Any request above `MAX_LIMIT` (or `0`, formerly "unlimited") is \
    /// hard-clamped to `MAX_LIMIT` — the full table is never materialized. \
    /// Page with `offset` instead.
    pub limit: Option<usize>,
}

/// Default page size when the caller omits `limit`.
const DEFAULT_LIMIT: usize = 25;

/// Hard ceiling on rows materialized per `get_output` call, regardless of what
/// the agent requests. Prevents a pathologically large (or `0` = "unlimited")
/// request from dragging the whole table into the tool result.
const MAX_LIMIT: usize = 50;

/// Hard ceiling on evidence records rendered inline (mirrors `MAX_LIMIT`).
const EVIDENCE_MAX_RECORDS: usize = 50;

pub struct GetOutputTool {
    client: Arc<DataEngineClient>,
}

impl GetOutputTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }

    /// Render an `evidence` artifact inline: the agent sees the citation list
    /// directly instead of a bare path. Read/parse failures degrade to a
    /// metadata entry carrying the error (fail-visible, never a swallowed
    /// path).
    async fn render_evidence(&self, name: &u8, file: &data_engine::FileRef) -> serde_json::Value {
        let mut entry = serde_json::json!({
            "name": name,
            "type": "evidence",
            "path": file.path,
            "format": file.format,
            "size": file.fingerprint.as_ref().map(|fp| fp.size),
            "fingerprint": file.fingerprint,
        });
        let bytes = match self.client.read_file(file.path.clone()).await {
            Ok(bytes) => bytes,
            Err(error) => {
                entry["read_error"] = serde_json::Value::String(error.to_string());
                return entry;
            }
        };
        match bib_types::evidence::EvidenceSet::parse(&bytes) {
            Ok(set) => {
                let total = set.records.len();
                let returned = total.min(EVIDENCE_MAX_RECORDS);
                let records: Vec<serde_json::Value> = set.records[..returned]
                    .iter()
                    .map(|record| {
                        serde_json::json!({
                            "cite": record.citation.short_cite(),
                            "title": record.citation.title,
                            "year": record.citation.year,
                            "journal": record.citation.journal,
                            "doi": record.citation.doi(),
                            "origin": record.origin,
                            "note": record.note,
                        })
                    })
                    .collect();
                entry["total"] = serde_json::Value::Number(total.into());
                entry["returned"] = serde_json::Value::Number(returned.into());
                entry["records"] = serde_json::Value::Array(records);
                entry
            }
            Err(error) => {
                entry["parse_error"] = serde_json::Value::String(error);
                entry
            }
        }
    }
}

/// Render a single cell (`array[row]`) as a JSON value.
///
/// Recursive over Arrow's nested types so VCF/biofusion schemas serialize
/// correctly:
/// - primitives (ints/floats/bool/strings, incl. Utf8View)
/// - `Struct` → JSON object keyed by field name (e.g. VCF `info`, genotype
///   columns)
/// - `List` / `LargeList` / `FixedSizeList` → JSON array (e.g. VCF `alt`,
///   `filter`, INFO `AF`)
/// - `Dictionary(K, V)` → decoded to the value type (e.g. VCF `chrom` which is
///   `Dictionary(Int32, Utf8)`)
/// - `Map` → JSON object
///
/// Unknown types fall back to a debug string carrying the type name so the
/// caller can see what wasn't rendered, rather than losing the column.
fn cell_to_json(array: &dyn Array, row: usize) -> serde_json::Value {
    use serde_json::{Map, Value};
    if array.is_null(row) {
        return Value::Null;
    }
    match array.data_type() {
        DataType::Boolean => Value::Bool(array.as_boolean().value(row)),
        DataType::Int8 => json!(array.as_primitive::<Int8Type>().value(row)),
        DataType::Int16 => json!(array.as_primitive::<Int16Type>().value(row)),
        DataType::Int32 => json!(array.as_primitive::<Int32Type>().value(row)),
        DataType::Int64 => json!(array.as_primitive::<Int64Type>().value(row)),
        DataType::UInt8 => json!(array.as_primitive::<UInt8Type>().value(row)),
        DataType::UInt16 => json!(array.as_primitive::<UInt16Type>().value(row)),
        DataType::UInt32 => json!(array.as_primitive::<UInt32Type>().value(row)),
        DataType::UInt64 => json!(array.as_primitive::<UInt64Type>().value(row)),
        DataType::Float32 => json!(array.as_primitive::<Float32Type>().value(row)),
        DataType::Float64 => json!(array.as_primitive::<Float64Type>().value(row)),
        DataType::Date32 => {
            // Days since UNIX epoch (1970-01-01).
            let days = array.as_primitive::<Date32Type>().value(row);
            json!(days)
        }
        DataType::Date64 => {
            let ms = array.as_primitive::<Date64Type>().value(row);
            json!(ms)
        }
        DataType::Timestamp(unit, _) => {
            // Render the raw integer; callers can interpret per `unit`. Avoids
            // pulling a chrono dependency just for formatting.
            let v = match unit {
                arrow::datatypes::TimeUnit::Second => array
                    .as_primitive::<arrow_array::types::TimestampSecondType>()
                    .value(row),
                arrow::datatypes::TimeUnit::Millisecond => array
                    .as_primitive::<arrow_array::types::TimestampMillisecondType>()
                    .value(row),
                arrow::datatypes::TimeUnit::Microsecond => array
                    .as_primitive::<arrow_array::types::TimestampMicrosecondType>()
                    .value(row),
                arrow::datatypes::TimeUnit::Nanosecond => array
                    .as_primitive::<arrow_array::types::TimestampNanosecondType>()
                    .value(row),
            };
            json!(v)
        }
        DataType::Utf8 => json!(array.as_string::<i32>().value(row)),
        DataType::LargeUtf8 => json!(array.as_string::<i64>().value(row)),
        DataType::Utf8View => json!(array.as_string_view().value(row)),
        DataType::Struct(_) => {
            let s = array.as_struct();
            let mut obj = Map::new();
            for (i, field) in s.fields().iter().enumerate() {
                obj.insert(field.name().clone(), cell_to_json(s.column(i), row));
            }
            Value::Object(obj)
        }
        DataType::List(_) => list_cells(array.as_list::<i32>(), row),
        DataType::LargeList(_) => list_cells(array.as_list::<i64>(), row),
        DataType::Dictionary(_, _) => {
            // Decode: look up the values array at the row's key index. We use
            // the generic dictionary-key accessor so any integer key type works
            // (VCF uses Int32; some sources use Int8/Int16).
            dict_cell(array, row)
        }
        DataType::Map(_, _) => {
            let m = array.as_map();
            let entry = m.value(row); // a StructArray of (key, value)
            let keys = entry.column(0);
            let vals = entry.column(1);
            let mut obj = Map::new();
            for i in 0..entry.len() {
                let k = cell_to_json(keys, i);
                let key_str = match k {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                obj.insert(key_str, cell_to_json(vals, i));
            }
            Value::Object(obj)
        }
        // Anything else: surface the type so the caller knows it was skipped.
        other => Value::String(format!("<unsupported type: {other}>")),
    }
}

/// Shared helper: serialize one row of a (Large)ListArray as a JSON array.
fn list_cells<O: OffsetSizeTrait>(
    list: &arrow_array::GenericListArray<O>,
    row: usize,
) -> serde_json::Value {
    let values = list.value(row); // sub-array for this row
    let mut out = Vec::with_capacity(values.len());
    for i in 0..values.len() {
        out.push(cell_to_json(values.as_ref(), i));
    }
    serde_json::Value::Array(out)
}

/// Decode a `DictionaryArray` cell by indexing into its values array at the
/// row's stored key.
fn dict_cell(array: &dyn Array, row: usize) -> serde_json::Value {
    use arrow_array::types::{
        Int8Type as K8, Int16Type as K16, Int32Type as K32, Int64Type as K64, UInt8Type as U8,
        UInt16Type as U16, UInt32Type as U32, UInt64Type as U64,
    };
    macro_rules! decode {
        ($t:ty) => {{
            let d = array.as_dictionary::<$t>();
            let key = d.key(row);
            match key {
                Some(k) => cell_to_json(d.values().as_ref(), k as usize),
                None => serde_json::Value::Null,
            }
        }};
    }
    match array.data_type() {
        DataType::Dictionary(key, _) => match key.as_ref() {
            DataType::Int8 => decode!(K8),
            DataType::Int16 => decode!(K16),
            DataType::Int32 => decode!(K32),
            DataType::Int64 => decode!(K64),
            DataType::UInt8 => decode!(U8),
            DataType::UInt16 => decode!(U16),
            DataType::UInt32 => decode!(U32),
            DataType::UInt64 => decode!(U64),
            _ => serde_json::Value::String("<unsupported dictionary key>".into()),
        },
        _ => serde_json::Value::Null,
    }
}

/// Convert collected `RecordBatch`es into a compact 2D matrix.
///
/// Rather than repeating every column name per row (an array of objects,
/// which balloons token cost on wide frames), the body is rendered as:
///
/// ```json
/// { "columns": ["c0", "c1"], "rows": [[v00, v01], [v10, v11]] }
/// ```
///
/// `columns` states the field names once, in order; each entry of `rows` is
/// one record's values, positionally aligned with `columns`. All batches of a
/// DataFrame share one schema, so the column order is taken from the first
/// batch and every batch is read positionally.
fn batches_to_matrix(batches: &[RecordBatch]) -> serde_json::Value {
    if batches.is_empty() {
        return serde_json::json!({ "columns": [], "rows": [] });
    }

    let schema = batches[0].schema();
    let fields = schema.fields();
    let columns: Vec<serde_json::Value> =
        fields.iter().map(|f| serde_json::json!(f.name())).collect();

    let mut rows = Vec::new();
    for batch in batches {
        let ncols = batch.num_columns();
        for row_idx in 0..batch.num_rows() {
            let row: Vec<serde_json::Value> = (0..ncols)
                .map(|col_idx| cell_to_json(batch.column(col_idx).as_ref(), row_idx))
                .collect();
            rows.push(serde_json::Value::Array(row));
        }
    }

    serde_json::json!({ "columns": columns, "rows": rows })
}

#[async_trait]
impl ToolFunction for GetOutputTool {
    type Input = GetOutputInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let Some(dfs) = self
            .client
            .get_output(input.id.clone())
            .await
            .map_err(ExecError::from)?
        else {
            // No output cached for this node. Determine *why* so the agent
            // gets an actionable hint rather than a bare "not found".
            let status = self
                .client
                .node_status(input.id.clone())
                .await
                .map_err(ExecError::from)?;

            let hint = match status {
                None => format!(
                    "Node '{}' has no output. The DAG has not been run yet — \
                     call the `run_dag` tool to execute it first, then retry \
                     `get_output`.",
                    input.id
                ),
                Some(data_engine::dag::runtime::RuntimeStatus::Failed) => format!(
                    "Node '{}' has no output because it failed during the last \
                     DAG run. Check the run report for error details, fix the \
                     issue, then re-run the DAG with `run_dag`.",
                    input.id
                ),
                Some(data_engine::dag::runtime::RuntimeStatus::Skipped) => format!(
                    "Node '{}' was skipped because an upstream node failed. \
                     Resolve the upstream failure, then re-run the DAG with \
                     `run_dag`.",
                    input.id
                ),
                Some(data_engine::dag::runtime::RuntimeStatus::Cancelled) => format!(
                    "Node '{}' has no output because the previous DAG run was cancelled \
                     by the memory guard. Lower node fan-out or re-run after memory is available.",
                    input.id
                ),
                Some(other) => format!(
                    "Node '{}' has no output (current status: {:?}). Wait for \
                     the DAG run to complete or re-run it with `run_dag`, then \
                     retry `get_output`.",
                    input.id, other
                ),
            };

            return Ok(ToolResult::error(hint));
        };

        let offset = input.offset.unwrap_or(0);

        // Resolve the requested page size, then hard-clamp it. Previously a
        // caller could pass `limit: 0` for "unlimited" or any arbitrarily large
        // number, materializing the entire table into one tool result. Now:
        //   - missing / `0` → DEFAULT_LIMIT (the `0` = "unlimited" mode is gone)
        //   - anything above MAX_LIMIT → MAX_LIMIT
        // so the row count returned to the agent is always bounded.
        let requested_limit = input.limit.unwrap_or(DEFAULT_LIMIT);
        let limit = if requested_limit == 0 {
            DEFAULT_LIMIT
        } else {
            requested_limit.min(MAX_LIMIT)
        };
        let limit_clamped = requested_limit == 0 || requested_limit > MAX_LIMIT;

        let mut outputs_info = Vec::with_capacity(dfs.len());
        for (name, value) in dfs.iter() {
            let Some(df) = value.as_dataframe().ok() else {
                outputs_info.push(match value {
                    data_engine::NodeValue::File(file)
                        if file.format.as_deref() == Some(bib_types::evidence::FORMAT) =>
                    {
                        self.render_evidence(name, file).await
                    }
                    data_engine::NodeValue::File(file) => serde_json::json!({
                        "name": name,
                        "type": "file",
                        "path": file.path,
                        "format": file.format,
                        "size": file.fingerprint.as_ref().map(|fp| fp.size),
                        "fingerprint": file.fingerprint,
                    }),
                    data_engine::NodeValue::FileSet(files) => serde_json::json!({
                        "name": name,
                        "type": "file_set",
                        "files": files.iter().map(|file| serde_json::json!({
                            "path": file.path,
                            "format": file.format,
                            "size": file.fingerprint.as_ref().map(|fp| fp.size),
                            "fingerprint": file.fingerprint,
                        })).collect::<Vec<_>>(),
                    }),
                    // Handled by the `as_dataframe` fast path above; this arm
                    // only exists to keep the match exhaustive without a
                    // panic path.
                    data_engine::NodeValue::DataFrame(_) => serde_json::json!({
                        "name": name,
                        "type": "dataframe",
                    }),
                });
                continue;
            };

            let fields: Vec<serde_json::Value> = df
                .schema()
                .fields()
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name(),
                        "type": f.data_type().to_string(),
                    })
                })
                .collect();

            // total_rows: full COUNT(*) over the DataFrame. This is eager but
            // get_output is an explicit, on-demand call (not in the DAG hot
            // path), so it's acceptable. Surface failures as `null` + a
            // `count_error` rather than silently `0` — a count error usually
            // signals the same decode problem that will bite `collect` below.
            let (total_rows, count_error): (Option<usize>, Option<String>) =
                match df.clone().count().await {
                    Ok(n) => (Some(n), None),
                    Err(e) => (None, Some(format!("{e}"))),
                };

            // Materialize the requested page. Crucially, do NOT swallow the
            // error: previously `collect().await.unwrap_or_default()` silently
            // turned a decode/cast failure into an empty batch vector, yielding
            // the misleading `returned_rows: 0, total_rows: N` (obstacle #2).
            // Now we capture the error and report it in-band per output.
            let fetch = Some(limit);
            let queried =
                df.clone()
                    .limit(offset, fetch)
                    .map_err(|e| ToolError::ExecutionFailed {
                        source: Box::new(e),
                    })?;

            let limit_field = serde_json::json!({
                "requested": requested_limit,
                "effective": limit,
                "clamped": limit_clamped,
                "max": MAX_LIMIT,
            });

            let entry = match queried.collect().await {
                Ok(batches) => {
                    let returned_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
                    let data = batches_to_matrix(&batches);
                    serde_json::json!({
                        "name": name,
                        "columns": fields.len(),
                        "total_rows": total_rows,
                        "count_error": count_error,
                        "returned_rows": returned_rows,
                        "offset": offset,
                        "limit": limit_field,
                        "fields": fields,
                        "data": data,
                    })
                }
                Err(e) => {
                    // collect failed (e.g. a column cast/decode error). Report
                    // the real message instead of pretending 0 rows were
                    // returned. `total_rows` may still be populated from the
                    // COUNT(*) above, which is exactly the divergence that used
                    // to masquerade as "returned_rows: 0".
                    serde_json::json!({
                        "name": name,
                        "columns": fields.len(),
                        "total_rows": total_rows,
                        "count_error": count_error,
                        "returned_rows": null,
                        "offset": offset,
                        "limit": limit_field,
                        "fields": fields,
                        "data": null,
                        "collect_error": format!("{e}"),
                    })
                }
            };

            outputs_info.push(entry);
        }

        let content = serde_json::json!({
            "node": input.id,
            "output_count": outputs_info.len(),
            "outputs": outputs_info,
        });

        Ok(ToolResult::success_json(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Fields, Schema};
    use arrow_array::types::{Float32Type, Int32Type};
    use arrow_array::{
        ArrayRef, BooleanArray, DictionaryArray, Float32Array, Float64Array, Int32Array,
        RecordBatch, StringArray, StructArray,
        builder::{ListBuilder, PrimitiveBuilder},
    };
    use std::sync::Arc;

    /// Build a one-row batch that exercises the nested types the VCF schema
    /// produces: a Dictionary(Int32, Utf8) column, a List(Utf8) column, and a
    /// Struct column containing a List(Float32) subfield. This is the shape
    /// that the old `pretty_format_batches`-based renderer handled poorly.
    fn nested_batch() -> RecordBatch {
        // chrom: Dictionary(Int32, Utf8) — like oxbow's chrom column.
        let dict_values = StringArray::from(vec!["chr1", "chr2"]);
        let dict_keys = Int32Array::from(vec![0]); // row 0 → "chr1"
        let chrom = DictionaryArray::<Int32Type>::new(dict_keys, Arc::new(dict_values));

        // alt: List(Utf8) — like oxbow's alt column. Built via ListBuilder so
        // we don't need the arrow-buffer OffsetBuffer type directly.
        let mut alt_b = ListBuilder::new(arrow_array::builder::StringBuilder::new());
        alt_b.values().append_value("A");
        alt_b.values().append_value("T");
        alt_b.append(true); // one list row: ["A", "T"]
        let alt = alt_b.finish();

        // info: Struct { AF: List(Float32) } — nested struct + list.
        let mut af_b = ListBuilder::new(PrimitiveBuilder::<Float32Type>::new());
        af_b.values().append_value(0.25);
        af_b.values().append_value(0.75);
        af_b.append(true);
        let af_list = af_b.finish();
        let af_field = Field::new(
            "AF",
            DataType::List(Arc::new(Field::new("item", DataType::Float32, true))),
            true,
        );
        let info = StructArray::new(
            vec![af_field].into(),
            vec![Arc::new(af_list) as ArrayRef],
            None,
        );

        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "chrom",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                false,
            ),
            Field::new(
                "alt",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                false,
            ),
            Field::new(
                "info",
                DataType::Struct(Fields::from(vec![Field::new(
                    "AF",
                    DataType::List(Arc::new(Field::new("item", DataType::Float32, true))),
                    true,
                )])),
                false,
            ),
        ]));

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(chrom) as ArrayRef,
                Arc::new(alt) as ArrayRef,
                Arc::new(info) as ArrayRef,
            ],
        )
        .expect("nested batch should construct")
    }

    #[test]
    fn cell_to_json_renders_dictionary_list_and_struct() {
        let batch = nested_batch();
        // One row, three columns.
        let rendered = batches_to_matrix(std::slice::from_ref(&batch));
        let obj = rendered
            .as_object()
            .expect("batches_to_matrix yields an object");

        // Column names stated once, in schema order.
        assert_eq!(
            obj["columns"],
            serde_json::json!(["chrom", "alt", "info"]),
            "columns must be the schema field names in order"
        );

        // One row, positionally aligned with `columns`.
        let rows = obj["rows"].as_array().expect("rows is an array");
        assert_eq!(rows.len(), 1, "expected exactly one row");
        let row = rows[0]
            .as_array()
            .expect("each row is an array of positional values");

        // Dictionary(Int32, Utf8) decodes to the underlying string.
        assert_eq!(row[0], serde_json::json!("chr1"));

        // List(Utf8) → JSON array of strings.
        assert_eq!(row[1], serde_json::json!(["A", "T"]));

        // Struct { AF: List(Float32) } → nested object with array.
        assert_eq!(row[2], serde_json::json!({"AF": [0.25, 0.75]}));
    }

    #[test]
    fn cell_to_json_renders_null_as_json_null() {
        // All-null column → every cell is JSON null, not an error.
        let arr = Int32Array::from(vec![None]); // one null int32
        let v = cell_to_json(&arr, 0);
        assert!(v.is_null(), "null cell must be JSON null, got {v}");
    }

    #[test]
    fn cell_to_json_renders_primitives() {
        assert_eq!(cell_to_json(&Int32Array::from(vec![7]), 0), json!(7));
        assert_eq!(cell_to_json(&Float64Array::from(vec![1.5]), 0), json!(1.5));
        assert_eq!(
            cell_to_json(&BooleanArray::from(vec![true]), 0),
            json!(true)
        );
        assert_eq!(cell_to_json(&StringArray::from(vec!["hi"]), 0), json!("hi"));
        // Unused Float32 import guard — keeps the type listed even if only used
        // in nested_batch via the builder.
        let _ = Float32Array::from(vec![0.0f32]);
    }
}
