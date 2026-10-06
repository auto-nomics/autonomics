//! `table_from_df` node — converts an upstream DataFrame into a LaTeX tabular string.
//!
//! This node reads the schema and data rows from the input DataFrame and
//! produces a single-row DataFrame with a `latex` column containing the
//! rendered LaTeX `tabular` environment (with booktabs rules).

use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::common::HashMap;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use dag_core::{
    dag::{DagError, graph::PortOutputs},
    node::{DagNode, NodeInput, NodePorts},
    registry::NodeFactory,
};

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TableFromDfSpec {
    /// Table format: `booktabs` (default) or `plain`.
    #[serde(default = "default_format")]
    pub format: String,

    /// Optional caption for the table.
    pub caption: Option<String>,

    /// Optional `\label{...}` for cross-referencing.
    pub label: Option<String>,

    /// Maximum number of data rows to include (0 or omitted = all).
    pub max_rows: Option<usize>,
}

fn default_format() -> String {
    "booktabs".into()
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TableFromDfNode {
    meta: NodePorts,
    format: String,
    caption: Option<String>,
    label: Option<String>,
    max_rows: Option<usize>,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None) // DataFrame input
        .add_output_port(None) // String output (latex column)
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

pub struct TableFromDfFactory {}

impl NodeFactory for TableFromDfFactory {
    fn kind(&self) -> &'static str {
        "table_from_df"
    }

    fn desc(&self) -> &'static str {
        "Convert a DataFrame to a LaTeX tabular environment string."
    }

    fn doc(&self) -> &'static str {
        "Takes a DataFrame as input and produces a single-row DataFrame with a 'latex' column \
         containing the rendered LaTeX tabular environment. Uses booktabs by default \
         (\\toprule, \\midrule, \\bottomrule). The header row comes from the DataFrame column \
         names. Renders Utf8/LargeUtf8/Utf8View, Boolean, Int8-64, UInt8-64, Float32/64, \
         Date32/64, Time32/64 and Timestamp columns; nulls become empty cells and any other \
         column type is rejected with an error instead of rendering blank."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TableFromDfSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: dag_core::registry::NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TableFromDfSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TableFromDfNode {
            meta: port_layout(),
            format: s.format,
            caption: s.caption,
            label: s.label,
            max_rows: s.max_rows,
        }))
    }
}

// ---------------------------------------------------------------------------
// DagNode impl
// ---------------------------------------------------------------------------

