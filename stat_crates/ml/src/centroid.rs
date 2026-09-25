//! Nearest shrunken centroid classifier (PAM).
//!
//! Tibshirani, Hastie, Narasimhan, Chu (2002) PNAS 99:6567, implemented to
//! match pamr's `nsc` numerically (posteriors verified to 1e-15):
//!
//! - pooled within-class SD `s_j` (divisor n - k) plus a median offset
//!   (`s_j + median(s)`, pamr's `offset.percent = 50` default);
//! - `m_k = sqrt(1/n_k - 1/n)` (pamr's `se.scale`; the PNAS paper prints
//!   `1/n_k + 1/n` but pamr implements the minus form);
//! - standardized centroid difference `d_kj = (x̄_kj - x̄_j) / (s_j · m_k)`,
//!   soft-shrunk `d'_kj = sign(d)·max(|d| - Δ, 0)`;
//! - z-space shrunk centroid `c_kj = d'_kj · m_k`;
//! - Gaussian-NB discriminant `disc_k(i) = Σ_j z_ij·c_kj - ½ Σ_j c_kj²
//!   + log π_k` with `z_ij = (x_ij - x̄_j)/s_j`; class = argmax (first max
//!   wins), posterior ∝ exp(clamp(disc, ±500)).
//!
//! Default Δ grid: `seq(0, max|d|, length = 30)` — the full grid is kept,
//! as pamr does (its `$nonzero` counts are diagnostic only, trailing
//! all-zero Δs are not removed).  Δ selection from K-fold CV error:
//! `MinError` (ties → largest Δ) or `OneSe` (largest Δ within one
//! binomial SE of the minimum).

use faer::Mat;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Artifact kind tag for PAM models.
pub const PAM_KIND: &str = "pam:v1";

#[derive(Debug, Error)]
pub enum CentroidError {
    #[error("empty input")]
    Empty,
    #[error("labels length ({labels}) doesn't match data rows ({rows})")]
    LabelMismatch { labels: usize, rows: usize },
    #[error("need >= 2 classes, got {0}")]
    TooFewClasses(usize),
    #[error("label {label} is not < k = {k} (labels must be contiguous 0..k)")]
    BadLabel { label: usize, k: usize },
    #[error("class {0} has no training samples in a CV fold")]
    MissingClass(usize),
    #[error("all features have zero within-class variance")]
    Degenerate,
    #[error("delta grid must be non-empty, non-negative and non-decreasing")]
    BadGrid,
    #[error("feature matrix contains non-finite values")]
    NonFinite,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CentroidError>;

/// How `delta_selected` is chosen from the CV curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaSelect {
    /// Δ of the smallest CV misclassification error (ties → largest Δ,
    /// i.e. the simplest signature).
    MinError,
    /// Largest Δ with CV error <= min error + binomial SE of the minimum.
    OneSe,
}

/// Fitted nearest-shrunken-centroid model (`kind = "pam:v1"`).
///
/// Everything needed for prediction travels with the model; preprocessing
/// (grand mean / pooled SD) is locked to training, so the apply side needs
/// no other state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PamModel {
    pub class_labels: Vec<String>,
    /// Grand mean per feature, `x̄_j`.
    pub grand_mean: Vec<f64>,
    /// Pooled within-class SD per feature, `s_j` (median offset included).
    pub pooled_sd: Vec<f64>,
    /// Per-class standardisation, `m_k = sqrt(1/n_k - 1/n)`.
    pub se_scale: Vec<f64>,
    /// Class priors, `π_k = n_k / n`.
    pub priors: Vec<f64>,
    /// z-space shrunken centroids `c_kj = d'_kj · m_k` (k rows × p cols)
    /// at `delta_selected`.
    pub shrunk_z_centroids: Vec<Vec<f64>>,
    /// The shrinkage threshold the centroids were shrunken at.
    pub delta_selected: f64,
}

/// Predictions with posterior probabilities.
#[derive(Debug, Clone)]
pub struct ClassProbs {
    pub predictions: Vec<usize>,
    /// n rows × k classes, rows sum to 1.
    pub probabilities: Vec<Vec<f64>>,
}

