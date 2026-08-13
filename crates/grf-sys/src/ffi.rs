//! Raw `extern "C"` declarations matching `wrapper/grf_shim.h`. Callers
//! should prefer the safe wrappers in the parent module.
//!
//! Every handle here is opaque. Use `Forest::from_raw` /
//! `Predictions::from_raw` to upgrade them.

use std::ffi::{c_char, c_double, c_uint, c_ulonglong};
use std::os::raw::c_uchar;

// Opaque types — their layout is unknown to Rust.
#[repr(C)]
pub struct grf_forest_t {
    _private: [u8; 0],
}

#[repr(C)]
pub struct grf_predictions_t {
    _private: [u8; 0],
}

#[repr(C)]
pub struct grf_split_freq_t {
    pub num_trees: usize,
    pub max_depth: usize,
    pub depths_x_features: *const c_ulonglong,
    pub n_features: usize,
}

#[repr(C)]
pub struct grf_train_opts_t {
    pub num_trees: c_uint,
    pub ci_group_size: c_uint,
    pub sample_fraction: c_double,
    pub mtry: c_uint,
    pub min_node_size: c_uint,
    pub honesty: bool,
    pub honesty_fraction: c_double,
    pub honesty_prune_leaves: bool,
    pub alpha: c_double,
    pub imbalance_penalty: c_double,
    pub num_threads: c_uint,
    pub seed: c_uint,
    pub legacy_seed: bool,
    pub compute_oob_predictions: bool,
    pub verbose: bool,
    pub clusters: *const c_ulonglong,
    pub n_clusters: usize,
    pub samples_per_cluster: c_uint,
}

unsafe extern "C" {
    // ──── error reporting ────
    pub fn grf_last_error() -> *const c_char;
    pub fn grf_clear_error();

    // ──── training ────
    pub fn grf_train_regression(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_index: usize,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_causal(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_index: usize,
        treatment_index: usize,
        sample_weights: *const c_double,
        stabilize_splits: bool,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_instrumental(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_index: usize,
        treatment_index: usize,
        instrument_index: usize,
        reduced_form_weight: c_double,
        stabilize_splits: bool,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_quantile(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_index: usize,
        quantiles: *const c_double,
        n_quantiles: usize,
        regression_splitting: bool,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_probability(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_index: usize,
        num_classes: usize,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_survival(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_index: usize,
        censor_index: usize,
        failure_times: *const c_double,
        n_failure_times: usize,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_multi_regression(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_indices: *const usize,
        num_outcomes: usize,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_multi_causal(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_indices: *const usize,
        num_outcomes: usize,
        treatment_index: usize,
        num_treatments: usize,
        gradient_weights: *const c_double,
        n_gradient_weights: usize,
        stabilize_splits: bool,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_causal_survival(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_index: usize,
        treatment_index: usize,
        censor_index: usize,
        target: i32,
        horizon: c_double,
        failure_times: *const c_double,
        n_failure_times: usize,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_lm(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_indices: *const usize,
        num_outcomes: usize,
        regressor_indices: *const usize,
        num_regressors: usize,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    pub fn grf_train_ll_regression(
        data: *const c_double,
        n_rows: usize,
        n_cols: usize,
        outcome_index: usize,
        ll_split_lambda: c_double,
        ll_split_weight_penalty: bool,
        ll_split_variables: *const usize,
        n_ll_split_variables: usize,
        ll_split_cutoff: usize,
        sample_weights: *const c_double,
        opts: *const grf_train_opts_t,
    ) -> *mut grf_forest_t;

    // ──── forest metadata ────
    pub fn grf_forest_free(forest: *mut grf_forest_t);
    pub fn grf_forest_kind(forest: *const grf_forest_t) -> *const c_char;
    pub fn grf_forest_num_trees(forest: *const grf_forest_t) -> usize;
    pub fn grf_forest_oob_predictions(
        forest: *const grf_forest_t,
        out_total_len: *mut usize,
        out_pred_length: *mut usize,
    ) -> *const c_double;

    pub fn grf_forest_oob_num_samples(forest: *const grf_forest_t) -> usize;

    // ──── prediction ────
    pub fn grf_predict(
        forest: *const grf_forest_t,
        train_data: *const c_double,
        n_train_rows: usize,
        n_train_cols: usize,
        train_outcome_index: usize,
        test_data: *const c_double,
        n_test_rows: usize,
        n_test_cols: usize,
        estimate_variance: bool,
        num_threads: c_uint,
    ) -> *mut grf_predictions_t;

    pub fn grf_predict_oob(
        forest: *const grf_forest_t,
        train_data: *const c_double,
        n_train_rows: usize,
        n_train_cols: usize,
        train_outcome_index: usize,
        estimate_variance: bool,
        num_threads: c_uint,
    ) -> *mut grf_predictions_t;

    pub fn grf_predictions_free(preds: *mut grf_predictions_t);

    pub fn grf_predictions_num_samples(p: *const grf_predictions_t) -> usize;
    pub fn grf_predictions_pred_length(p: *const grf_predictions_t) -> usize;
    pub fn grf_predictions_values(p: *const grf_predictions_t) -> *const c_double;
    pub fn grf_predictions_variance(p: *const grf_predictions_t) -> *const c_double;
    pub fn grf_predictions_debiased_error(p: *const grf_predictions_t) -> *const c_double;
    pub fn grf_predictions_excess_error(p: *const grf_predictions_t) -> *const c_double;

    // ──── serialization ────
    pub fn grf_forest_serialize(forest: *const grf_forest_t, out_len: *mut usize) -> *mut c_uchar;
    pub fn grf_forest_deserialize(buf: *const c_uchar, len: usize) -> *mut grf_forest_t;
    pub fn grf_forest_get_tree(
        forest: *const grf_forest_t,
        index: usize,
        out_len: *mut usize,
    ) -> *mut c_uchar;
    pub fn grf_forest_merge(forests: *const *const grf_forest_t, n: usize) -> *mut grf_forest_t;

    // ──── analysis ────
    pub fn grf_compute_split_frequencies(
        forest: *const grf_forest_t,
        max_depth: usize,
    ) -> *mut grf_split_freq_t;
    pub fn grf_split_freq_free(sf: *mut grf_split_freq_t);

    pub fn grf_compute_weights(
        forest: *const grf_forest_t,
        train_data: *const c_double,
        n_train_rows: usize,
        n_train_cols: usize,
        test_data: *const c_double,
        n_test_rows: usize,
        n_test_cols: usize,
        out_n_train: *mut usize,
        out_n_test: *mut usize,
        num_threads: c_uint,
    ) -> *mut c_double;

    pub fn grf_compute_weights_oob(
        forest: *const grf_forest_t,
        train_data: *const c_double,
        n_train_rows: usize,
        n_train_cols: usize,
        out_n_train: *mut usize,
        num_threads: c_uint,
    ) -> *mut c_double;
}