#[async_trait]
impl DagNode for TableFromDfNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "table_from_df"
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
            return Err(DagError::NodeError {
                node_type: "table_from_df".into(),
                msg: "no input DataFrame".into(),
            });
        }

        let df = inputs[0].dataframe()?;

        // Collect rows from the DataFrame.
        let batches = df
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "table_from_df".into(),
                msg: format!("failed to collect batches: {e}"),
            })?;

        // Extract schema (column names).
        let schema = df.schema();
        let col_names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
        let n_cols = col_names.len();

        if n_cols == 0 {
            return Err(DagError::NodeError {
                node_type: "table_from_df".into(),
                msg: "input DataFrame has no columns".into(),
            });
        }

        // Build column alignment spec.
        let col_spec: String = "l".repeat(n_cols);

        // Collect data rows.
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut row_count = 0usize;

        for batch in &batches {
            let n = batch.num_rows();
            for i in 0..n {
                if let Some(max) = self.max_rows {
                    if row_count >= max {
                        break;
                    }
                }
                let mut row = Vec::with_capacity(n_cols);
                for j in 0..n_cols {
                    let col = batch.column(j);
                    let cell =
                        arrow_cell_to_string(col.as_ref(), i, &col_names[j]).map_err(|msg| {
                            DagError::NodeError {
                                node_type: "table_from_df".into(),
                                msg,
                            }
                        })?;
                    row.push(cell);
                }
                rows.push(row);
                row_count += 1;
            }
        }

        // Render LaTeX tabular.
        let latex = render_tabular(
            &col_names,
            &rows,
            &col_spec,
            &self.format,
            self.caption.as_deref(),
            self.label.as_deref(),
        );

        // Build output DataFrame.
        let out_schema: SchemaRef = Arc::new(Schema::new(vec![Field::new(
            "latex",
            DataType::Utf8,
            false,
        )]));
        let cols: Vec<ArrayRef> = vec![Arc::new(StringArray::from(vec![latex.as_str()]))];
        let batch = RecordBatch::try_new(out_schema, cols).map_err(|e| DagError::NodeError {
            node_type: "table_from_df".into(),
            msg: format!("failed to build output batch: {e}"),
        })?;

        let session = node_ctx.session();
        let out_df = session.read_batch(batch).map_err(|e| DagError::NodeError {
            node_type: "table_from_df".into(),
            msg: format!("read_batch failed: {e}"),
        })?;

        let mut out: PortOutputs = PortOutputs::new();
        out.insert(0, out_df);
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a cell value as a string from an Arrow array at a given row index.
///
/// Nulls render as empty cells. Unsupported column types are an explicit
/// error — previously they fell through to a silent empty string, producing
/// blank table columns (e.g. every unsigned integer type).
fn arrow_cell_to_string(
    array: &dyn arrow_array::Array,
    row: usize,
    column: &str,
) -> Result<String, String> {
    use arrow_array::Array;
    use arrow_schema::{DataType, TimeUnit};

    macro_rules! cell {
        ($array_type:ty, $value:expr) => {{
            let arr = array
                .as_any()
                .downcast_ref::<$array_type>()
                .expect("array type was matched on data_type");
            $value(arr.value(row))
        }};
    }

    if row >= array.len() {
        return Err(format!(
            "column `{column}`: row index {row} is out of bounds ({} rows)",
            array.len()
        ));
    }
    if array.is_null(row) {
        return Ok(String::new());
    }

    let rendered = match array.data_type() {
        DataType::Utf8 => cell!(StringArray, |v: &str| v.to_string()),
        DataType::LargeUtf8 => {
            cell!(arrow_array::LargeStringArray, |v: &str| v.to_string())
        }
        DataType::Utf8View => {
            cell!(arrow_array::StringViewArray, |v: &str| v.to_string())
        }
        DataType::Boolean => cell!(arrow_array::BooleanArray, |v: bool| v.to_string()),
        DataType::Int8 => cell!(arrow_array::Int8Array, |v: i8| v.to_string()),
        DataType::Int16 => cell!(arrow_array::Int16Array, |v: i16| v.to_string()),
        DataType::Int32 => cell!(arrow_array::Int32Array, |v: i32| v.to_string()),
        DataType::Int64 => cell!(arrow_array::Int64Array, |v: i64| v.to_string()),
        DataType::UInt8 => cell!(arrow_array::UInt8Array, |v: u8| v.to_string()),
        DataType::UInt16 => cell!(arrow_array::UInt16Array, |v: u16| v.to_string()),
        DataType::UInt32 => cell!(arrow_array::UInt32Array, |v: u32| v.to_string()),
        DataType::UInt64 => cell!(arrow_array::UInt64Array, |v: u64| v.to_string()),
        DataType::Float32 => {
            cell!(arrow_array::Float32Array, |v: f32| format_float(f64::from(
                v
            )))
        }
        DataType::Float64 => cell!(arrow_array::Float64Array, |v: f64| format_float(v)),
        DataType::Date32 => cell!(arrow_array::Date32Array, |v: i32| {
            format_date(i64::from(v))
        }),
        DataType::Date64 => cell!(arrow_array::Date64Array, |v: i64| {
            format_date(v.div_euclid(86_400_000))
        }),
        DataType::Time32(TimeUnit::Second) => cell!(arrow_array::Time32SecondArray, |v: i32| {
            format_time(i64::from(v) * 1_000_000_000, 3)
        }),
        DataType::Time32(TimeUnit::Millisecond) => {
            cell!(arrow_array::Time32MillisecondArray, |v: i32| format_time(
                i64::from(v) * 1_000_000,
                3
            ))
        }
        DataType::Time64(TimeUnit::Microsecond) => {
            cell!(arrow_array::Time64MicrosecondArray, |v: i64| format_time(
                v * 1_000,
                6
            ))
        }
        DataType::Time64(TimeUnit::Nanosecond) => {
            cell!(arrow_array::Time64NanosecondArray, |v: i64| format_time(
                v, 9
            ))
        }
        DataType::Timestamp(TimeUnit::Second, _) => {
            cell!(arrow_array::TimestampSecondArray, |v: i64| {
                format_datetime(v, 0)
            })
        }
        DataType::Timestamp(TimeUnit::Millisecond, _) => {
            cell!(arrow_array::TimestampMillisecondArray, |v: i64| {
                format_datetime(v.div_euclid(1_000), v.rem_euclid(1_000) * 1_000_000)
            })
        }
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            cell!(arrow_array::TimestampMicrosecondArray, |v: i64| {
                format_datetime(v.div_euclid(1_000_000), v.rem_euclid(1_000_000) * 1_000)
            })
        }
        DataType::Timestamp(TimeUnit::Nanosecond, _) => {
            cell!(arrow_array::TimestampNanosecondArray, |v: i64| {
                format_datetime(v.div_euclid(1_000_000_000), v.rem_euclid(1_000_000_000))
            })
        }
        other => {
            return Err(format!(
                "column `{column}` has unsupported type {other} for LaTeX table rendering \
                 (supported: Utf8/LargeUtf8/Utf8View, Boolean, Int8-64, UInt8-64, Float32/64, \
                 Date32/64, Time32/64, Timestamp)",
            ));
        }
    };
    Ok(rendered)
}

