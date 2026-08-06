//! OpenGWAS tophits source node.
//!
//! [`OpengwasTophitsNode`] (`source_opengwas_tophits`) calls the OpenGWAS
//! `POST /tophits` endpoint and emits the top-associated SNPs as a
//! DataFusion `DataFrame`. The schema is inferred dynamically from the
//! JSON response — OpenGWAS may return slightly different field sets
//! depending on the GWAS dataset, so the node adapts rather than imposing
//! a fixed schema.
//!
//! This is a zero-input / single-output source node. It requires the
//! `OPENGWAS_TOKEN` environment variable.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::common::HashMap;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use crate::dag::{DagError, graph::PortOutputs};
use crate::node_registry::registry::{NodeCtx, NodeFactory};
use crate::nodes::meta::{DagNode, NodePorts};

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

fn default_pval() -> f64 {
    5e-8
}
fn default_clump() -> i32 {
    1
}
fn default_r2() -> f64 {
    0.001
}
fn default_kb() -> i32 {
    5000
}
fn default_pop() -> String {
    "EUR".to_string()
}

/// Spec for [`OpengwasTophitsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasTophitsSpec {
    /// GWAS study IDs to query, e.g. `["ukb-b-19953"]`.
    pub id: Vec<String>,
    /// P-value threshold (must be ≤ 0.01). Default `5e-8`.
    #[serde(default = "default_pval")]
    pub pval: f64,
    /// Whether to clump results server-side: `1` (yes) or `0` (no). Default `1`.
    #[serde(default = "default_clump")]
    pub clump: i32,
    /// Clumping r² threshold. Default `0.001`.
    #[serde(default = "default_r2")]
    pub r2: f64,
    /// Clumping window size in kb. Default `5000`.
    #[serde(default = "default_kb")]
    pub kb: i32,
    /// Reference population for clumping: `"EUR"`, `"SAS"`, `"EAS"`,
    /// `"AFR"`, or `"AMR"`. Default `"EUR"`.
    #[serde(default = "default_pop")]
    pub pop: String,
}

// ---------------------------------------------------------------------------
// Node + Factory
// ---------------------------------------------------------------------------

const SOURCE_OPENGWAS_TOPHITS_KIND: &str = "source_opengwas_tophits";

/// Source node that fetches top GWAS hits from OpenGWAS `/tophits`.
#[derive(Clone)]
pub struct OpengwasTophitsNode {
    meta: NodePorts,
    spec: OpengwasTophitsSpec,
}

impl OpengwasTophitsNode {
    pub fn new(spec: OpengwasTophitsSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
        }
    }
}