/// CV error curve over the Δ grid.
#[derive(Debug, Clone)]
pub struct PamCvCurve {
    /// The evaluated grid (non-decreasing).
    pub delta: Vec<f64>,
    /// Pooled out-of-fold misclassification rate per Δ (empty without CV).
    pub cv_error: Vec<f64>,
    /// Binomial SE `sqrt(p(1-p)/n)` per Δ (empty without CV).
    pub cv_se: Vec<f64>,
    /// Features with any nonzero `d'_kj` per Δ, full-data fit.
    pub nonzero_features: Vec<usize>,
    /// Δ of the minimum CV error (0 without CV).
    pub delta_min: f64,
    /// Largest Δ within one SE of the minimum (0 without CV).
    pub delta_1se: f64,
}

/// One signature-panel row: a nonzero shrunken coefficient.
#[derive(Debug, Clone)]
pub struct SignatureEntry {
    pub class_index: usize,
    pub class_label: String,
    pub feature_index: usize,
    pub feature_name: String,
    /// The shrunken standardized difference `d'_kj`.
    pub d_shrunk: f64,
}

// ── internal training statistics ─────────────────────────────────────────

/// Δ-independent training statistics; every grid point reuses these.
struct PamStats {
    k: usize,
    p: usize,
    grand_mean: Vec<f64>,
    pooled_sd: Vec<f64>,
    se_scale: Vec<f64>,
    priors: Vec<f64>,
    /// Standardized centroid differences `d_kj` (k × p).
    d: Vec<Vec<f64>>,
}

impl PamStats {
    fn fit(x: &Mat<f64>, y: &[usize], k: usize) -> Result<Self> {
        let n = x.nrows();
        let p = x.ncols();
        let mut nk = vec![0usize; k];
        for &c in y {
            nk[c] += 1;
        }
        if let Some((c, _)) = nk.iter().enumerate().find(|&(_, &count)| count == 0) {
            return Err(CentroidError::MissingClass(c));
        }

        let mut grand_mean = vec![0.0; p];
        for i in 0..n {
            for j in 0..p {
                grand_mean[j] += x[(i, j)];
            }
        }
        for v in &mut grand_mean {
            *v /= n as f64;
        }

        // class means, pooled within-class SD (divisor n - k)
        let mut class_mean = vec![vec![0.0; p]; k];
        for i in 0..n {
            for j in 0..p {
                class_mean[y[i]][j] += x[(i, j)];
            }
        }
        for c in 0..k {
            for v in &mut class_mean[c] {
                *v /= nk[c] as f64;
            }
        }
        let mut ss = vec![0.0; p];
        for i in 0..n {
            for j in 0..p {
                let r = x[(i, j)] - class_mean[y[i]][j];
                ss[j] += r * r;
            }
        }
        let denom = (n - k) as f64;
        let pooled0: Vec<f64> = ss.iter().map(|&s| (s / denom).sqrt()).collect();
        if pooled0.iter().all(|&s| s == 0.0) {
            return Err(CentroidError::Degenerate);
        }
        // pamr offset: s_j + median(s); type-7 quantile at 0.5 is the
        // textbook median of the sorted pooled SDs.
        let mut sorted = pooled0.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite pooled SDs"));
        let offset = median(&sorted);
        let pooled_sd: Vec<f64> = pooled0.iter().map(|&s| s + offset).collect();

        let se_scale: Vec<f64> = nk
            .iter()
            .map(|&c| (1.0 / c as f64 - 1.0 / n as f64).sqrt())
            .collect();
        let priors: Vec<f64> = nk.iter().map(|&c| c as f64 / n as f64).collect();

        let d: Vec<Vec<f64>> = (0..k)
            .map(|c| {
                (0..p)
                    .map(|j| (class_mean[c][j] - grand_mean[j]) / (pooled_sd[j] * se_scale[c]))
                    .collect()
            })
            .collect();

        Ok(PamStats {
            k,
            p,
            grand_mean,
            pooled_sd,
            se_scale,
            priors,
            d,
        })
    }

    /// Shrunken z-space centroids `c_kj = d'_kj · m_k` at `delta`.
    fn shrunk_centroids(&self, delta: f64) -> Vec<Vec<f64>> {
        (0..self.k)
            .map(|c| {
                self.d[c]
                    .iter()
                    .map(|&d| d.signum() * (d.abs() - delta).max(0.0) * self.se_scale[c])
                    .collect()
            })
            .collect()
    }

    /// Features with any nonzero shrunken coefficient at `delta`.
    fn nonzero_features(&self, delta: f64) -> usize {
        let mut any = vec![false; self.p];
        for c in 0..self.k {
            for (j, &d) in self.d[c].iter().enumerate() {
                if d.abs() > delta {
                    any[j] = true;
                }
            }
        }
        any.iter().filter(|&&a| a).count()
    }
}

