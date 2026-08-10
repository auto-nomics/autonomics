//! Forest types and high-level trainer API.
//!
//! Each forest kind is represented by a [`ForestKind`] enum tag and a
//! dedicated trainer struct (e.g. [`RegressionTrainer`]) that knows how to
//! call into grf-sys. Trained forests are wrapped in [`ForestBlob`], an
//! owned opaque byte buffer that owns the underlying C++ `grf::Forest`
//! via the FFI boundary.
//!
//! ## Serialization
//!
//! `ForestBlob::serialize` round-trips through the C ABI's grf binary
//! format. This is **byte-identical** to what `RcppUtilities::serialize_forest`
//! produces, so a forest trained in R can in principle be deserialized
//! here, and vice versa (modulo a small difference in how OOB predictions
//! are stored — see note on [`ForestBlob::oob_predictions`]).
//!
//! ## Threading
//!
//! GRF forests are not safe to share across threads while training or
//! predicting — the underlying Eigen buffers are mutated in-place during
//! prediction. `ForestBlob` is therefore `!Send`. Wrap in `Arc<Mutex<_>>`
//! if you need concurrent access from multiple threads.

use std::sync::Arc;

use serde::Serialize;

use grf_sys as sys;

use crate::data::Matrix;
use crate::{GrfError, Result};

/// Identifier for which forest type a [`ForestBlob`] holds.
///
/// Used by [`ForestBlob::kind`] to dispatch prediction / analysis operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ForestKind {
    Regression,
    Causal,
    Instrumental,
    Quantile,
    Probability,
    Survival,
    MultiRegression,
    MultiCausal,
    CausalSurvival,
    Lm,
    LlRegression,
}

impl ForestKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Regression => "regression",
            Self::Causal => "causal",
            Self::Instrumental => "instrumental",
            Self::Quantile => "quantile",
            Self::Probability => "probability",
            Self::Survival => "survival",
            Self::MultiRegression => "multi_regression",
            Self::MultiCausal => "multi_causal",
            Self::CausalSurvival => "causal_survival",
            Self::Lm => "lm",
            Self::LlRegression => "ll_regression",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "regression" => Self::Regression,
            "causal" => Self::Causal,
            "instrumental" => Self::Instrumental,
            "quantile" => Self::Quantile,
            "probability" => Self::Probability,
            "survival" => Self::Survival,
            "multi_regression" => Self::MultiRegression,
            "multi_causal" => Self::MultiCausal,
            "causal_survival" => Self::CausalSurvival,
            "lm" => Self::Lm,
            "ll_regression" => Self::LlRegression,
            _ => return None,
        })
    }

    /// Number of "label" columns the data matrix needs in addition to the
    /// `p` feature columns (e.g. Causal needs 2 extra: Y_centered and W_centered).
    pub fn extra_columns(&self) -> usize {
        match self {
            Self::Regression | Self::Quantile | Self::Probability | Self::LlRegression => 1,
            Self::Causal | Self::Survival | Self::MultiCausal | Self::Lm => 2,
            Self::Instrumental | Self::CausalSurvival => 3,
            Self::MultiRegression => 0, // caller provides any number of outcome cols
        }
    }
}

/// Trained forest blob. Owns a `grf_forest_t*` from grf-sys.
///
/// Cloning is cheap (the underlying C++ handle is reference-counted via
/// `Arc` — see [`ForestBlob::clone`]). Drop releases the C++ handle.
#[derive(Clone)]
pub struct ForestBlob {
    inner: Arc<sys::Forest>,
    kind: ForestKind,
    n_features: usize,
}

// Manual Debug impl — the inner `sys::Forest` is not Debug (opaque FFI
// handle). Surface only the kind/feature count/total trees.
impl std::fmt::Debug for ForestBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForestBlob")
            .field("kind", &self.kind)
            .field("n_features", &self.n_features)
            .field("num_trees", &self.inner.num_trees())
            .finish()
    }
}

// Manual Serialize impl — the inner `sys::Forest` is an opaque FFI handle
// (no serde representation). We serialize only the discriminant metadata
// so the DAG layer can use this struct as a tagged payload.
impl Serialize for ForestBlob {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("ForestBlob", 3)?;
        st.serialize_field("kind", &self.kind)?;
        st.serialize_field("n_features", &self.n_features)?;
        st.serialize_field("num_trees", &self.inner.num_trees())?;
        st.end()
    }
}