pub struct OpengwasTophitsNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for OpengwasTophitsNodeFactory {
    fn kind(&self) -> &'static str {
        SOURCE_OPENGWAS_TOPHITS_KIND
    }

    fn desc(&self) -> &'static str {
        "Fetches top GWAS hits from the OpenGWAS /tophits endpoint as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/tophits` endpoint for \
        top-associated SNPs (optionally clumped) and emits them as a \
        DataFrame. No input ports; one output port.\n\n\
        Requires the `OPENGWAS_TOKEN` environment variable.\n\n\
        The output schema is inferred from the API response — typically \
        `rsid`, `chr`, `position`, `ea`, `nea`, `eaf`, `beta`, `se`, `pval`, \
        `samplesize`, `ncontrol`, `ncase`, etc."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasTophitsSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpengwasTophitsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasTophitsNode::new(node_spec)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let s = parse_spec::<OpengwasTophitsSpec>(spec, SOURCE_OPENGWAS_TOPHITS_KIND)?;
        let out = ctx.output_var.to_string();
        let ids = s.id.join("\", \"");
        let code = vec![
            format!("# OpenGWAS tophits for: [\"{}\"]", ids),
            format!("# NOTE: requires the TwoSampleMR R package and OPENGWAS_TOKEN"),
            format!("ao <- extract_outcome_data(snps = c(), outcomes = c(\"{}\"))", ids),
            format!(
                "# Top hits with pval ≤ {} (clump={}, pop=\"{}\")",
                s.pval, s.clump, s.pop
            ),
            format!(
                "{out} <- ao[ao$pval.outcome <= {}, ]",
                s.pval
            ),
        ];
        Ok(crate::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["TwoSampleMR".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasTophitsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        SOURCE_OPENGWAS_TOPHITS_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[crate::dag::NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = opengwas::OpengwasClient::new(None).map_err(|e| {
            DagError::Schedule(format!(
                "failed to create OpenGWAS client (is OPENGWAS_TOKEN set?): {e}"
            ))
        })?;

        let req = opengwas::types::TophitsRequest {
            id: self.spec.id.clone(),
            pval: Some(self.spec.pval),
            preclumped: None,
            clump: Some(self.spec.clump),
            r2: Some(self.spec.r2),
            kb: Some(self.spec.kb),
            pop: Some(self.spec.pop.clone()),
            commercial_approval_received: None,
        };

        let resp = client
            .tophits(&req)
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /tophits request failed: {e}")))?;

        let rows = extract_rows(&resp);
        tracing::info!(
            "OpenGWAS tophits: {} SNPs returned for {} (pval={}, clump={})",
            rows.len(),
            self.spec.id.join(", "),
            self.spec.pval,
            self.spec.clump,
        );

        let batch = build_tophits_batch(&rows)?;
        let session = node_ctx.session();
        let df = session
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read tophits batch: {e}")))?;

        let mut res: PortOutputs = HashMap::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ---------------------------------------------------------------------------
// JSON → Arrow conversion
// ---------------------------------------------------------------------------

/// GWAS field ordering priority — matches the column conventions used by the
/// `opengwas` format module so the output schema reads naturally.
const FIELD_PRIORITY: &[&str] = &[
    "rsid",
    "chr",
    "chromosome",
    "position",
    "pos",
    "ea",
    "nea",
    "eaf",
    "beta",
    "se",
    "pval",
    "p",
    "samplesize",
    "sample_size",
    "ncase",
    "ncontrol",
    "unit",
    "population",
    "trait",
    "study_id",
    "id",
];

/// Extract a flat row array from the API response. The response may be a
/// bare JSON array, or an object keyed by study ID whose values are arrays
/// (mirrors `opengwas::format::extract_rows`).
fn extract_rows(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(arr) => arr.clone(),
        Value::Object(map) => {
            let all_arrays = !map.is_empty() && map.values().all(|v| v.is_array());
            if all_arrays {
                map.values()
                    .flat_map(|v| v.as_array().into_iter().flatten().cloned())
                    .collect()
            } else {
                vec![value.clone()]
            }
        }
        _ => vec![value.clone()],
    }
}

/// Infer the column set from the union of keys across all rows, ordered by
/// [`FIELD_PRIORITY`] then alphabetical for any remaining keys.
fn infer_columns(rows: &[Value]) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if seen.insert(key.clone()) {
                    columns.push(key.clone());
                }
            }
        }
    }
    columns.sort_by(|a, b| {
        let ai = FIELD_PRIORITY.iter().position(|&p| p == a).unwrap_or(999);
        let bi = FIELD_PRIORITY.iter().position(|&p| p == b).unwrap_or(999);
        ai.cmp(&bi).then_with(|| a.cmp(b))
    });
    columns
}

/// Infer the Arrow [`DataType`] for a column by scanning all non-null values
/// in that column. If every non-null value is an integer → `Int64`; if every
/// non-null value is numeric (integer or float) → `Float64`; otherwise `Utf8`.
fn infer_type(rows: &[Value], key: &str) -> DataType {
    let mut all_int = true;
    let mut all_num = true;
    let mut any_non_null = false;
    for row in rows {
        match row.get(key) {
            Some(Value::Number(n)) => {
                any_non_null = true;
                if n.is_i64() || n.is_u64() {
                    // integer
                } else {
                    all_int = false;
                }
            }
            Some(Value::String(_)) => {
                any_non_null = true;
                all_int = false;
                all_num = false;
            }
            Some(Value::Bool(_)) => {
                any_non_null = true;
                all_int = false;
                all_num = false;
            }
            Some(Value::Null) | None => {}
            Some(_) => {
                any_non_null = true;
                all_int = false;
                all_num = false;
            }
        }
    }
    if !any_non_null {
        DataType::Utf8 // all-null column, default to string
    } else if all_int {
        DataType::Int64
    } else if all_num {
        DataType::Float64
    } else {
        DataType::Utf8
    }
}

