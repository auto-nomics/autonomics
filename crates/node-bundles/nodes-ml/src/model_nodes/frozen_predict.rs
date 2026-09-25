use super::*;

use arrow_array::{BooleanArray, Float64Array};
use faer::Mat;

use ml::centroid::{PAM_KIND, PamModel, pam_predict};
use ml::multinomial::{MULTINOMIAL_ENET_KIND, MultinomialEnetModel, mnet_predict};

use crate::common;

// ═══════════════════════════════════════════════════════════════════════
// FrozenPredict — score new samples with a fitted-model artifact row
// ═══════════════════════════════════════════════════════════════════════
//
// Port 0 takes a single-row artifact table (as emitted by a fit node's p0
// or by `ml_model_load`); port 1 takes the new-sample feature table.  The
// artifact's `kind` selects the model family (`pam:v1`,
// `multinomial_enet:v1`); unknown kinds are a NodeError.  Nothing is
// retrained and no training data is touched — the whole model lives in the
// artifact bytes.
//
// Output = the port-1 columns plus, for each class, `p_<class>`, then
// `prediction` (label), `top_prob`, `margin` (top1 − top2) and
// `is_uncertain` (true when top_prob < min_top_prob or margin <
// uncertain_margin; each threshold is optional).

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FrozenPredictSpec {
    /// Flag predictions whose top probability falls below this (e.g. 0.6).
    /// None (default) disables the top-probability criterion.
    #[serde(default)]
    pub min_top_prob: Option<f64>,
    /// Flag predictions whose top1 − top2 probability margin falls below
    /// this.  None (default) disables the margin criterion.
    #[serde(default)]
    pub uncertain_margin: Option<f64>,
}

pub struct FrozenPredictFactory;
impl NodeFactory for FrozenPredictFactory {
    fn kind(&self) -> &'static str {
        "ml_frozen_predict"
    }
    fn desc(&self) -> &'static str {
        "Predict with a frozen model artifact row; no retraining."
    }
    fn doc(&self) -> &'static str {
        "ml_frozen_predict: port 0 = single-row artifact table (fit node p0 or \
        ml_model_load output), port 1 = new-sample feature table.  Dispatches on \
        the artifact kind (pam:v1, multinomial_enet:v1); unknown kinds fail.  \
        Output: input columns + p_<class> per class + prediction + top_prob + \
        margin + is_uncertain (thresholds min_top_prob / uncertain_margin, \
        both optional)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FrozenPredictSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None) // p0 — artifact row
            .add_input_port(None) // p1 — new samples
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: FrozenPredictSpec = serde_json::from_value(spec)?;
        let reject = |reason: String| dag_core::registry::error::Error::SpecRejection {
            kind: "ml_frozen_predict".into(),
            reason,
            schema_pretty: serde_json::to_string_pretty(&schema_for!(FrozenPredictSpec))
                .unwrap_or_default(),
        };
        if let Err(reason) = validate_spec(s.min_top_prob, s.uncertain_margin) {
            return Err(reject(reason));
        }
        Ok(Box::new(FrozenPredictNode {
            min_top_prob: s.min_top_prob,
            uncertain_margin: s.uncertain_margin,
            meta: self.ports(),
        }))
    }
}