/// Type-7 midpoint median of an ascending-sorted slice.
fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// Index of the maximum value; first max wins ties (R `which.max`).
fn argmax(row: &[f64]) -> usize {
    let mut best = 0;
    for (c, &v) in row.iter().enumerate() {
        if v > row[best] {
            best = c;
        }
    }
    best
}

/// Discriminants `disc_k(i) = Σ_j z_ij·c_kj - ½ Σ_j c_kj² + log π_k` for
/// every row (n × k).
fn discriminants(
    grand_mean: &[f64],
    pooled_sd: &[f64],
    priors: &[f64],
    cent: &[Vec<f64>],
    x: &Mat<f64>,
) -> Result<Vec<Vec<f64>>> {
    let (k, p) = (cent.len(), grand_mean.len());
    if x.ncols() != p {
        return Err(CentroidError::Other(format!(
            "model has {p} features, data has {}",
            x.ncols()
        )));
    }
    // ½ Σ_j c_kj² − log π_k, once per class
    let offset: Vec<f64> = (0..k)
        .map(|c| 0.5 * cent[c].iter().map(|v| v * v).sum::<f64>() - priors[c].ln())
        .collect();
    Ok((0..x.nrows())
        .map(|i| {
            (0..k)
                .map(|c| {
                    let mut s = -offset[c];
                    for j in 0..p {
                        let z = (x[(i, j)] - grand_mean[j]) / pooled_sd[j];
                        s += z * cent[c][j];
                    }
                    s
                })
                .collect()
        })
        .collect())
}

/// Class = argmax discriminant; posterior = exp(clamp(disc, ±500))
/// normalised (pamr `safe.exp`), uniform on total underflow.
fn class_probs(disc: &[Vec<f64>]) -> ClassProbs {
    let mut predictions = Vec::with_capacity(disc.len());
    let mut probabilities = Vec::with_capacity(disc.len());
    for row in disc {
        predictions.push(argmax(row));
        let exps: Vec<f64> = row.iter().map(|&v| v.clamp(-500.0, 500.0).exp()).collect();
        let sum: f64 = exps.iter().sum();
        if sum > 0.0 && sum.is_finite() {
            probabilities.push(exps.iter().map(|e| e / sum).collect());
        } else {
            let u = 1.0 / row.len() as f64;
            probabilities.push(vec![u; row.len()]);
        }
    }
    ClassProbs {
        predictions,
        probabilities,
    }
}

// ── public API ───────────────────────────────────────────────────────────

