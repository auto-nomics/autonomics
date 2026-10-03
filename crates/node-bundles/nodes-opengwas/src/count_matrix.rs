//! Scalar and matrix OpenGWAS operations as DAG nodes — the M3-② spike for
//! the survey's T2 rule ("no new `PortType`s"):
//!
//! - [`OpengwasGwasinfoCountNode`] (`source_opengwas_gwasinfo_count`) — the
//!   scalar dataset count as a **single-row DataFrame**.
//! - [`OpengwasLdMatrixNode`] (`source_opengwas_ld_matrix`) — the N×N LD
//!   matrix as a **long-format DataFrame** (`rsid_a, rsid_b, r`), which is
//!   SQL-pivotable and sidesteps the wide-matrix question entirely: no
//!   parquet detour, no port-model extension.
//!
//! Both were previously tool-only (`opengwas_gwasinfo_count`,
//! `opengwas_ld_matrix`) under the OpenGWAS precedent rule "tables → nodes;
//! scalars/matrices/files → tools". These nodes are the proof that the
//! exception class collapses into the DataFrame channel.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use dag_core::dag::{DagError, DagNode, graph::PortOutputs};
use dag_core::node::{DagNode as _, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::shared::{make_client, single_output_port};

// ---------------------------------------------------------------------------
// gwasinfo_count — scalar → single-row DataFrame
// ---------------------------------------------------------------------------

/// Spec for [`OpengwasGwasinfoCountNode`].
#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct OpengwasGwasinfoCountSpec {}

/// Source node emitting the cached-dataset count as one row / one column.
#[derive(Clone)]
pub struct OpengwasGwasinfoCountNode {
    meta: NodePorts,
    #[allow(dead_code)]
    spec: OpengwasGwasinfoCountSpec,
}

pub struct OpengwasGwasinfoCountNodeFactory;

impl NodeFactory for OpengwasGwasinfoCountNodeFactory {
    fn kind(&self) -> &'static str {
        "source_opengwas_gwasinfo_count"
    }

    fn desc(&self) -> &'static str {
        "Emit the number of cached OpenGWAS datasets as a one-row table."
    }

    fn doc(&self) -> &'static str {
        "A source node over the local OpenGWAS dataset cache — the scalar \
         `gwasinfo_count` as a single-row DataFrame (`count` column), so \
         even scalar lookups ride the DataFrame channel instead of the tool \
         channel. Spec takes no fields.\n\n\
         Output schema: `count` (Int64, exactly one row)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasGwasinfoCountSpec)
    }

    fn ports(&self) -> NodePorts {
        single_output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: OpengwasGwasinfoCountSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasGwasinfoCountNode {
            meta: single_output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpengwasGwasinfoCountNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_opengwas_gwasinfo_count"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        let count = client
            .gwasinfo_count()
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS gwasinfo_count failed: {e}")))?;
        emit_single_row_count(ctx, count)
    }
}

fn emit_single_row_count(ctx: &NodeCtx, count: i64) -> Result<PortOutputs, DagError> {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "count",
        DataType::Int64,
        true,
    )]));
    let batch = arrow_array::RecordBatch::try_new(
        schema,
        vec![Arc::new(Int64Array::from(vec![Some(count)]))],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build count batch: {e}")))?;
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::Schedule(format!("failed to read count batch: {e}")))?;
    let mut outputs = PortOutputs::new();
    outputs.insert(0, df);
    Ok(outputs)
}

// ---------------------------------------------------------------------------
// ld_matrix — N×N matrix → long-format DataFrame
// ---------------------------------------------------------------------------

/// Spec for [`OpengwasLdMatrixNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasLdMatrixSpec {
    /// rs IDs to compute pairwise LD for (at least 2).
    pub rsid: Vec<String>,
    /// Reference population: `EUR` (default), `SAS`, `EAS`, `AFR`, `AMR`.
    #[serde(default)]
    pub pop: Option<String>,
}

/// Source node emitting pairwise LD R values as a long-format table.
#[derive(Clone)]
pub struct OpengwasLdMatrixNode {
    meta: NodePorts,
    spec: OpengwasLdMatrixSpec,
}

pub struct OpengwasLdMatrixNodeFactory;