/// Spec validation shared by `build` and the unit tests.
fn validate_spec(min_top_prob: Option<f64>, uncertain_margin: Option<f64>) -> Result<(), String> {
    for (name, v) in [
        ("min_top_prob", min_top_prob),
        ("uncertain_margin", uncertain_margin),
    ] {
        if let Some(v) = v {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(format!("{name} must be in [0, 1], got {v}"));
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
struct FrozenPredictNode {
    min_top_prob: Option<f64>,
    uncertain_margin: Option<f64>,
    meta: NodePorts,
}

/// Class-probability output shared by both model families.
pub(super) struct FrozenOut {
    pub labels: Vec<String>,
    /// Row-major probabilities, one row per sample.
    pub probs: Vec<Vec<f64>>,
}

/// Dispatch on the artifact kind and score `x`.  Pure: no DAG plumbing,
/// so the fit→artifact→predict chain is unit-testable end to end.
pub(super) fn frozen_predict(
    artifact: &ml::ModelArtifact,
    x: &Mat<f64>,
) -> Result<FrozenOut, String> {
    let out = match artifact.kind.as_str() {
        PAM_KIND => {
            let model: PamModel = artifact
                .deserialize_fitted()
                .map_err(|e| format!("deserialize {PAM_KIND}: {e}"))?;
            let out = pam_predict(&model, x).map_err(|e| e.to_string())?;
            FrozenOut {
                labels: model.class_labels.clone(),
                probs: out.probabilities,
            }
        }
        MULTINOMIAL_ENET_KIND => {
            let model: MultinomialEnetModel = artifact
                .deserialize_fitted()
                .map_err(|e| format!("deserialize {MULTINOMIAL_ENET_KIND}: {e}"))?;
            let out = mnet_predict(&model, x).map_err(|e| e.to_string())?;
            FrozenOut {
                labels: model.class_labels.clone(),
                probs: out.probabilities,
            }
        }
        other => {
            return Err(format!(
                "unsupported artifact kind \"{other}\" (expected {PAM_KIND} or {MULTINOMIAL_ENET_KIND})"
            ));
        }
    };
    if out.labels.len() < 2 {
        return Err(format!("model has {} classes, need >= 2", out.labels.len()));
    }
    Ok(out)
}

/// top_prob, margin and the uncertainty flag for one probability row.
fn uncertainty(probs: &[f64], min_top: Option<f64>, min_margin: Option<f64>) -> (f64, f64, bool) {
    let mut sorted = probs.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let top = sorted[0];
    let margin = sorted[0] - sorted[1];
    let flag = min_top.is_some_and(|t| top < t) || min_margin.is_some_and(|m| margin < m);
    (top, margin, flag)
}

#[async_trait]
impl DagNode for FrozenPredictNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_frozen_predict"
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
        let ne = |msg: String| DagError::NodeError {
            node_type: "ml_frozen_predict".into(),
            msg,
        };
        // port 0: the artifact row; port 1: the samples
        let artifact_batches = collect_port(inputs, 0).await?;
        let sample_batches = collect_port(inputs, 1).await?;
        let artifact = artifact_from_batch(&artifact_batches).map_err(ne)?;
        if sample_batches.is_empty() || sample_batches[0].num_rows() == 0 {
            return Err(ne("no sample rows on port 1".into()));
        }
        let x = common::extract_matrix(&sample_batches, &artifact.feature_names)
            .map_err(|e| ne(e.to_string()))?;
        let out = frozen_predict(&artifact, &x).map_err(ne)?;
        let k = out.labels.len();

        // ── assemble: input columns + p_<class>×K + decision columns ─────
        let mut tops = Vec::with_capacity(x.nrows());
        let mut margins = Vec::with_capacity(x.nrows());
        let mut flags = Vec::with_capacity(x.nrows());
        let mut preds = Vec::with_capacity(x.nrows());
        for row in &out.probs {
            let (top, margin, flag) = uncertainty(row, self.min_top_prob, self.uncertain_margin);
            tops.push(top);
            margins.push(margin);
            flags.push(flag);
            let mut best = 0;
            for (c, &v) in row.iter().enumerate() {
                if v > row[best] {
                    best = c;
                }
            }
            preds.push(out.labels[best].clone());
        }

        let (_schema, mut fields, mut arrays) =
            common::concat_input(&sample_batches).map_err(|e| ne(e.to_string()))?;
        for c in 0..k {
            fields.push(Arc::new(Field::new(
                format!("p_{}", out.labels[c]),
                DataType::Float64,
                false,
            )));
            arrays.push(Arc::new(Float64Array::from(
                out.probs.iter().map(|r| r[c]).collect::<Vec<_>>(),
            )));
        }
        fields.push(Arc::new(Field::new("prediction", DataType::Utf8, false)));
        arrays.push(Arc::new(StringArray::from(preds)));
        fields.push(Arc::new(Field::new("top_prob", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(tops)));
        fields.push(Arc::new(Field::new("margin", DataType::Float64, false)));
        arrays.push(Arc::new(Float64Array::from(margins)));
        fields.push(Arc::new(Field::new(
            "is_uncertain",
            DataType::Boolean,
            false,
        )));
        arrays.push(Arc::new(BooleanArray::from(flags)));

        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| ne(format!("build output batch: {e}")))?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| ne(format!("read_batch: {e}")))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

async fn collect_port(inputs: &[NodeInput], port: u8) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs
        .iter()
        .find(|i| i.port == port)
        .ok_or_else(|| DagError::NodeError {
            node_type: "ml_frozen_predict".into(),
            msg: format!("input port {port} not connected"),
        })?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_frozen_predict".into(),
            msg: format!("collect port {port}: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::UInt32Array;
    use dag_core::registry::NodeCtx;

    use crate::supervised_nodes::MnetFitFactory;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    /// 3-class synthetic table: f0/f1 informative, f2 noise; labels A/B/C.
    fn toy_batches() -> Vec<RecordBatch> {
        let n_per = 10usize;
        let mut f0 = Vec::new();
        let mut f1 = Vec::new();
        let mut f2 = Vec::new();
        let mut label = Vec::new();
        for c in 0..3 {
            for r in 0..n_per {
                let base = (c as f64 - 1.0) * 2.0 + (r as f64 - 4.5) * 0.1;
                f0.push(base);
                f1.push(-base * 0.8);
                f2.push(((r * 7 + c * 3) % 11) as f64 / 11.0 - 0.5);
                label.push(((b'A' + c as u8) as char).to_string());
            }
        }
        let schema = Arc::new(Schema::new(vec![
            Field::new("f0", DataType::Float64, false),
            Field::new("f1", DataType::Float64, false),
            Field::new("f2", DataType::Float64, false),
            Field::new("label", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(f0)),
                Arc::new(Float64Array::from(f1)),
                Arc::new(Float64Array::from(f2)),
                Arc::new(StringArray::from(label)),
            ],
        )
        .unwrap();
        vec![batch]
    }

    fn batch_to_df(batch: RecordBatch) -> datafusion::prelude::DataFrame {
        datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap()
    }

    #[test]
    fn test_spec_defaults_and_validation() {
        let spec: FrozenPredictSpec = serde_json::from_str("{}").unwrap();
        assert!(spec.min_top_prob.is_none() && spec.uncertain_margin.is_none());
        assert!(validate_spec(None, None).is_ok());
        assert!(validate_spec(Some(0.6), Some(0.1)).is_ok());
        assert!(validate_spec(Some(0.0), Some(1.0)).is_ok());
        assert!(validate_spec(Some(1.5), None).is_err());
        assert!(validate_spec(None, Some(-0.1)).is_err());
        assert!(validate_spec(Some(f64::NAN), None).is_err());
    }

    #[test]
    fn test_unknown_kind_rejected() {
        let artifact = ml::ModelArtifact::new(
            "kmeans:v1",
            &vec![1.0f64, 2.0],
            vec!["a".to_string()],
            serde_json::json!({}),
        )
        .unwrap();
        assert!(frozen_predict(&artifact, &Mat::from_fn(2, 1, |_, _| 0.0)).is_err());
    }

    #[test]
    fn test_uncertainty_flags() {
        // binary fractions: top/margin arithmetic is exact
        let row = [0.75, 0.25, 0.0];
        // no thresholds → never uncertain
        assert_eq!(uncertainty(&row, None, None), (0.75, 0.5, false));
        // top 0.75 >= 0.6, margin 0.5 >= 0.1 → certain
        assert_eq!(uncertainty(&row, Some(0.6), Some(0.1)), (0.75, 0.5, false));
        // top below threshold → uncertain
        assert!(uncertainty(&row, Some(0.9), None).2);
        // margin below threshold → uncertain
        assert!(uncertainty(&row, None, Some(0.6)).2);
    }

    #[test]
    fn test_pam_artifact_dispatch_roundtrip() {
        // fit PAM directly, freeze to an artifact, predict through the node core
        let batches = toy_batches();
        let feats = ["f0", "f1", "f2"].map(|s| s.to_string()).to_vec();
        let x = common::extract_matrix(&batches, &feats).unwrap();
        let y: Vec<usize> = (0..3).flat_map(|c| vec![c; 10]).collect();
        let labels: Vec<String> = ["A", "B", "C"].iter().map(|s| s.to_string()).collect();
        let folds = ml::split::stratified_kfold(&y, 3, true, 1).unwrap();
        let (model, _, _) = ml::centroid::pam_fit(
            &x,
            &y,
            &labels,
            &feats,
            None,
            Some(&folds),
            ml::centroid::DeltaSelect::MinError,
        )
        .unwrap();
        let direct = pam_predict(&model, &x).unwrap();
        let artifact =
            ml::ModelArtifact::new(PAM_KIND, &model, feats, serde_json::json!({"n": 30})).unwrap();
        let out = frozen_predict(&artifact, &x).unwrap();
        assert_eq!(out.labels, labels);
        assert_eq!(out.probs.len(), x.nrows());
        for i in 0..x.nrows() {
            for c in 0..3 {
                assert!((out.probs[i][c] - direct.probabilities[i][c]).abs() < 1e-12);
            }
        }
    }

    // ── end-to-end: mnet fit node → p0 artifact → frozen_predict node ─────
    //
    // Exercises the full DAG plumbing (spec build, execute, artifact row in,
    // decision columns out) and the OOF / full-fit separation: the OOF
    // predictions (per-fold refits, port 3) must not simply echo the frozen
    // full-data model's in-sample predictions.
    #[tokio::test]
    async fn test_e2e_mnet_fit_to_frozen_predict() {
        let ctx = node_ctx();
        let table = toy_batches();
        // 3 folds cycling rows: every fold holds out a third of each class
        let fold: Vec<u32> = (0..30).map(|i| (i % 3) as u32).collect();
        let with_fold = {
            let b = &table[0];
            let mut fields: Vec<Arc<Field>> = b.schema().fields().iter().cloned().collect();
            let mut arrays: Vec<Arc<dyn arrow_array::Array>> = b.columns().to_vec();
            fields.push(Arc::new(Field::new("fold", DataType::UInt32, false)));
            arrays.push(Arc::new(UInt32Array::from(fold)));
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
        };

        // ── stage 1: ml_multinomial_enet_fit(fold_column) ──────────────────
        let fit_spec = serde_json::json!({
            "features": ["f0", "f1", "f2"],
            "label_column": "label",
            "fold_column": "fold",
            "cv_k": 3,
            "n_lambda": 8,
            "max_iter": 3000,
            "tol": 1e-7,
            "seed": 42,
        });
        let mut fit_node = MnetFitFactory
            .build(fit_spec, node_ctx())
            .map_err(|e| e.to_string())
            .unwrap();
        let fit_out = fit_node
            .execute(
                &ctx,
                &[NodeInput::new_dataframe(0, batch_to_df(with_fold.clone()))],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let artifact_rows = fit_out
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(artifact_rows.len(), 1);
        assert_eq!(
            artifact_rows[0]
                .column(artifact_rows[0].schema().index_of("kind").unwrap())
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0),
            MULTINOMIAL_ENET_KIND
        );
        let oof_rows = fit_out
            .dataframe(3)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(oof_rows[0].num_rows(), 30);
        let oof_pred_col = oof_rows[0]
            .column(oof_rows[0].schema().index_of("prediction").unwrap())
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let label_col = oof_rows[0]
            .column(oof_rows[0].schema().index_of("label").unwrap())
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let oof_correct = (0..30)
            .filter(|&i| oof_pred_col.value(i) == label_col.value(i))
            .count();

        // ── stage 2: ml_frozen_predict on the same rows ────────────────────
        let mut frozen_node = FrozenPredictFactory
            .build(
                serde_json::json!({"min_top_prob": 0.9, "uncertain_margin": 0.8}),
                node_ctx(),
            )
            .map_err(|e| e.to_string())
            .unwrap();
        let frozen_out = frozen_node
            .execute(
                &ctx,
                &[
                    NodeInput::new_dataframe(0, batch_to_df(artifact_rows[0].clone())),
                    NodeInput::new_dataframe(1, batch_to_df(table[0].clone())),
                ],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let scored = frozen_out
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        let batch = &scored[0];
        assert_eq!(batch.num_rows(), 30);
        for name in [
            "f0",
            "f1",
            "f2",
            "label",
            "p_A",
            "p_B",
            "p_C",
            "prediction",
            "top_prob",
            "margin",
            "is_uncertain",
        ] {
            assert!(
                batch.schema().index_of(name).is_ok(),
                "missing column {name}"
            );
        }
        let frozen_pred = batch
            .column(batch.schema().index_of("prediction").unwrap())
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let top = batch
            .column(batch.schema().index_of("top_prob").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let margin = batch
            .column(batch.schema().index_of("margin").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let uncertain = batch
            .column(batch.schema().index_of("is_uncertain").unwrap())
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        for i in 0..30 {
            let p: Vec<f64> = ["p_A", "p_B", "p_C"]
                .iter()
                .map(|n| {
                    batch
                        .column(batch.schema().index_of(n).unwrap())
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .unwrap()
                        .value(i)
                })
                .collect();
            let sum: f64 = p.iter().sum();
            assert!((sum - 1.0).abs() < 1e-10, "row {i} probs sum {sum}");
            assert!((top.value(i) - p.iter().cloned().fold(f64::MIN, f64::max)).abs() < 1e-12);
            let expect_flag = top.value(i) < 0.9 || margin.value(i) < 0.8;
            assert_eq!(uncertain.value(i), expect_flag, "row {i}");
        }

        // ── separation: OOF comes from per-fold refits, not the frozen ─────
        // full-data model.  On well-separated data the *classes* agree, so
        // the separation shows at the probability level: models trained on
        // 20 rows cannot reproduce a 30-row fit's probabilities.
        let row_index = oof_rows[0]
            .column(oof_rows[0].schema().index_of("row_index").unwrap())
            .as_any()
            .downcast_ref::<UInt32Array>()
            .unwrap();
        let oof_prob = |i: usize, c: usize| {
            oof_rows[0]
                .column(
                    oof_rows[0]
                        .schema()
                        .index_of(["p_A", "p_B", "p_C"][c])
                        .unwrap(),
                )
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(i)
        };
        let frozen_prob = |i: usize, c: usize| {
            batch
                .column(batch.schema().index_of(["p_A", "p_B", "p_C"][c]).unwrap())
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(i)
        };
        let mut max_diff = 0.0f64;
        for i in 0..30 {
            for c in 0..3 {
                let d = (oof_prob(i, c) - frozen_prob(row_index.value(i) as usize, c)).abs();
                max_diff = max_diff.max(d);
            }
        }
        assert!(
            max_diff > 1e-6,
            "OOF probabilities identical to frozen full-fit (max diff {max_diff})"
        );
        let frozen_correct = (0..30)
            .filter(|&i| frozen_pred.value(row_index.value(i) as usize) == label_col.value(i))
            .count();
        // in-sample fit must not be worse than the held-out estimate
        assert!(
            frozen_correct >= oof_correct,
            "full-fit {frozen_correct} < OOF {oof_correct}"
        );
    }
}