/// Fit PAM, optionally cross-validating the Δ grid.
///
/// `folds` = `(train, test)` index pairs produced by the caller
/// (`ml::split::kfold` / `stratified_kfold` / `group_kfold`).  Without
/// folds the model is fit at the grid's first Δ (no shrinkage when left
/// default) and the curve's error columns stay empty.  A single-point
/// `delta_grid` pins the selection to that point regardless of rule.
pub fn pam_fit(
    x: &Mat<f64>,
    y: &[usize],
    class_labels: &[String],
    feature_names: &[String],
    delta_grid: Option<Vec<f64>>,
    folds: Option<&[(Vec<usize>, Vec<usize>)]>,
    select: DeltaSelect,
) -> Result<(PamModel, PamCvCurve, Vec<SignatureEntry>)> {
    let n = x.nrows();
    let p = x.ncols();
    if n == 0 || p == 0 {
        return Err(CentroidError::Empty);
    }
    if y.len() != n {
        return Err(CentroidError::LabelMismatch {
            labels: y.len(),
            rows: n,
        });
    }
    if feature_names.len() != p {
        return Err(CentroidError::Other(format!(
            "feature_names has {} entries, data has {p} columns",
            feature_names.len()
        )));
    }
    let k = class_labels.len();
    if k < 2 {
        return Err(CentroidError::TooFewClasses(k));
    }
    for &c in y {
        if c >= k {
            return Err(CentroidError::BadLabel { label: c, k });
        }
    }
    if !x.col_iter().all(|col| col.is_all_finite()) {
        return Err(CentroidError::NonFinite);
    }

    let stats = PamStats::fit(x, y, k)?;

    let grid: Vec<f64> = match delta_grid {
        Some(g) => {
            if g.is_empty() || g.iter().any(|v| !v.is_finite() || *v < 0.0) {
                return Err(CentroidError::BadGrid);
            }
            if g.windows(2).any(|w| w[0] > w[1]) {
                return Err(CentroidError::BadGrid);
            }
            g
        }
        None => default_grid(&stats)?,
    };

    // full grid is kept (pamr parity); nonzero counts are diagnostics
    let delta = grid;
    let nonzero_features: Vec<usize> = delta
        .iter()
        .map(|&dlt| stats.nonzero_features(dlt))
        .collect();

    // CV error curve over the fixed grid (per-fold stats on train only)
    let (cv_error, cv_se, delta_min, delta_1se) = match folds {
        Some(folds) => {
            let mut miss = vec![0u64; delta.len()];
            let mut total = 0u64;
            for (train, test) in folds {
                let y_train: Vec<usize> = train.iter().map(|&i| y[i]).collect();
                let x_train = Mat::from_fn(train.len(), p, |r, j| x[(train[r], j)]);
                let fold_stats = PamStats::fit(&x_train, &y_train, k)?;
                let x_test = Mat::from_fn(test.len(), p, |r, j| x[(test[r], j)]);
                for (di, &dlt) in delta.iter().enumerate() {
                    let cent = fold_stats.shrunk_centroids(dlt);
                    let disc = discriminants(
                        &fold_stats.grand_mean,
                        &fold_stats.pooled_sd,
                        &fold_stats.priors,
                        &cent,
                        &x_test,
                    )?;
                    for (r, &i) in test.iter().enumerate() {
                        if argmax(&disc[r]) != y[i] {
                            miss[di] += 1;
                        }
                    }
                }
                total += test.len() as u64;
            }
            let n_f64 = total as f64;
            let cv_error: Vec<f64> = miss.iter().map(|&m| m as f64 / n_f64).collect();
            let cv_se: Vec<f64> = cv_error
                .iter()
                .map(|&e| (e * (1.0 - e) / n_f64).sqrt())
                .collect();

            // min error (ties → largest Δ), then the 1SE rule
            let mut min_i = 0;
            for (i, &e) in cv_error.iter().enumerate() {
                if e <= cv_error[min_i] {
                    min_i = i;
                }
            }
            let bound = cv_error[min_i] + cv_se[min_i];
            let mut se_i = min_i;
            for (i, &e) in cv_error.iter().enumerate() {
                if e <= bound && delta[i] >= delta[se_i] {
                    se_i = i;
                }
            }
            (cv_error, cv_se, delta[min_i], delta[se_i])
        }
        None => (Vec::new(), Vec::new(), 0.0, 0.0),
    };

    let delta_selected = if folds.is_some() {
        match select {
            DeltaSelect::MinError => delta_min,
            DeltaSelect::OneSe => delta_1se,
        }
    } else {
        delta[0] // no CV evidence: no shrinkage
    };

    let model = PamModel {
        class_labels: class_labels.to_vec(),
        grand_mean: stats.grand_mean.clone(),
        pooled_sd: stats.pooled_sd.clone(),
        se_scale: stats.se_scale.clone(),
        priors: stats.priors.clone(),
        shrunk_z_centroids: stats.shrunk_centroids(delta_selected),
        delta_selected,
    };
    let curve = PamCvCurve {
        delta,
        cv_error,
        cv_se,
        nonzero_features,
        delta_min,
        delta_1se,
    };

    // signature panel: nonzero d'_kj at the selected Δ
    let mut signature = Vec::new();
    for c in 0..k {
        for j in 0..p {
            let d = stats.d[c][j];
            let shrunk = d.signum() * (d.abs() - delta_selected).max(0.0);
            if shrunk != 0.0 {
                signature.push(SignatureEntry {
                    class_index: c,
                    class_label: class_labels[c].clone(),
                    feature_index: j,
                    feature_name: feature_names[j].clone(),
                    d_shrunk: shrunk,
                });
            }
        }
    }

    Ok((model, curve, signature))
}

/// Default Δ grid: `seq(0, max|d|, length = 30)` (R's from + i·by form).
fn default_grid(stats: &PamStats) -> Result<Vec<f64>> {
    let dmax = stats
        .d
        .iter()
        .flat_map(|row| row.iter())
        .fold(0.0f64, |m, &v| m.max(v.abs()));
    if dmax == 0.0 {
        return Err(CentroidError::Degenerate);
    }
    let by = dmax / 29.0;
    Ok((0..30).map(|i| i as f64 * by).collect())
}