impl ForestBlob {
    /// Take ownership of a raw `grf_sys::Forest`. Internal-use only.
    pub(crate) fn from_sys(
        inner: sys::Forest,
        kind: ForestKind,
        n_features: usize,
    ) -> Self {
        Self { inner: Arc::new(inner), kind, n_features }
    }

    pub fn kind(&self) -> ForestKind { self.kind }
    pub fn n_features(&self) -> usize { self.n_features }
    pub fn num_trees(&self) -> usize { self.inner.num_trees() }

    /// OOB predictions captured at training time (one column-major block).
    /// Returns `None` if the forest was trained with
    /// `compute_oob_predictions=false`.
    pub fn oob_predictions(&self) -> Option<OobPredictions> {
        let (values, pred_length) = self.inner.oob_predictions()?;
        Some(OobPredictions { values, pred_length })
    }

    /// Number of prediction outputs per row (1 for regression/causal/etc;
    /// ≥1 for multi-output forests).
    pub fn pred_length(&self) -> usize {
        // Sensible default; specific trainers override.
        match self.kind {
            ForestKind::Regression | ForestKind::Causal | ForestKind::Instrumental
            | ForestKind::Survival | ForestKind::LlRegression => 1,
            ForestKind::Quantile => 0, // depends on input quantile count
            ForestKind::Probability => 0, // depends on num_classes
            ForestKind::MultiRegression | ForestKind::MultiCausal | ForestKind::Lm => 0,
            ForestKind::CausalSurvival => 0, // depends on failure_times length
        }
    }

    /// Serialize to a portable binary buffer. The format is the grf-internal
    /// forest representation (matching what `RcppUtilities::serialize_forest`
    /// emits in R), so forests can be exchanged between R and Rust.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        self.inner.serialize().map_err(GrfError::from)
    }

    pub fn deserialize(bytes: &[u8], kind: ForestKind, n_features: usize) -> Result<Self> {
        let inner = sys::Forest::deserialize(bytes).map_err(GrfError::from)?;
        let blob = Self::from_sys(inner, kind, n_features);
        // Round-trip kind: the buffer's stored kind must match the caller's claim.
        let actual = ForestKind::from_str(blob.inner.kind())
            .ok_or_else(|| GrfError::Shape(format!("unknown kind in buffer: {}", blob.inner.kind())))?;
        if actual != kind {
            return Err(GrfError::KindMismatch { trained: actual, requested: kind });
        }
        Ok(blob)
    }

    /// Borrow the underlying grf-sys handle. Used by the predict / analysis
    /// helpers below; not generally needed by user code.
    pub fn inner(&self) -> &sys::Forest { &self.inner }

    /// Forest weights α(test_row, train_row) — co-leaf occupancy counts
    /// divided by total trees. Returns a dense column-major buffer of
    /// shape `(n_train × n_test)`.
    pub fn compute_weights(
        &self,
        train_data: &[f64], n_train_rows: usize, n_train_cols: usize,
        test_data: &[f64], n_test_rows: usize, n_test_cols: usize,
        num_threads: u32,
    ) -> Option<Vec<f64>> {
        self.inner.compute_weights(
            train_data, n_train_rows, n_train_cols,
            test_data, n_test_rows, n_test_cols,
            num_threads,
        )
    }

    /// OOB forest weights over the training set.
    pub fn compute_weights_oob(
        &self,
        train_data: &[f64], n_train_rows: usize, n_train_cols: usize,
        num_threads: u32,
    ) -> Option<Vec<f64>> {
        self.inner.compute_weights_oob(
            train_data, n_train_rows, n_train_cols,
            num_threads,
        )
    }

    /// Compute split-frequency matrix (depth × n_features).
    pub fn compute_split_frequencies(&self, max_depth: usize) -> Option<Vec<Vec<u64>>> {
        self.inner.compute_split_frequencies(max_depth)
    }
}

/// OOB predictions captured at training time.
#[derive(Debug, Clone)]
pub struct OobPredictions {
    /// Column-major flat buffer of shape (pred_length, n_samples).
    pub values: Vec<f64>,
    pub pred_length: usize,
}

