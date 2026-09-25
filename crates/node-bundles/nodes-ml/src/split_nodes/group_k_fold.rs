use super::*;

use arrow_array::StringArray;

// ═══════════════════════════════════════════════════════════════════════
// GroupKFold — all rows of a group land in the same fold
// ═══════════════════════════════════════════════════════════════════════
//
// The patient-level split primitive: `group_column` (e.g. patient_id)
// assigns every row carrying the same group value to one fold, so
// downstream CV can never leak episodes of one patient across folds.

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GroupKFoldSpec {
    /// Column whose values define groups (string or numeric).  All rows
    /// sharing a value are assigned to the same fold.
    pub group_column: String,
    /// Number of folds.
    pub k: usize,
}

pub struct GroupKFoldFactory;
impl NodeFactory for GroupKFoldFactory {
    fn kind(&self) -> &'static str {
        "ml_group_kfold"
    }
    fn desc(&self) -> &'static str {
        "Group K-Fold fold assignment (no group spans folds)."
    }
    fn doc(&self) -> &'static str {
        "GroupKFold: assigns each row a `fold` in 0..k such that every row of the same \
        group (group_column value) lands in the same fold — the patient-level split for \
        repeated-episode data. Group keys are canonicalised to strings (values rendered \
        verbatim for strings, via numeric display for numbers) and ordered lexicographically; \
        ordering only affects which fold a group lands in, never group integrity. \
        Assignment of groups to folds is deterministic round-robin."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GroupKFoldSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: GroupKFoldSpec = serde_json::from_value(spec)?;
        Ok(Box::new(GroupKFoldNode {
            group_column: s.group_column,
            k: s.k,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct GroupKFoldNode {
    group_column: String,
    k: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for GroupKFoldNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_group_kfold"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let ids = group_ids(&batches, &self.group_column)?;
        let folds = ml::split::group_kfold(&ids, self.k).map_err(|e| DagError::NodeError {
            node_type: "ml_group_kfold".into(),
            msg: e.to_string(),
        })?;

        let n = ids.len();
        let mut fold_labels = vec![0u32; n];
        for (fold_idx, (_, test)) in folds.iter().enumerate() {
            for &i in test {
                fold_labels[i] = fold_idx as u32;
            }
        }

        let batch = append_column(
            &batches,
            "fold",
            Arc::new(UInt32Array::from(fold_labels)),
            DataType::UInt32,
        )?;
        emit_batch(ctx, batch)
    }
}

// ── group column → dense group ids ──────────────────────────────────────

/// Map a string-or-numeric column to dense `0..n_groups` ids (usize).
///
/// Keys are canonicalised by [`common::group_keys`]; ids follow their
/// lexicographic order, independent of row order.
fn group_ids(batches: &[RecordBatch], name: &str) -> Result<Vec<usize>, DagError> {
    fn err(msg: impl Into<String>) -> DagError {
        DagError::NodeError {
            node_type: "ml_group_kfold".into(),
            msg: msg.into(),
        }
    }
    let keys = common::group_keys(batches, name).map_err(err)?;
    Ok(common::dense_group_ids(&keys))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_string_batch(values: Vec<Option<&str>>) -> Vec<RecordBatch> {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("pid", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(values))],
        )
        .unwrap();
        vec![batch]
    }

    fn one_numeric_batch(values: Vec<i64>) -> Vec<RecordBatch> {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("pid", DataType::Int64, true)])),
            vec![Arc::new(arrow_array::Int64Array::from(values))],
        )
        .unwrap();
        vec![batch]
    }

    #[test]
    fn test_spec_deserialize() {
        let spec: GroupKFoldSpec =
            serde_json::from_str(r#"{"group_column": "patient_id", "k": 5}"#).unwrap();
        assert_eq!(spec.group_column, "patient_id");
        assert_eq!(spec.k, 5);
    }

    #[test]
    fn test_group_ids_dense_and_stable() {
        let batches = one_string_batch(vec![
            Some("p2"),
            Some("p1"),
            Some("p2"),
            Some("p3"),
            Some("p1"),
        ]);
        let ids = group_ids(&batches, "pid").unwrap();
        // BTreeMap lexicographic: p1→0, p2→1, p3→2
        assert_eq!(ids, vec![1, 0, 1, 2, 0]);
    }

    #[test]
    fn test_group_ids_across_batches() {
        let mut batches = one_string_batch(vec![Some("b"), Some("a")]);
        batches.extend(one_string_batch(vec![Some("c"), Some("a")]));
        let ids = group_ids(&batches, "pid").unwrap();
        assert_eq!(ids, vec![1, 0, 2, 0]);
    }

    #[test]
    fn test_group_ids_numeric() {
        let batches = one_numeric_batch(vec![10, 2, 10, 7]);
        let ids = group_ids(&batches, "pid").unwrap();
        // String rendering: "10" < "2" < "7" lexicographically
        assert_eq!(ids, vec![0, 1, 0, 2]);
    }

    #[test]
    fn test_group_ids_missing_column() {
        let batches = one_string_batch(vec![Some("p1")]);
        assert!(group_ids(&batches, "nope").is_err());
    }

    #[test]
    fn test_group_ids_rejects_non_groupable_type() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "flag",
                DataType::Boolean,
                true,
            )])),
            vec![Arc::new(arrow_array::BooleanArray::from(vec![true, false]))],
        )
        .unwrap();
        assert!(group_ids(&[batch], "flag").is_err());
    }

    #[test]
    fn test_folds_keep_groups_intact() {
        // 4 patients × 2 episodes each, k=2: same patient never splits.
        let values: Vec<Option<&str>> = ["p1", "p2", "p3", "p4", "p1", "p2", "p3", "p4"]
            .into_iter()
            .map(Some)
            .collect();
        let batches = one_string_batch(values);
        let ids = group_ids(&batches, "pid").unwrap();
        let folds = ml::split::group_kfold(&ids, 2).unwrap();
        for (train, test) in &folds {
            let train_groups: std::collections::HashSet<usize> =
                train.iter().map(|&i| ids[i]).collect();
            let test_groups: std::collections::HashSet<usize> =
                test.iter().map(|&i| ids[i]).collect();
            assert!(train_groups.is_disjoint(&test_groups));
        }
    }
}