/// Predict classes and posterior probabilities for new samples.
pub fn pam_predict(model: &PamModel, x: &Mat<f64>) -> Result<ClassProbs> {
    if x.nrows() == 0 {
        return Err(CentroidError::Empty);
    }
    let disc = discriminants(
        &model.grand_mean,
        &model.pooled_sd,
        &model.priors,
        &model.shrunk_z_centroids,
        x,
    )?;
    Ok(class_probs(&disc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelArtifact;

    fn mat(rows: &[Vec<f64>]) -> Mat<f64> {
        Mat::from_fn(rows.len(), rows[0].len(), |i, j| rows[i][j])
    }

    fn toy_data() -> (Mat<f64>, Vec<usize>, Vec<String>, Vec<String>) {
        // 3 classes × 6 samples; first three features informative,
        // last three constant/noise so they shrink away at large Δ.
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for c in 0..3usize {
            for r in 0..6 {
                let base = (c as f64 - 1.0) * 3.0 + (r as f64 - 2.5) * 0.15;
                rows.push(vec![
                    base,
                    base * 0.8,
                    -base * 0.5,
                    r as f64 * 0.1,
                    0.5,
                    -0.2,
                ]);
                y.push(c);
            }
        }
        let labels: Vec<String> = (0..3).map(|c| format!("class_{c}")).collect();
        let feats: Vec<String> = (0..6).map(|j| format!("f{j}")).collect();
        (mat(&rows), y, labels, feats)
    }

    #[test]
    fn test_fit_predict_separable() {
        let (x, y, labels, feats) = toy_data();
        let folds = crate::split::stratified_kfold(&y, 3, true, 42).unwrap();
        let (model, curve, sig) = pam_fit(
            &x,
            &y,
            &labels,
            &feats,
            None,
            Some(&folds),
            DeltaSelect::MinError,
        )
        .unwrap();
        let out = pam_predict(&model, &x).unwrap();
        assert_eq!(out.predictions, y);
        for (i, row) in out.probabilities.iter().enumerate() {
            assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-12, "row {i}");
            assert!(row[y[i]] > 0.9, "row {i}: {row:?}");
        }
        assert!(!sig.is_empty());
        assert!(sig.iter().all(|e| e.class_label == labels[e.class_index]));
        // perfect separation at Δ=0 and at the selected Δ; the curve only
        // rises to the prior baseline where everything shrinks away
        assert_eq!(curve.cv_error[0], 0.0);
        assert_eq!(
            curve.cv_error.iter().fold(f64::INFINITY, |m, &e| m.min(e)),
            0.0
        );
        assert!(model.delta_selected > 0.0); // ties resolve to the largest zero-error Δ
        assert_eq!(curve.delta.len(), curve.cv_error.len());
        assert_eq!(curve.delta.len(), curve.nonzero_features.len());
        // default grid: starts at 0, non-decreasing, top ≈ max|d|
        assert_eq!(curve.delta[0], 0.0);
        assert!(curve.delta.windows(2).all(|w| w[0] <= w[1]));
        assert!(*curve.delta.last().unwrap() > 1.0);
        // monotone shrinkage: nonzero features never increase with Δ
        assert!(curve.nonzero_features.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn test_huge_delta_empties_signature_prior_prediction() {
        let (x, y, labels, feats) = toy_data();
        // uniform priors → argmax disc = argmax log π = first class
        let (model, _curve, sig) = pam_fit(
            &x,
            &y,
            &labels,
            &feats,
            Some(vec![1e6]),
            None,
            DeltaSelect::MinError,
        )
        .unwrap();
        assert!(sig.is_empty());
        assert!(
            model
                .shrunk_z_centroids
                .iter()
                .all(|r| r.iter().all(|&v| v == 0.0))
        );
        let out = pam_predict(&model, &x).unwrap();
        assert!(out.predictions.iter().all(|&c| c == 0));
        // uniform priors: posteriors all 1/3
        assert!(
            out.probabilities[0]
                .iter()
                .all(|&v| (v - 1.0 / 3.0).abs() < 1e-12)
        );
    }

    #[test]
    fn test_single_point_grid_selects_that_delta() {
        let (x, y, labels, feats) = toy_data();
        let folds = crate::split::stratified_kfold(&y, 3, true, 42).unwrap();
        let (model, _curve, _sig) = pam_fit(
            &x,
            &y,
            &labels,
            &feats,
            Some(vec![0.5]),
            Some(&folds),
            DeltaSelect::OneSe,
        )
        .unwrap();
        assert!((model.delta_selected - 0.5).abs() < 1e-12);
    }

    #[test]
    fn test_fold_missing_class_rejected() {
        let (x, y, labels, feats) = toy_data();
        // train rows 6..18 hold classes 1 & 2 only
        let folds = vec![((6..18).collect(), (0..6).collect())];
        let err = pam_fit(
            &x,
            &y,
            &labels,
            &feats,
            None,
            Some(&folds),
            DeltaSelect::MinError,
        )
        .unwrap_err();
        assert!(matches!(err, CentroidError::MissingClass(0)), "{err}");
    }

    #[test]
    fn test_validation_errors() {
        let (x, y, labels, feats) = toy_data();
        assert!(
            pam_fit(
                &x,
                &y[..2],
                &labels,
                &feats,
                None,
                None,
                DeltaSelect::MinError
            )
            .is_err()
        );
        let one_label = vec!["a".to_string()];
        assert!(
            pam_fit(
                &x,
                &y,
                &one_label,
                &feats,
                None,
                None,
                DeltaSelect::MinError
            )
            .is_err()
        );
        let bad_y: Vec<usize> = y.iter().map(|&c| c + 5).collect();
        assert!(
            pam_fit(
                &x,
                &bad_y,
                &labels,
                &feats,
                None,
                None,
                DeltaSelect::MinError
            )
            .is_err()
        );
        let short_feats: Vec<String> = feats[..3].to_vec();
        assert!(
            pam_fit(
                &x,
                &y,
                &labels,
                &short_feats,
                None,
                None,
                DeltaSelect::MinError
            )
            .is_err()
        );
        assert!(
            pam_fit(
                &x,
                &y,
                &labels,
                &feats,
                Some(vec![1.0, 0.5]),
                None,
                DeltaSelect::MinError
            )
            .is_err()
        );
        assert!(
            pam_fit(
                &x,
                &y,
                &labels,
                &feats,
                Some(vec![-1.0]),
                None,
                DeltaSelect::MinError
            )
            .is_err()
        );
        // non-finite input
        let mut bad_rows = vec![vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0]; 6];
        bad_rows.push(vec![f64::NAN; 6]);
        let bad_x = mat(&bad_rows);
        let bad_y2 = vec![0, 1, 0, 1, 0, 1, 0];
        assert!(
            pam_fit(
                &bad_x,
                &bad_y2,
                &labels,
                &feats,
                None,
                None,
                DeltaSelect::MinError
            )
            .is_err()
        );
        // wrong-width predict
        let (model, _, _) =
            pam_fit(&x, &y, &labels, &feats, None, None, DeltaSelect::MinError).unwrap();
        let bad = Mat::zeros(2, 3);
        assert!(pam_predict(&model, &bad).is_err());
    }

    #[test]
    fn test_artifact_roundtrip() {
        let (x, y, labels, feats) = toy_data();
        let (model, _, _) =
            pam_fit(&x, &y, &labels, &feats, None, None, DeltaSelect::MinError).unwrap();
        let artifact = ModelArtifact::new(
            PAM_KIND,
            &model,
            feats.clone(),
            serde_json::json!({"delta": model.delta_selected, "k": labels.len()}),
        )
        .unwrap();
        let bytes = artifact.to_bytes().unwrap();
        let back = ModelArtifact::from_bytes(&bytes).unwrap();
        assert_eq!(back.kind, PAM_KIND);
        let restored: PamModel = back.deserialize_fitted().unwrap();
        assert_eq!(restored.class_labels, model.class_labels);
        assert_eq!(restored.delta_selected, model.delta_selected);
        let out1 = pam_predict(&model, &x).unwrap();
        let out2 = pam_predict(&restored, &x).unwrap();
        assert_eq!(out1.predictions, out2.predictions);
        for (a, b) in out1.probabilities.iter().zip(&out2.probabilities) {
            assert!(a.iter().zip(b).all(|(u, v)| (u - v).abs() < 1e-12));
        }
    }

    #[test]
    fn test_median() {
        assert!((median(&[1.0, 2.0, 3.0]) - 2.0).abs() < 1e-12);
        assert!((median(&[1.0, 2.0, 3.0, 4.0]) - 2.5).abs() < 1e-12);
    }
}