/// Per-forest statistics surfaced to the agent layer.
#[derive(Debug, Clone, Serialize)]
pub struct ForestStats {
    pub kind: ForestKind,
    pub num_trees: usize,
    pub n_features: usize,
    pub pred_length: usize,
    pub has_oob_predictions: bool,
}

impl From<&ForestBlob> for ForestStats {
    fn from(b: &ForestBlob) -> Self {
        ForestStats {
            kind: b.kind,
            num_trees: b.num_trees(),
            n_features: b.n_features,
            pred_length: b.pred_length(),
            has_oob_predictions: b.oob_predictions().is_some(),
        }
    }
}

/// Request to predict on new data (or to compute OOB predictions).
#[derive(Debug, Clone)]
pub struct PredictRequest {
    pub train_x: Matrix,
    pub train_outcome_index: usize,
    pub test_x: Option<Matrix>,
    pub estimate_variance: bool,
    pub num_threads: Option<u32>,
}

impl PredictRequest {
    pub fn oob(train_x: Matrix, outcome_index: usize, num_threads: Option<u32>) -> Self {
        Self {
            train_x, train_outcome_index: outcome_index,
            test_x: None, estimate_variance: false, num_threads,
        }
    }
    pub fn new_data(train_x: Matrix, outcome_index: usize, test_x: Matrix, estimate_variance: bool) -> Self {
        Self {
            train_x, train_outcome_index: outcome_index,
            test_x: Some(test_x), estimate_variance, num_threads: None,
        }
    }
}

/// Predictions returned by a forest.
#[derive(Debug, Clone)]
pub struct Predictions {
    /// Column-major flat buffer of shape (pred_length, n_samples).
    pub values: Vec<f64>,
    pub variance: Option<Vec<f64>>,
    pub debiased_error: Option<Vec<f64>>,
    pub excess_error: Option<Vec<f64>>,
    pub pred_length: usize,
}

