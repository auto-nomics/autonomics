use super::*;

use faer::Mat;

use ml::centroid::{DeltaSelect, PAM_KIND, pam_fit, pam_predict};
use ml::split::Fold;

use crate::model_nodes::artifact_to_batch;

// ═══════════════════════════════════════════════════════════════════════
// PamFit — nearest shrunken centroid (PAM), multiclass
// ═══════════════════════════════════════════════════════════════════════
//
// Fits `ml::centroid::pam_fit` (pamr-verified) on a feature table and
// emits four ports:
//   p0 artifact — single-row artifact table (`pam:v1`) for
//      ml_model_save / ml_frozen_predict;
//   p1 cv_curve — Δ grid with CV error, binomial SE and nonzero-feature
//      count (cv_error/cv_se are NaN when cv_k < 2);
//   p2 signature_panel — every nonzero shrunken (class, feature) pair;
//   p3 oof — out-of-fold predictions, one row per input row, produced when
//      `fold_column` is given (each fold is predicted by a model trained
//      and Δ-selected on the remaining folds only — no leakage); an empty
//      table otherwise.
//
// Labels: any string-or-numeric column; class ids follow the sorted
// unique label values (same lexicographic ranking as ml_group_kfold).

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PamFitSpec {
    /// Feature columns, in the order they enter the model.
    pub features: Vec<String>,
    /// Label column (string or numeric); classes = sorted unique values.
    pub label_column: String,
    /// Δ selection rule from the CV curve: `"min_error"` (default) or
    /// `"one_se"` (largest Δ within one SE of the minimum).
    #[serde(default = "d_delta_select")]
    pub delta_select: String,
    /// CV folds for Δ selection (also the inner CV when fold_column is
    /// given).  Set below 2 to skip CV entirely (model fits at Δ = 0).
    #[serde(default = "d_cv_k")]
    pub cv_k: usize,
    /// Optional group column (e.g. patient_id): CV folds keep whole groups
    /// together, both for the full-data Δ selection and per-fold inner CV.
    #[serde(default)]
    pub group_column: Option<String>,
    /// Optional fold column (0-based ids): emit out-of-fold predictions on
    /// port 3, one fold held out at a time.
    #[serde(default)]
    pub fold_column: Option<String>,
    /// Seed for fold shuffling (ChaCha8).
    #[serde(default)]
    pub seed: u64,
}
fn d_delta_select() -> String {
    "min_error".into()
}
fn d_cv_k() -> usize {
    5
}