/// Build an Arrow [`RecordBatch`] from the JSON row array, inferring the
/// schema dynamically.
fn build_tophits_batch(rows: &[Value]) -> Result<RecordBatch, DagError> {
    if rows.is_empty() {
        return Err(DagError::Schedule(
            "OpenGWAS /tophits returned no rows".into(),
        ));
    }

    let columns = infer_columns(rows);
    let fields: Vec<Field> = columns
        .iter()
        .map(|name| Field::new(name, infer_type(rows, name), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(columns.len());
    for col in &columns {
        let dtype = schema.field_with_name(col).unwrap().data_type().clone();
        let arr = build_column(rows, col, &dtype);
        arrays.push(arr);
    }

    RecordBatch::try_new(schema, arrays)
        .map_err(|e| DagError::Schedule(format!("failed to build tophits batch: {e}")))
}

/// Build a single column array from the JSON values.
fn build_column(rows: &[Value], key: &str, dtype: &DataType) -> Arc<dyn Array> {
    match dtype {
        DataType::Int64 => {
            let vals: Vec<Option<i64>> = rows
                .iter()
                .map(|r| r.get(key).and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64))))
                .collect();
            Arc::new(Int64Array::from(vals))
        }
        DataType::Float64 => {
            let vals: Vec<Option<f64>> = rows
                .iter()
                .map(|r| r.get(key).and_then(|v| v.as_f64()))
                .collect();
            Arc::new(Float64Array::from(vals))
        }
        _ => {
            // String column — stringify any JSON value.
            let vals: Vec<Option<String>> = rows
                .iter()
                .map(|r| match r.get(key) {
                    Some(Value::Null) | None => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Number(n)) => Some(n.to_string()),
                    Some(Value::Bool(b)) => Some(b.to_string()),
                    Some(other) => Some(other.to_string()),
                })
                .collect();
            let refs: Vec<Option<&str>> = vals.iter().map(|o| o.as_deref()).collect();
            Arc::new(StringArray::from(refs))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_rows_flat_array() {
        let val = serde_json::json!([
            {"rsid": "rs1", "pval": 1e-8},
            {"rsid": "rs2", "pval": 1e-9},
        ]);
        let rows = extract_rows(&val);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn extract_rows_keyed_object() {
        let val = serde_json::json!({
            "ieu-a-2": [{"rsid": "rs1"}],
            "ukb-b-1": [{"rsid": "rs2"}, {"rsid": "rs3"}],
        });
        let rows = extract_rows(&val);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn infer_columns_priority_order() {
        let rows = serde_json::json!([
            {"samplesize": 1000, "rsid": "rs1", "pval": 5e-8, "beta": 0.1, "zzz": "extra"},
        ]);
        let rows = extract_rows(&rows);
        let cols = infer_columns(&rows);
        // rsid first (priority 0), then pval (priority 10), beta (priority 8),
        // samplesize (priority 12), then "zzz" alphabetical last.
        assert_eq!(cols[0], "rsid");
        assert_eq!(cols.last().unwrap(), "zzz");
    }

    #[test]
    fn build_batch_infers_types() {
        let rows = extract_rows(&serde_json::json!([
            {"rsid": "rs1", "chr": 1, "position": 12345, "beta": 0.05, "pval": 5e-8},
            {"rsid": "rs2", "chr": 2, "position": 67890, "beta": -0.03, "pval": 1e-9},
        ]));
        let batch = build_tophits_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 2);

        // chr / position → Int64 (all integers)
        let chr = batch
            .column_by_name("chr")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(chr.value(0), 1);
        assert_eq!(chr.value(1), 2);

        // beta / pval → Float64 (has fractional values)
        let beta = batch
            .column_by_name("beta")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((beta.value(0) - 0.05).abs() < 1e-10);

        // rsid → Utf8
        let rsid = batch
            .column_by_name("rsid")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(rsid.value(0), "rs1");
    }

    #[test]
    fn build_batch_handles_nulls() {
        let rows = extract_rows(&serde_json::json!([
            {"rsid": "rs1", "pval": 5e-8},
            {"rsid": "rs2", "pval": null},
        ]));
        let batch = build_tophits_batch(&rows).unwrap();
        let pval = batch
            .column_by_name("pval")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((pval.value(0) - 5e-8).abs() < 1e-20);
        assert!(pval.is_null(1));
    }

    #[test]
    fn build_batch_empty_rows_errors() {
        let rows: Vec<Value> = vec![];
        assert!(build_tophits_batch(&rows).is_err());
    }

    #[test]
    fn spec_deserialises() {
        let json = serde_json::json!({
            "id": ["ukb-b-19953"],
        });
        let spec: OpengwasTophitsSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.id, vec!["ukb-b-19953"]);
        assert!((spec.pval - 5e-8).abs() < f64::EPSILON);
        assert_eq!(spec.clump, 1);
        assert_eq!(spec.pop, "EUR");
    }

    #[test]
    fn spec_with_overrides() {
        let json = serde_json::json!({
            "id": ["ieu-a-2", "ukb-b-1"],
            "pval": 1e-5,
            "clump": 0,
            "r2": 0.01,
            "kb": 1000,
            "pop": "EAS",
        });
        let spec: OpengwasTophitsSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.id.len(), 2);
        assert!((spec.pval - 1e-5).abs() < f64::EPSILON);
        assert_eq!(spec.clump, 0);
        assert!((spec.r2 - 0.01).abs() < f64::EPSILON);
        assert_eq!(spec.kb, 1000);
        assert_eq!(spec.pop, "EAS");
    }
}