impl Predictions {
    pub fn n_samples(&self) -> usize {
        if self.pred_length == 0 { 0 } else { self.values.len() / self.pred_length }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// trainers
// ═══════════════════════════════════════════════════════════════════════

/// Input shape required by [`RegressionTrainer::fit`].
#[derive(Debug, Clone)]
pub struct RegressionSpec {
    pub x: Matrix,
    pub y: Vec<f64>,
    pub sample_weights: Option<Vec<f64>>,
    pub options: sys::TrainOptions,
}

pub struct RegressionTrainer;

impl RegressionTrainer {
    pub fn fit(spec: RegressionSpec) -> Result<ForestBlob> {
        if spec.x.n_rows != spec.y.len() {
            return Err(GrfError::Shape(format!(
                "x.n_rows={} != y.len={}", spec.x.n_rows, spec.y.len()
            )));
        }
        let n_features = spec.x.n_cols;
        let n_rows = spec.x.n_rows;
        // grf expects column-major [X..., Y]. We pack into a single buffer.
        let mut data = spec.x.data.clone();
        data.extend_from_slice(&spec.y);
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let raw = unsafe {
            sys::ffi::grf_train_regression(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                n_features, // outcome_index = column right after X
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::Regression, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct CausalSpec {
    pub x: Matrix,
    /// Outcome, possibly centered (Y - Y.hat). Caller is responsible for the
    /// R-learner first stage if `y_hat` / `w_hat` are not given.
    pub y_centered: Vec<f64>,
    /// Treatment, possibly centered (W - W.hat).
    pub w_centered: Vec<f64>,
    /// Optional pre-computed Y.hat / W.hat to expose in the output for
    /// downstream ATE / BLP / RATE computation.
    pub y_hat: Option<Vec<f64>>,
    pub w_hat: Option<Vec<f64>>,
    pub sample_weights: Option<Vec<f64>>,
    pub stabilize_splits: bool,
    pub options: sys::TrainOptions,
}

pub struct CausalTrainer;

impl CausalTrainer {
    pub fn fit(spec: CausalSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        if spec.y_centered.len() != n_rows || spec.w_centered.len() != n_rows {
            return Err(GrfError::Shape("y/w length mismatch".into()));
        }
        let mut data = spec.x.data.clone();
        let y_idx = n_features;
        let w_idx = n_features + 1;
        let outcome_offset = data.len();
        data.resize(outcome_offset + 2 * n_rows, 0.0);
        for i in 0..n_rows {
            data[outcome_offset + i] = spec.y_centered[i];
            data[outcome_offset + n_rows + i] = spec.w_centered[i];
        }
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let raw = unsafe {
            sys::ffi::grf_train_causal(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                y_idx, w_idx,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                spec.stabilize_splits,
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::Causal, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct InstrumentalSpec {
    pub x: Matrix,
    pub y_centered: Vec<f64>,
    pub w_centered: Vec<f64>,
    pub z_centered: Vec<f64>,
    pub sample_weights: Option<Vec<f64>>,
    pub reduced_form_weight: f64,
    pub stabilize_splits: bool,
    pub options: sys::TrainOptions,
}

pub struct InstrumentalTrainer;

impl InstrumentalTrainer {
    pub fn fit(spec: InstrumentalSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let mut data = spec.x.data.clone();
        let base = data.len();
        data.resize(base + 3 * n_rows, 0.0);
        for i in 0..n_rows {
            data[base + i] = spec.y_centered[i];
            data[base + n_rows + i] = spec.w_centered[i];
            data[base + 2 * n_rows + i] = spec.z_centered[i];
        }
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let raw = unsafe {
            sys::ffi::grf_train_instrumental(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                n_features, n_features + 1, n_features + 2,
                spec.reduced_form_weight, spec.stabilize_splits,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::Instrumental, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct QuantileSpec {
    pub x: Matrix,
    pub y: Vec<f64>,
    pub quantiles: Vec<f64>,
    pub regression_splitting: bool,
    pub options: sys::TrainOptions,
}

pub struct QuantileTrainer;

impl QuantileTrainer {
    pub fn fit(spec: QuantileSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let mut data = spec.x.data.clone();
        data.extend_from_slice(&spec.y);
        let opts = spec.options.to_ffi();
        let raw = unsafe {
            sys::ffi::grf_train_quantile(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                n_features,
                spec.quantiles.as_ptr(), spec.quantiles.len(),
                spec.regression_splitting,
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::Quantile, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct ProbabilitySpec {
    pub x: Matrix,
    pub y: Vec<f64>,
    pub num_classes: usize,
    pub sample_weights: Option<Vec<f64>>,
    pub options: sys::TrainOptions,
}

pub struct ProbabilityTrainer;

impl ProbabilityTrainer {
    pub fn fit(spec: ProbabilitySpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let mut data = spec.x.data.clone();
        data.extend_from_slice(&spec.y);
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let raw = unsafe {
            sys::ffi::grf_train_probability(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                n_features, spec.num_classes,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::Probability, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct SurvivalSpec {
    pub x: Matrix,
    pub time: Vec<f64>,
    pub censor: Vec<f64>,
    pub failure_times: Option<Vec<f64>>,
    pub sample_weights: Option<Vec<f64>>,
    pub options: sys::TrainOptions,
}

pub struct SurvivalTrainer;

impl SurvivalTrainer {
    pub fn fit(spec: SurvivalSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let mut data = spec.x.data.clone();
        data.extend_from_slice(&spec.time);
        data.extend_from_slice(&spec.censor);
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let (ft_ptr, ft_len) = spec.failure_times
            .as_ref()
            .map(|v| (v.as_ptr(), v.len()))
            .unwrap_or((std::ptr::null(), 0));
        let raw = unsafe {
            sys::ffi::grf_train_survival(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                n_features, n_features + 1,
                ft_ptr, ft_len,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::Survival, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct MultiRegressionSpec {
    pub x: Matrix,
    /// Multiple outcome columns. Each column corresponds to one task.
    pub y_columns: Vec<Vec<f64>>,
    pub sample_weights: Option<Vec<f64>>,
    pub options: sys::TrainOptions,
}

pub struct MultiRegressionTrainer;

impl MultiRegressionTrainer {
    pub fn fit(spec: MultiRegressionSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let num_outcomes = spec.y_columns.len();
        let mut data = spec.x.data.clone();
        // Concatenate outcome columns column-major: col j of outcome m goes
        // right after col (m-1).
        let base = data.len();
        data.resize(base + num_outcomes * n_rows, 0.0);
        for (m, col) in spec.y_columns.iter().enumerate() {
            if col.len() != n_rows {
                return Err(GrfError::Shape(format!(
                    "y_columns[{}].len()={} != n_rows={}", m, col.len(), n_rows
                )));
            }
            for i in 0..n_rows {
                data[base + m * n_rows + i] = col[i];
            }
        }
        // Indices of the outcome columns in the packed data.
        let outcome_indices: Vec<usize> = (0..num_outcomes)
            .map(|m| n_features + m)
            .collect();
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let raw = unsafe {
            sys::ffi::grf_train_multi_regression(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                outcome_indices.as_ptr(), num_outcomes,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::MultiRegression, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct MultiCausalSpec {
    pub x: Matrix,
    pub y_columns: Vec<Vec<f64>>,
    /// Treatment column.
    pub w: Vec<f64>,
    pub num_treatments: usize,
    pub gradient_weights: Option<Vec<f64>>,
    pub stabilize_splits: bool,
    pub sample_weights: Option<Vec<f64>>,
    pub options: sys::TrainOptions,
}

pub struct MultiCausalTrainer;

impl MultiCausalTrainer {
    pub fn fit(spec: MultiCausalSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let num_outcomes = spec.y_columns.len();
        let mut data = spec.x.data.clone();
        let base = data.len();
        data.resize(base + (num_outcomes + 1) * n_rows, 0.0);
        for (m, col) in spec.y_columns.iter().enumerate() {
            for i in 0..n_rows {
                data[base + m * n_rows + i] = col[i];
            }
        }
        for i in 0..n_rows {
            data[base + num_outcomes * n_rows + i] = spec.w[i];
        }
        let outcome_indices: Vec<usize> = (0..num_outcomes)
            .map(|m| n_features + m)
            .collect();
        let treatment_index = n_features + num_outcomes;
        let (gw_ptr, gw_len) = spec.gradient_weights
            .as_ref()
            .map(|v| (v.as_ptr(), v.len()))
            .unwrap_or((std::ptr::null(), 0));
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let raw = unsafe {
            sys::ffi::grf_train_multi_causal(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                outcome_indices.as_ptr(), num_outcomes,
                treatment_index, spec.num_treatments,
                gw_ptr, gw_len,
                spec.stabilize_splits,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::MultiCausal, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct CausalSurvivalSpec {
    pub x: Matrix,
    pub time: Vec<f64>,
    pub w: Vec<f64>,
    pub censor: Vec<f64>,
    /// 0 = RMST, 1 = survival probability.
    pub target: i32,
    pub horizon: f64,
    pub failure_times: Option<Vec<f64>>,
    pub sample_weights: Option<Vec<f64>>,
    pub options: sys::TrainOptions,
}

pub struct CausalSurvivalTrainer;

impl CausalSurvivalTrainer {
    pub fn fit(spec: CausalSurvivalSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let mut data = spec.x.data.clone();
        let base = data.len();
        data.resize(base + 3 * n_rows, 0.0);
        for i in 0..n_rows {
            data[base + i] = spec.time[i];
            data[base + n_rows + i] = spec.w[i];
            data[base + 2 * n_rows + i] = spec.censor[i];
        }
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let (ft_ptr, ft_len) = spec.failure_times
            .as_ref()
            .map(|v| (v.as_ptr(), v.len()))
            .unwrap_or((std::ptr::null(), 0));
        let raw = unsafe {
            sys::ffi::grf_train_causal_survival(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                n_features, n_features + 1, n_features + 2,
                spec.target, spec.horizon,
                ft_ptr, ft_len,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::CausalSurvival, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct LmSpec {
    pub x: Matrix,
    pub y_columns: Vec<Vec<f64>>,
    pub w_columns: Vec<Vec<f64>>,
    pub sample_weights: Option<Vec<f64>>,
    pub options: sys::TrainOptions,
}

pub struct LmTrainer;

impl LmTrainer {
    pub fn fit(spec: LmSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let num_outcomes = spec.y_columns.len();
        let num_regressors = spec.w_columns.len();
        let mut data = spec.x.data.clone();
        let base = data.len();
        data.resize(base + (num_outcomes + num_regressors) * n_rows, 0.0);
        for (m, col) in spec.y_columns.iter().enumerate() {
            for i in 0..n_rows {
                data[base + m * n_rows + i] = col[i];
            }
        }
        for (m, col) in spec.w_columns.iter().enumerate() {
            for i in 0..n_rows {
                data[base + (num_outcomes + m) * n_rows + i] = col[i];
            }
        }
        let y_indices: Vec<usize> = (0..num_outcomes).map(|m| n_features + m).collect();
        let w_indices: Vec<usize> = (0..num_regressors).map(|m| n_features + num_outcomes + m).collect();
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let raw = unsafe {
            sys::ffi::grf_train_lm(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                y_indices.as_ptr(), num_outcomes,
                w_indices.as_ptr(), num_regressors,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::Lm, n_features))
    }
}

#[derive(Debug, Clone)]
pub struct LlRegressionSpec {
    pub x: Matrix,
    pub y: Vec<f64>,
    pub ll_split_lambda: f64,
    pub ll_split_weight_penalty: bool,
    pub ll_split_variables: Vec<usize>,
    pub ll_split_cutoff: usize,
    pub sample_weights: Option<Vec<f64>>,
    pub options: sys::TrainOptions,
}

pub struct LlRegressionTrainer;

impl LlRegressionTrainer {
    pub fn fit(spec: LlRegressionSpec) -> Result<ForestBlob> {
        let n_rows = spec.x.n_rows;
        let n_features = spec.x.n_cols;
        let mut data = spec.x.data.clone();
        data.extend_from_slice(&spec.y);
        let opts = spec.options.to_ffi();
        let sw = spec.sample_weights.as_deref();
        let raw = unsafe {
            sys::ffi::grf_train_ll_regression(
                data.as_ptr(), n_rows, n_cols(&data, n_rows),
                n_features,
                spec.ll_split_lambda, spec.ll_split_weight_penalty,
                spec.ll_split_variables.as_ptr(), spec.ll_split_variables.len(),
                spec.ll_split_cutoff,
                sw.map_or(std::ptr::null(), |s| s.as_ptr()),
                &opts,
            )
        };
        let forest = unsafe { sys::Forest::from_raw(raw) }
            .ok_or(GrfError::Sys(sys::GrfError::NullHandle))?;
        Ok(ForestBlob::from_sys(forest, ForestKind::LlRegression, n_features))
    }
}

// ─────────────────────────── prediction helpers ───────────────────────────

impl ForestBlob {
    /// Predict on new test data, returning point predictions (and optionally
    /// variance estimates if `request.estimate_variance = true`).
    pub fn predict(&self, request: PredictRequest) -> Result<Predictions> {
        let num_threads = request.num_threads.unwrap_or(0);
        let raw = if let Some(test) = &request.test_x {
            unsafe {
                sys::ffi::grf_predict(
                    self.inner.inner_handle(),
                    request.train_x.data.as_ptr(),
                    request.train_x.n_rows, request.train_x.n_cols,
                    request.train_outcome_index,
                    test.data.as_ptr(),
                    test.n_rows, test.n_cols,
                    request.estimate_variance, num_threads,
                )
            }
        } else {
            unsafe {
                sys::ffi::grf_predict_oob(
                    self.inner.inner_handle(),
                    request.train_x.data.as_ptr(),
                    request.train_x.n_rows, request.train_x.n_cols,
                    request.train_outcome_index,
                    request.estimate_variance, num_threads,
                )
            }
        };
        let preds = unsafe { sys::Predictions::from_raw(raw) }
            .ok_or_else(|| GrfError::Sys(sys::GrfError::Cpp(sys::check_error().to_string())))?;
        Ok(Predictions {
            values: preds.values(),
            variance: preds.variance(),
            debiased_error: preds.debiased_error(),
            excess_error: preds.excess_error(),
            pred_length: preds.pred_length(),
        })
    }
}

// ─────────────────────────── helpers ───────────────────────────

fn n_cols(data: &[f64], n_rows: usize) -> usize {
    if n_rows == 0 { 0 } else { data.len() / n_rows }
}