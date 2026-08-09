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
         (\\toprule, \\midrule, \\bottomrule). The header row comes from the DataFrame column names."
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

        let df = &inputs[0].data;

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
                    let cell = arrow_cell_to_string(col.as_ref(), i);
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
        let batch =
            RecordBatch::try_new(out_schema, cols).map_err(|e| DagError::NodeError {
                node_type: "table_from_df".into(),
                msg: format!("failed to build output batch: {e}"),
            })?;

        let session = node_ctx.session();
        let out_df = session
            .read_batch(batch)
            .map_err(|e| DagError::NodeError {
                node_type: "table_from_df".into(),
                msg: format!("read_batch failed: {e}"),
            })?;

        let mut out: PortOutputs = HashMap::new();
        out.insert(0, out_df);
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a cell value as a string from an Arrow array at a given row index.
fn arrow_cell_to_string(array: &dyn arrow_array::Array, row: usize) -> String {
    use arrow_array::Array;

    // Try to cast to string array.
    if let Some(str_arr) = array.as_any().downcast_ref::<StringArray>() {
        if row < str_arr.len() {
            return str_arr.value(row).to_string();
        }
        return String::new();
    }

    // Try string view array (DataFusion >= 42 uses Utf8View).
    if let Some(str_arr) = array.as_any().downcast_ref::<arrow_array::StringViewArray>() {
        if row < str_arr.len() {
            return str_arr.value(row).to_string();
        }
        return String::new();
    }

    // Try numeric types via format.
    if let Some(int64) = array.as_any().downcast_ref::<arrow_array::Int64Array>() {
        if row < int64.len() {
            return int64.value(row).to_string();
        }
    }
    if let Some(int32) = array.as_any().downcast_ref::<arrow_array::Int32Array>() {
        if row < int32.len() {
            return int32.value(row).to_string();
        }
    }
    if let Some(float64) = array.as_any().downcast_ref::<arrow_array::Float64Array>() {
        if row < float64.len() {
            return format_float(float64.value(row));
        }
    }
    if let Some(float32) = array.as_any().downcast_ref::<arrow_array::Float32Array>() {
        if row < float32.len() {
            return format_float(f64::from(float32.value(row)));
        }
    }
    if let Some(bool_arr) = array.as_any().downcast_ref::<arrow_array::BooleanArray>() {
        if row < bool_arr.len() {
            return bool_arr.value(row).to_string();
        }
    }

    // Fallback: empty string.
    String::new()
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
}