pub struct PamFitFactory;
impl NodeFactory for PamFitFactory {
    fn kind(&self) -> &'static str {
        "ml_pam_fit"
    }
    fn desc(&self) -> &'static str {
        "Nearest shrunken centroid (PAM) multiclass classifier with Δ CV."
    }
    fn doc(&self) -> &'static str {
        "PamFit: fits a pamr-equivalent nearest shrunken centroid model. \
        Labels map to sorted unique values of label_column. Outputs: p0 \
        model artifact (kind pam:v1) for save/frozen-predict, p1 Δ CV curve, \
        p2 signature panel (nonzero shrunken centroids), p3 out-of-fold \
        predictions when fold_column is given (per-fold refit + inner Δ \
        CV on the remaining rows — leakage-free)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PamFitSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None) // p0 artifact
            .add_output_port(None) // p1 cv_curve
            .add_output_port(None) // p2 signature_panel
            .add_output_port(None) // p3 oof
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PamFitSpec = serde_json::from_value(spec)?;
        let reject = |reason: String| dag_core::registry::error::Error::SpecRejection {
            kind: "ml_pam_fit".into(),
            reason,
            schema_pretty: serde_json::to_string_pretty(&schema_for!(PamFitSpec))
                .unwrap_or_default(),
        };
        if let Err(reason) = validate_spec(&s.features, &s.delta_select) {
            return Err(reject(reason));
        }
        Ok(Box::new(PamFitNode {
            features: s.features,
            label_column: s.label_column,
            delta_select: s.delta_select,
            cv_k: s.cv_k,
            group_column: s.group_column,
            fold_column: s.fold_column,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PamFitNode {
    features: Vec<String>,
    label_column: String,
    delta_select: String,
    cv_k: usize,
    group_column: Option<String>,
    fold_column: Option<String>,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for PamFitNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_pam_fit"
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
            node_type: "ml_pam_fit".into(),
            msg,
        };
        let select = match self.delta_select.as_str() {
            "one_se" => DeltaSelect::OneSe,
            _ => DeltaSelect::MinError,
        };

        let batches = collect_batches(inputs).await?;
        let x = common::extract_matrix(&batches, &self.features).map_err(|e| ne(e.to_string()))?;
        let n = x.nrows();
        if n == 0 {
            return Err(ne("no input rows".into()));
        }

        // labels: sorted-unique dense ids (lexicographic, like ml_group_kfold)
        let label_keys = common::group_keys(&batches, &self.label_column).map_err(ne)?;
        let class_labels: Vec<String> = label_keys
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .cloned()
            .collect();
        let y = common::dense_group_ids(&label_keys);

        // groups (shared by the full-data CV and the per-fold inner CV)
        let groups: Option<Vec<usize>> = match &self.group_column {
            Some(col) => {
                let keys = common::group_keys(&batches, col).map_err(ne)?;
                Some(common::dense_group_ids(&keys))
            }
            None => None,
        };

        // CV folds for Δ selection
        let folds: Option<Vec<Fold>> = if self.cv_k >= 2 {
            Some(make_folds(&y, groups.as_deref(), self.cv_k, self.seed).map_err(ne)?)
        } else {
            None
        };

        let (model, curve, signature) = pam_fit(
            &x,
            &y,
            &class_labels,
            &self.features,
            None,
            folds.as_deref(),
            select,
        )
        .map_err(|e| ne(e.to_string()))?;

        // ── p0 artifact ──────────────────────────────────────────────────
        let artifact = ml::ModelArtifact::new(
            PAM_KIND,
            &model,
            self.features.clone(),
            serde_json::json!({
                "delta_selected": model.delta_selected,
                "delta_min": curve.delta_min,
                "delta_1se": curve.delta_1se,
                "class_labels": class_labels,
                "n_samples": n,
            }),
        )
        .map_err(|e| ne(format!("serialize artifact: {e}")))?;
        let raw = artifact
            .to_bytes()
            .map_err(|e| ne(format!("serialize artifact: {e}")))?;
        let p0 = artifact_to_batch(&artifact, &raw).map_err(ne)?;

        // ── p1 cv_curve ──────────────────────────────────────────────────
        let no_cv = curve.cv_error.is_empty();
        let cv_error = if no_cv {
            vec![f64::NAN; curve.delta.len()]
        } else {
            curve.cv_error.clone()
        };
        let cv_se = if no_cv {
            vec![f64::NAN; curve.delta.len()]
        } else {
            curve.cv_se.clone()
        };
        let p1 = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Arc::new(Field::new("delta", DataType::Float64, false)),
                Arc::new(Field::new("cv_error", DataType::Float64, false)),
                Arc::new(Field::new("cv_se", DataType::Float64, false)),
                Arc::new(Field::new("nonzero_features", DataType::UInt32, false)),
            ])),
            vec![
                Arc::new(Float64Array::from(curve.delta)),
                Arc::new(Float64Array::from(cv_error)),
                Arc::new(Float64Array::from(cv_se)),
                Arc::new(UInt32Array::from(
                    curve
                        .nonzero_features
                        .into_iter()
                        .map(|v| v as u32)
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .map_err(|e| ne(format!("build cv_curve batch: {e}")))?;

        // ── p2 signature_panel ───────────────────────────────────────────
        let p2 = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Arc::new(Field::new("class_index", DataType::UInt32, false)),
                Arc::new(Field::new("class_label", DataType::Utf8, false)),
                Arc::new(Field::new("feature_index", DataType::UInt32, false)),
                Arc::new(Field::new("feature_name", DataType::Utf8, false)),
                Arc::new(Field::new("d_shrunk", DataType::Float64, false)),
            ])),
            vec![
                Arc::new(UInt32Array::from(
                    signature
                        .iter()
                        .map(|e| e.class_index as u32)
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    signature
                        .iter()
                        .map(|e| e.class_label.clone())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(UInt32Array::from(
                    signature
                        .iter()
                        .map(|e| e.feature_index as u32)
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    signature
                        .iter()
                        .map(|e| e.feature_name.clone())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    signature.iter().map(|e| e.d_shrunk).collect::<Vec<_>>(),
                )),
            ],
        )
        .map_err(|e| ne(format!("build signature batch: {e}")))?;

        // ── p3 oof ───────────────────────────────────────────────────────
        let oof_rows = match &self.fold_column {
            Some(col) => {
                let keys = common::group_keys(&batches, col).map_err(ne)?;
                let fold_ids = common::dense_group_ids(&keys);
                Some(
                    oof_predictions(
                        &x,
                        &y,
                        &class_labels,
                        &self.features,
                        &fold_ids,
                        groups.as_deref(),
                        select,
                        self.cv_k,
                        self.seed,
                    )
                    .map_err(ne)?,
                )
            }
            None => None,
        };
        let mut oof_fields: Vec<Arc<Field>> = vec![
            Arc::new(Field::new("row_index", DataType::UInt32, false)),
            Arc::new(Field::new(
                self.label_column.as_str(),
                DataType::Utf8,
                false,
            )),
        ];
        for label in &class_labels {
            oof_fields.push(Arc::new(Field::new(
                format!("p_{label}"),
                DataType::Float64,
                false,
            )));
        }
        oof_fields.push(Arc::new(Field::new("prediction", DataType::Utf8, false)));
        let oof_arrays: Vec<Arc<dyn Array>> = match &oof_rows {
            Some(rows) => {
                let mut arrays: Vec<Arc<dyn Array>> = vec![
                    Arc::new(UInt32Array::from(
                        rows.iter().map(|r| r.row_index as u32).collect::<Vec<_>>(),
                    )),
                    Arc::new(StringArray::from(
                        rows.iter()
                            .map(|r| label_keys[r.row_index].clone())
                            .collect::<Vec<_>>(),
                    )),
                ];
                for c in 0..class_labels.len() {
                    arrays.push(Arc::new(Float64Array::from(
                        rows.iter().map(|r| r.probs[c]).collect::<Vec<_>>(),
                    )));
                }
                arrays.push(Arc::new(StringArray::from(
                    rows.iter()
                        .map(|r| class_labels[r.pred].clone())
                        .collect::<Vec<_>>(),
                )));
                arrays
            }
            None => Vec::new(),
        };
        let p3 = RecordBatch::try_new(Arc::new(Schema::new(oof_fields)), oof_arrays)
            .map_err(|e| ne(format!("build oof batch: {e}")))?;

        let sess = ctx.session();
        let mut res = PortOutputs::new();
        for (port, batch) in [(0u8, p0), (1, p1), (2, p2), (3, p3)] {
            res.insert(
                port,
                sess.read_batch(batch)
                    .map_err(|e| ne(format!("read_batch({port}): {e}")))?,
            );
        }
        Ok(res)
    }
}