impl NodeFactory for OpengwasLdMatrixNodeFactory {
    fn kind(&self) -> &'static str {
        "source_opengwas_ld_matrix"
    }

    fn desc(&self) -> &'static str {
        "Emit pairwise LD R values as a long-format (rsid_a, rsid_b, r) table."
    }

    fn doc(&self) -> &'static str {
        "A source node over OpenGWAS `POST /ld/matrix` — the N×N LD matrix \
         flattened into long format: one row per `(rsid_a, rsid_b)` pair \
         with its R value. Long format is SQL-native: `SELECT * WHERE \
         rsid_a = 'rs1'` reads a row; a wide matrix re-forms with \
         `CREATE VIEW ... PIVOT`-style SQL if a downstream consumer wants \
         it. Requires at least two rs IDs.\n\n\
         `pop` selects the LD reference population (`EUR` default).\n\n\
         Output schema: `rsid_a, rsid_b, r` (rsid labels carry the API's \
         `_REF_ALT` suffixes stripped, matching the tool's rendering)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasLdMatrixSpec)
    }

    fn ports(&self) -> NodePorts {
        single_output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: OpengwasLdMatrixSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasLdMatrixNode {
            meta: single_output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for OpengwasLdMatrixNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_opengwas_ld_matrix"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.spec.rsid.len() < 2 {
            return Err(DagError::Schedule(format!(
                "source_opengwas_ld_matrix requires at least 2 rs IDs; got {}",
                self.spec.rsid.len()
            )));
        }
        let client = make_client()?;
        let value = client
            .ld_matrix(&opengwas::types::LdMatrixRequest {
                rsid: self.spec.rsid.clone(),
                pop: self.spec.pop.clone(),
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS ld_matrix failed: {e}")))?;
        let pairs = flatten_ld_matrix(&value)?;
        if pairs.is_empty() {
            return Err(DagError::Schedule(
                "source_opengwas_ld_matrix: the API returned no LD pairs for the given \
                 rs IDs (are they in the reference panel?)"
                    .into(),
            ));
        }
        let batch = build_ld_pairs_batch(&pairs)?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read LD batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

/// One `(rsid_a, rsid_b, r)` triple; rsid labels have `_REF_ALT` suffixes
/// stripped the same way the tool's renderer does.
pub(crate) type LdPair = (String, String, f64);

/// Strip a trailing `_REF_ALT` allele suffix (`rs123_A_G` → `rs123`).
fn strip_allele_suffix(label: &str) -> String {
    match label.split_once('_') {
        Some((stem, _)) if stem.starts_with("rs") => stem.to_string(),
        _ => label.to_string(),
    }
}

/// Flatten either LD API response shape into ordered pairs:
/// canonical `{snplist: [..], matrix: [[..], ..]}` or nested
/// `{rsid: {other_rsid: r}}`.
pub(crate) fn flatten_ld_matrix(value: &Value) -> Result<Vec<LdPair>, DagError> {
    if let Some(obj) = value.as_object() {
        if let (Some(snplist), Some(matrix)) = (obj.get("snplist"), obj.get("matrix")) {
            if let (Some(snps), Some(rows)) = (snplist.as_array(), matrix.as_array()) {
                let labels: Vec<String> = snps
                    .iter()
                    .map(|v| strip_allele_suffix(&scalar_label(v)))
                    .collect();
                let mut pairs = Vec::new();
                for (i, row) in rows.iter().enumerate() {
                    let cells = row.as_array().ok_or_else(|| {
                        DagError::Schedule("LD matrix row is not an array".into())
                    })?;
                    for (j, cell) in cells.iter().enumerate() {
                        if let Some(r) = cell.as_f64() {
                            let a = labels.get(i).cloned().unwrap_or_else(|| i.to_string());
                            let b = labels.get(j).cloned().unwrap_or_else(|| j.to_string());
                            pairs.push((a, b, r));
                        }
                    }
                }
                return Ok(pairs);
            }
        }
    }

    if let Some(map) = value.as_object() {
        if !map.is_empty() && map.values().all(|v| v.is_object()) {
            // Nested shape: {rsid: {other: r}} — sort keys so the pair order
            // is deterministic regardless of JSON map ordering.
            let mut pairs: Vec<LdPair> = Vec::new();
            let mut outer: BTreeMap<&str, &serde_json::Map<String, Value>> = BTreeMap::new();
            for (k, v) in map {
                if let Some(inner) = v.as_object() {
                    outer.insert(k.as_str(), inner);
                }
            }
            for (a, inner) in outer {
                let mut cells: BTreeMap<&str, f64> = BTreeMap::new();
                for (b, r) in inner {
                    if let Some(r) = r.as_f64() {
                        cells.insert(b.as_str(), r);
                    }
                }
                for (b, r) in cells {
                    pairs.push((strip_allele_suffix(a), strip_allele_suffix(b), r));
                }
            }
            return Ok(pairs);
        }
    }

    Err(DagError::Schedule(
        "source_opengwas_ld_matrix: unrecognized LD response shape (expected \
         {snplist, matrix} or {rsid: {other: r}})"
            .into(),
    ))
}

fn scalar_label(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn build_ld_pairs_batch(pairs: &[LdPair]) -> Result<arrow_array::RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("rsid_a", DataType::Utf8, true),
        Field::new("rsid_b", DataType::Utf8, true),
        Field::new("r", DataType::Float64, true),
    ]));
    let strings = |slot: usize| -> Arc<dyn arrow_array::Array> {
        let vals: Vec<Option<&str>> = pairs.iter().map(|p| Some(p.slot_str(slot))).collect();
        Arc::new(StringArray::from(vals))
    };
    let values: Vec<Option<f64>> = pairs.iter().map(|p| Some(p.2)).collect();
    arrow_array::RecordBatch::try_new(
        schema,
        vec![strings(0), strings(1), Arc::new(Float64Array::from(values))],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build LD pairs batch: {e}")))
}

trait SlotStr {
    fn slot_str(&self, slot: usize) -> &str;
}

impl SlotStr for LdPair {
    fn slot_str(&self, slot: usize) -> &str {
        match slot {
            0 => &self.0,
            _ => &self.1,
        }
    }
}

#[cfg(test)]
mod count_matrix_tests {
    use super::*;

    #[test]
    fn count_node_spec_accepts_empty_object() {
        let factory = OpengwasGwasinfoCountNodeFactory;
        let node = factory
            .build(serde_json::json!({}), test_ctx())
            .expect("empty spec builds");
        assert_eq!(node.kind(), "source_opengwas_gwasinfo_count");
    }

    #[tokio::test]
    async fn scalar_count_emits_exactly_one_row_one_column() {
        use arrow_array::Array;
        let ctx = test_ctx();
        let outputs = emit_single_row_count(&ctx, 5241).unwrap();
        let value = outputs.get(&0).expect("count output");
        let df = value.as_dataframe().expect("dataframe output");
        let batches = df.clone().collect().await.unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 1);
        assert_eq!(batches[0].num_columns(), 1);
        let count = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(count.value(0), 5241);
    }

    #[test]
    fn canonical_matrix_flattens_to_ordered_pairs_with_suffix_strip() {
        let value = serde_json::json!({
            "snplist": ["rs1_A_G", "rs2_T_C", "rs3"],
            "matrix": [[1.0, 0.5, 0.1], [0.5, 1.0, 0.2], [0.1, 0.2, 1.0]]
        });
        let pairs = flatten_ld_matrix(&value).unwrap();
        assert_eq!(pairs.len(), 9);
        assert_eq!(pairs[0], ("rs1".to_string(), "rs1".to_string(), 1.0));
        assert_eq!(pairs[1], ("rs1".to_string(), "rs2".to_string(), 0.5));
        assert_eq!(pairs[5], ("rs2".to_string(), "rs3".to_string(), 0.2));
    }

    #[test]
    fn nested_matrix_flattens_deterministically() {
        let value = serde_json::json!({
            "rs2_T_C": { "rs1_A_G": 0.5, "rs3": 0.2 },
            "rs1_A_G": { "rs2_T_C": 0.5, "rs3": 0.1 }
        });
        let pairs = flatten_ld_matrix(&value).unwrap();
        assert_eq!(pairs.len(), 4);
        // Outer key order: rs1 before rs2 (BTreeMap).
        assert_eq!(pairs[0].0, "rs1");
        assert_eq!(pairs[2].0, "rs2");
        assert!((pairs[2].2 - 0.5).abs() < 1e-12);
    }

    #[test]
    fn alien_shape_is_rejected() {
        let err = flatten_ld_matrix(&serde_json::json!({ "unexpected": 1 }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unrecognized LD response shape"), "{err}");
    }

    fn test_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }
}