/// Render days-since-1970-01-01 as `YYYY-MM-DD` (proleptic Gregorian).
///
/// Howard Hinnant's `civil_from_days` algorithm — valid for any i64 day
/// count, no external date dependency.
fn format_date(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// (year, month, day) from days since the Unix epoch.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (year + i64::from(month <= 2), month, day)
}

/// Render sub-second time-of-day: `value` in nanoseconds, `precision` in
/// digits (3 = ms, 6 = µs, 9 = ns). Fractional digits are dropped when zero.
fn format_time(nanos: i64, precision: usize) -> String {
    let seconds = nanos.div_euclid(1_000_000_000);
    let frac = nanos.rem_euclid(1_000_000_000);
    let hh = seconds / 3_600;
    let mm = (seconds % 3_600) / 60;
    let ss = seconds % 60;
    if frac == 0 {
        format!("{hh:02}:{mm:02}:{ss:02}")
    } else {
        let frac_str = format!("{frac:09}");
        let kept = &frac_str[..precision];
        format!("{hh:02}:{mm:02}:{ss:02}.{kept}")
    }
}

/// Render seconds-since-epoch (+ nanosecond remainder) as
/// `YYYY-MM-DD HH:MM:SS[.frac]` (UTC, no timezone suffix).
fn format_datetime(seconds: i64, nanos: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let tod = seconds.rem_euclid(86_400);
    let time = if nanos == 0 {
        format_time(tod * 1_000_000_000, 9)
    } else {
        format_time(tod * 1_000_000_000 + nanos, 9)
    };
    format!("{} {time}", format_date(days))
}