// ── pure helpers ─────────────────────────────────────────────────────────

/// Spec validation shared by `build` and the unit tests.
fn validate_spec(features: &[String], delta_select: &str) -> Result<(), String> {
    if features.is_empty() {
        return Err("features must contain at least one column name".into());
    }
    if !matches!(delta_select, "min_error" | "one_se") {
        return Err(format!(
            "delta_select must be \"min_error\" or \"one_se\", got \"{delta_select}\""
        ));
    }
    Ok(())
}

/// Build CV folds for Δ selection: group-aware when `groups` is given,
/// stratified by label otherwise.
fn make_folds(
    y: &[usize],
    groups: Option<&[usize]>,
    cv_k: usize,
    seed: u64,
) -> Result<Vec<Fold>, String> {
    match groups {
        Some(groups) => ml::split::group_kfold(groups, cv_k).map_err(|e| e.to_string()),
        None => ml::split::stratified_kfold(y, cv_k, true, seed).map_err(|e| e.to_string()),
    }
}

/// One out-of-fold prediction row, keyed by its original table position.
struct OofRow {
    row_index: usize,
    pred: usize,
    probs: Vec<f64>,
}

/// Leakage-free out-of-fold predictions: for every fold id, refit PAM on
/// the remaining rows (Δ re-selected via inner CV on those rows only) and
/// predict the held-out fold.  Returns rows sorted by original position.
#[allow(clippy::too_many_arguments)]
fn oof_predictions(
    x: &Mat<f64>,
    y: &[usize],
    class_labels: &[String],
    feature_names: &[String],
    fold_ids: &[usize],
    groups: Option<&[usize]>,
    select: DeltaSelect,
    cv_k: usize,
    seed: u64,
) -> Result<Vec<OofRow>, String> {
    let n = y.len();
    let p = x.ncols();
    let fold_values: Vec<usize> = {
        let mut v: Vec<usize> = fold_ids.to_vec();
        v.sort_unstable();
        v.dedup();
        v
    };
    let mut rows: Vec<OofRow> = Vec::with_capacity(n);
    for (f_idx, &f) in fold_values.iter().enumerate() {
        let test: Vec<usize> = (0..n).filter(|&i| fold_ids[i] == f).collect();
        let train: Vec<usize> = (0..n).filter(|&i| fold_ids[i] != f).collect();
        let x_train = Mat::from_fn(train.len(), p, |r, j| x[(train[r], j)]);
        let y_train: Vec<usize> = train.iter().map(|&i| y[i]).collect();
        let inner = if cv_k >= 2 {
            let groups_train: Option<Vec<usize>> =
                groups.map(|g| train.iter().map(|&i| g[i]).collect());
            Some(make_folds(
                &y_train,
                groups_train.as_deref(),
                cv_k,
                seed.wrapping_add(f_idx as u64 + 1),
            )?)
        } else {
            None
        };
        let (model, _, _) = pam_fit(
            &x_train,
            &y_train,
            class_labels,
            feature_names,
            None,
            inner.as_deref(),
            select,
        )
        .map_err(|e| format!("fold {f}: {e}"))?;
        let x_test = Mat::from_fn(test.len(), p, |r, j| x[(test[r], j)]);
        let out = pam_predict(&model, &x_test).map_err(|e| format!("fold {f}: {e}"))?;
        for (r, &i) in test.iter().enumerate() {
            rows.push(OofRow {
                row_index: i,
                pred: out.predictions[r],
                probs: out.probabilities[r].clone(),
            });
        }
    }
    rows.sort_unstable_by_key(|r| r.row_index);
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_defaults() {
        let spec: PamFitSpec =
            serde_json::from_str(r#"{"features":["a","b"],"label_column":"y"}"#).unwrap();
        assert_eq!(spec.features, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(spec.delta_select, "min_error");
        assert_eq!(spec.cv_k, 5);
        assert!(spec.group_column.is_none());
        assert!(spec.fold_column.is_none());
        assert_eq!(spec.seed, 0);
    }

    #[test]
    fn test_validate_spec_rejects_bad_delta_select() {
        assert!(validate_spec(&["a".into()], "nope").is_err());
        assert!(validate_spec(&[], "min_error").is_err());
        assert!(validate_spec(&["a".into()], "min_error").is_ok());
        assert!(validate_spec(&["a".into()], "one_se").is_ok());
    }

    #[test]
    fn test_oof_covers_all_rows_and_separates() {
        // 3 classes × 4 samples, two informative features
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for c in 0..3usize {
            for r in 0..4 {
                let base = (c as f64 - 1.0) * 3.0 + (r as f64 - 1.5) * 0.2;
                rows.push(vec![base, -base * 0.7]);
                y.push(c);
            }
        }
        let x = Mat::from_fn(12, 2, |i, j| rows[i][j]);
        let labels: Vec<String> = (0..3).map(|c| format!("class_{c}")).collect();
        let feats = vec!["f0".to_string(), "f1".to_string()];
        // rows interleaved across folds: fold of row i = i % 3
        let fold_ids: Vec<usize> = (0..12).map(|i| i % 3).collect();

        let oof = oof_predictions(
            &x,
            &y,
            &labels,
            &feats,
            &fold_ids,
            None,
            DeltaSelect::MinError,
            3,
            42,
        )
        .unwrap();
        assert_eq!(oof.len(), 12);
        // every original row exactly once, ascending
        let idx: Vec<usize> = oof.iter().map(|r| r.row_index).collect();
        assert_eq!(idx, (0..12).collect::<Vec<_>>());
        // probabilities are proper distributions
        for r in &oof {
            assert!((r.probs.iter().sum::<f64>() - 1.0).abs() < 1e-10);
        }
        // well-separated data: every held-out row classified correctly
        assert!(oof.iter().enumerate().all(|(i, r)| r.pred == y[i]));
    }
}