fn format_float(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

/// Render a complete LaTeX table float.
fn render_tabular(
    header: &[String],
    rows: &[Vec<String>],
    col_spec: &str,
    format: &str,
    caption: Option<&str>,
    label: Option<&str>,
) -> String {
    let mut out = String::with_capacity(1024);
    let use_booktabs = format != "plain";

    // Table float wrapper.
    out.push_str("\\begin{table}[htbp]\n");
    out.push_str("  \\centering\n");

    if let Some(cap) = caption {
        out.push_str(&format!("  \\caption{{{cap}}}\n"));
    }

    // Tabular environment.
    out.push_str(&format!("  \\begin{{tabular}}{{{col_spec}}}\n"));

    if use_booktabs {
        out.push_str("    \\toprule\n");
    } else {
        out.push_str("    \\hline\n");
    }

    // Header row.
    let header_escaped: Vec<String> = header.iter().map(|h| escape_latex_text(h)).collect();
    out.push_str(&format!("    {} \\\\\n", header_escaped.join(" & ")));

    if use_booktabs {
        out.push_str("    \\midrule\n");
    } else {
        out.push_str("    \\hline\n");
    }

    // Data rows.
    for row in rows {
        let cells: Vec<String> = row.iter().map(|c| escape_latex_text(c)).collect();
        out.push_str(&format!("    {} \\\\\n", cells.join(" & ")));
    }

    if use_booktabs {
        out.push_str("    \\bottomrule\n");
    } else {
        out.push_str("    \\hline\n");
    }

    out.push_str("  \\end{tabular}\n");

    if let Some(lbl) = label {
        out.push_str(&format!("  \\label{{{lbl}}}\n"));
    }

    out.push_str("\\end{table}");
    out
}

/// Escape special LaTeX characters in plain text.
fn escape_latex_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '$' => out.push_str("\\$"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_booktabs_table() {
        let latex = render_tabular(
            &["Method".into(), "Beta".into(), "P".into()],
            &[
                vec!["IVW".into(), "0.42".into(), "0.001".into()],
                vec!["Egger".into(), "0.38".into(), "0.005".into()],
            ],
            "lll",
            "booktabs",
            Some("MR results"),
            Some("tab:mr"),
        );
        assert!(latex.contains("\\begin{table}"));
        assert!(latex.contains("\\begin{tabular}{lll}"));
        assert!(latex.contains("\\toprule"));
        assert!(latex.contains("Method & Beta & P"));
        assert!(latex.contains("IVW & 0.42 & 0.001"));
        assert!(latex.contains("\\midrule"));
        assert!(latex.contains("\\bottomrule"));
        assert!(latex.contains("\\caption{MR results}"));
        assert!(latex.contains("\\label{tab:mr}"));
    }

    #[test]
    fn render_plain_table() {
        let latex = render_tabular(
            &["A".into(), "B".into()],
            &[vec!["1".into(), "2".into()]],
            "cc",
            "plain",
            None,
            None,
        );
        assert!(latex.contains("\\hline"));
        assert!(!latex.contains("\\toprule"));
        assert!(latex.contains("A & B"));
        assert!(latex.contains("1 & 2"));
    }

    #[test]
    fn escape_special_chars() {
        assert_eq!(escape_latex_text("100%"), "100\\%");
        assert_eq!(escape_latex_text("a_b"), "a\\_b");
        assert_eq!(escape_latex_text("x & y"), "x \\& y");
    }

    // ---- cell rendering type coverage ----

    use arrow_array::{
        ArrayRef, BinaryArray, Date32Array, Date64Array, Float64Array, Int64Array,
        Time32SecondArray, Time64NanosecondArray, TimestampMicrosecondArray, UInt8Array,
        UInt16Array, UInt32Array, UInt64Array,
    };

    fn cell_of(array: &ArrayRef, row: usize, column: &str) -> String {
        arrow_cell_to_string(array.as_ref(), row, column).unwrap()
    }

    #[test]
    fn unsigned_int_columns_render_values_and_nulls() {
        let u8_col: ArrayRef = Arc::new(UInt8Array::from(vec![Some(7), None]));
        let u16_col: ArrayRef = Arc::new(UInt16Array::from(vec![Some(300), Some(65535)]));
        let u32_col: ArrayRef = Arc::new(UInt32Array::from(vec![Some(4_000_000_000), None]));
        let u64_col: ArrayRef = Arc::new(UInt64Array::from(vec![
            Some(18_446_744_073_709_551_615),
            Some(0),
        ]));
        // Regression: every unsigned width used to fall through to "".
        assert_eq!(cell_of(&u8_col, 0, "u8"), "7");
        assert_eq!(cell_of(&u8_col, 1, "u8"), "");
        assert_eq!(cell_of(&u16_col, 1, "u16"), "65535");
        assert_eq!(cell_of(&u32_col, 0, "u32"), "4000000000");
        assert_eq!(cell_of(&u32_col, 1, "u32"), "");
        assert_eq!(cell_of(&u64_col, 0, "u64"), "18446744073709551615");
        // Previously-covered types keep rendering.
        let i64_col: ArrayRef = Arc::new(Int64Array::from(vec![Some(-5)]));
        let f64_col: ArrayRef = Arc::new(Float64Array::from(vec![Some(1.25)]));
        assert_eq!(cell_of(&i64_col, 0, "i64"), "-5");
        assert_eq!(cell_of(&f64_col, 0, "f64"), "1.25");
    }

    #[test]
    fn date_and_time_columns_render_iso_values() {
        // Day counts verified against the proleptic Gregorian calendar:
        // 0 = 1970-01-01, 59 = 1970-03-01, 365 = 1971-01-01 (1970 has 365
        // days), 366 = 1971-01-02, 10957 = 2000-01-01 (30·365 + 7 leap
        // days), 11017 = 2000-03-01.
        for (days, expected) in [
            (0, "1970-01-01"),
            (59, "1970-03-01"),
            (365, "1971-01-01"),
            (366, "1971-01-02"),
            (10957, "2000-01-01"),
            (11017, "2000-03-01"),
            (-1, "1969-12-31"),
        ] {
            let d32: ArrayRef = Arc::new(Date32Array::from(vec![days as i32]));
            assert_eq!(cell_of(&d32, 0, "d"), expected);
            let d64: ArrayRef = Arc::new(Date64Array::from(vec![days * 86_400_000 + 999_999]));
            assert_eq!(cell_of(&d64, 0, "d"), expected);
        }
        let t32: ArrayRef = Arc::new(Time32SecondArray::from(vec![3661]));
        assert_eq!(cell_of(&t32, 0, "t"), "01:01:01");
        let t64: ArrayRef = Arc::new(Time64NanosecondArray::from(vec![12_345_678_901_234]));
        assert_eq!(cell_of(&t64, 0, "t"), "03:25:45.678901234");
        // 1_700_000_000 s since epoch = 2023-11-14 22:13:20 UTC.
        let ts: ArrayRef = Arc::new(TimestampMicrosecondArray::from(vec![1_700_000_000_000_000]));
        assert_eq!(cell_of(&ts, 0, "ts"), "2023-11-14 22:13:20");
    }

    #[test]
    fn unsupported_column_type_errors_instead_of_rendering_blank() {
        let binary: ArrayRef = Arc::new(BinaryArray::from_vec(vec![b"\x01\x02"]));
        let error = arrow_cell_to_string(binary.as_ref(), 0, "blob").unwrap_err();
        assert!(
            error.contains("column `blob` has unsupported type Binary"),
            "{error}"
        );
        // Out-of-bounds rows are caught explicitly too.
        let col: ArrayRef = Arc::new(Int64Array::from(vec![Some(1)]));
        let error = arrow_cell_to_string(col.as_ref(), 5, "x").unwrap_err();
        assert!(error.contains("out of bounds"), "{error}");
    }

    #[tokio::test]
    async fn execute_renders_unsigned_and_date_columns_end_to_end() {
        use dag_core::registry::NodeCtx;

        let schema = Arc::new(Schema::new(vec![
            Field::new("gene", arrow_schema::DataType::Utf8, false),
            Field::new("n_snps", arrow_schema::DataType::UInt32, true),
            Field::new("reads", arrow_schema::DataType::UInt64, true),
            Field::new("peak", arrow_schema::DataType::Date32, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["GENE1", "GENE2"])),
                Arc::new(UInt32Array::from(vec![Some(12), None])),
                Arc::new(UInt64Array::from(vec![Some(9_000_000_000_000), Some(3)])),
                Arc::new(Date32Array::from(vec![10957, 11017])),
            ],
        )
        .unwrap();
        let session = datafusion::prelude::SessionContext::new();
        let df = session.read_batch(batch).unwrap();

        let mut node = TableFromDfFactory {}
            .build(
                serde_json::json!({}),
                NodeCtx::new(session.runtime_env(), None),
            )
            .unwrap();
        let out = node
            .execute(
                &NodeCtx::new(session.runtime_env(), None),
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let batches = out.dataframe(0).unwrap().clone().collect().await.unwrap();
        let latex = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(0)
            .to_string();
        // Unsigned values render; the null UInt32 cell stays empty.
        assert!(
            latex.contains("GENE1 & 12 & 9000000000000 & 2000-01-01"),
            "{latex}"
        );
        assert!(latex.contains("GENE2 &  & 3 & 2000-03-01"), "{latex}");
    }
}
