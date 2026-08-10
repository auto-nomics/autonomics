/*
 * grf_shim.h — C ABI wrapper for the GRF C++ core.
 *
 * This is the ONLY header downstream Rust code includes. Every symbol is
 * `extern "C"` so we don't depend on C++ name mangling, and every forest /
 * prediction handle is opaque so we can evolve the C++ representation
 * without breaking ABI.
 *
 * Threading: GRF's training and prediction both spawn worker threads via
 * std::thread internally. The shim itself is thread-safe at the call
 * boundary (each call constructs fresh C++ objects); however, Forest
 * handles are NOT safe to share across threads — each call takes a const
 * Forest* and the caller must serialize concurrent predict/serialize calls.
 *
 * Data layout: input matrices are column-major (Fortran order), matching
 * Eigen's default and exactly what `Data(const double*, n_rows, n_cols)`
 * expects. For a DataFrame X with shape (n_rows, n_cols), the column-
 * major buffer is `X.col(j).data() == &buf[j * n_rows]`.
 *
 * Lifecycle: every `grf_*_new` function must be matched by a `grf_*_free`
 * function. The shim never allocates inside a Forest handle.
 */

#ifndef GRF_SHIM_H_
#define GRF_SHIM_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ─────────────────────────── opaque handles (C view) ─────────────────────────── */

typedef struct grf_forest_t grf_forest_t;
typedef struct grf_predictions_t grf_predictions_t;
typedef struct grf_split_freq_t grf_split_freq_t;

#ifdef __cplusplus
} // extern "C"
#endif

/* ─────────────────────────── opaque handles (C++ view) ─────────────────────────── */

#ifdef __cplusplus
#include <memory>
#include <string>
#include <vector>
namespace grf { class Forest; }
struct grf_forest_t {
    std::string kind;
    std::unique_ptr<grf::Forest> forest;
    std::vector<double> oob_predictions;
    size_t oob_pred_length = 0;
    // Predictor parameters (stored at training time so grf_predict can
    // reconstruct the correct ForestPredictor for this forest type).
    std::vector<double> quantiles;          // quantile forest
    size_t num_classes = 0;                 // probability forest
    size_t num_failures = 0;                // survival forest
    int survival_prediction_type = 0;       // survival forest
    // Survival forest: the failure-time grid the outcome was relabeled to,
    // and the column index of the censor indicator in the training data.
    // Stored so grf_predict can relabel new data identically and set the
    // censor index (Data::is_failure requires it, else bad_optional_access).
    std::vector<double> failure_times;
    size_t censor_index = 0;
    size_t num_outcomes = 0;                // multi_regression, multi_causal, lm
    size_t num_treatments = 0;              // multi_causal, lm
};
struct grf_predictions_t {
    std::vector<double> values;
    std::vector<double> variance;
    std::vector<double> debiased_error;
    std::vector<double> excess_error;
    size_t pred_length = 0;
    bool has_variance = false;
    bool has_error = false;
};
struct grf_split_freq_t {
    size_t num_trees;
    size_t max_depth;
    uint64_t* depths_x_features;
    size_t n_features;
};
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* ─────────────────────────── error reporting ─────────────────────────── */

/// Returns the last error message set by the most recent failing call on
/// this thread. The returned pointer is owned by the shim; do NOT free.
/// Empty string if no error.
const char* grf_last_error(void);

/// Clear the last error.
void grf_clear_error(void);

/* ─────────────────────────── forest training ─────────────────────────── */

/// Common training options shared across forest types. Mirrors
/// grf::ForestOptions but flattened for ABI stability.
typedef struct grf_train_opts_t {
    uint32_t num_trees;          // default 2000
    uint32_t ci_group_size;      // default 2; 1 = no variance estimate
    double sample_fraction;      // default 0.5
    uint32_t mtry;               // 0 = "let grf pick default"
    uint32_t min_node_size;      // default 5
    bool honesty;                // default true
    double honesty_fraction;     // default 0.5
    bool honesty_prune_leaves;   // default true
    double alpha;                // default 0.05
    double imbalance_penalty;    // default 0.0
    uint32_t num_threads;        // 0 = hardware concurrency
    uint32_t seed;               // RNG seed
    bool legacy_seed;            // grf pre-2.4.0 reproducibility mode
    bool compute_oob_predictions; // whether to precompute OOB predictions
    bool verbose;                // log to stderr

    // Optional cluster-robust sampling. clusters == NULL means no clusters.
    const uint64_t* clusters;
    size_t n_clusters;           // == n_rows
    uint32_t samples_per_cluster; // 0 = use full cluster
} grf_train_opts_t;

/// Train a regression forest. `outcome_index` selects which column of the
/// data matrix is Y. `sample_weights` may be NULL. Returns NULL on failure.
grf_forest_t* grf_train_regression(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/// Train a causal forest (R-learner style; pass Y.hat and W.hat in `data`).
/// Layout: data columns are [X..., Y_centered, W_centered]. Y_centered is
/// at outcome_index, W_centered at treatment_index.
/// `stabilize_splits` mirrors grf's option.
grf_forest_t* grf_train_causal(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t treatment_index,
    const double* sample_weights,
    bool stabilize_splits,
    const grf_train_opts_t* opts
);

/// Train an instrumental forest (X, Y_centered, W_centered, Z_centered).
/// outcome_index=Y, treatment_index=W_centered, instrument_index=Z_centered.
grf_forest_t* grf_train_instrumental(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t treatment_index, size_t instrument_index,
    double reduced_form_weight,
    bool stabilize_splits,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/// Train a quantile forest. `quantiles` is a length-`n_quantiles` array.
grf_forest_t* grf_train_quantile(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index,
    const double* quantiles, size_t n_quantiles,
    bool regression_splitting,
    const grf_train_opts_t* opts
);

/// Train a probability (multiclass classification) forest.
/// `num_classes` = number of distinct class labels.
grf_forest_t* grf_train_probability(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index,
    size_t num_classes,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/// Train a survival forest. Layout: [X..., time, censor].
/// `failure_times` may be NULL; if non-NULL, length == n_failure_times.
grf_forest_t* grf_train_survival(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t censor_index,
    const double* failure_times, size_t n_failure_times,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/// Train a multi-task regression forest. `outcome_indices` is an array of
/// `num_outcomes` column indices into the data matrix.
grf_forest_t* grf_train_multi_regression(
    const double* data, size_t n_rows, size_t n_cols,
    const size_t* outcome_indices, size_t num_outcomes,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/// Train a multi-arm causal forest. Y columns are at outcome_indices;
/// treatment column is at treatment_index. `num_treatments` must equal the
/// number of distinct levels of W (>= 2).
grf_forest_t* grf_train_multi_causal(
    const double* data, size_t n_rows, size_t n_cols,
    const size_t* outcome_indices, size_t num_outcomes,
    size_t treatment_index,
    size_t num_treatments,
    const double* gradient_weights, size_t n_gradient_weights,
    bool stabilize_splits,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/// Train a causal survival forest. Layout: [X..., Y(time), W, D].
/// target=0 -> RMST; target=1 -> survival probability at horizon.
/// `horizon` is required.
grf_forest_t* grf_train_causal_survival(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t treatment_index, size_t censor_index,
    int target, double horizon,
    const double* failure_times, size_t n_failure_times,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/// Train a linear-model forest (conditionally linear model).
/// Y columns at outcome_indices, W regressors at `regressor_indices`.
grf_forest_t* grf_train_lm(
    const double* data, size_t n_rows, size_t n_cols,
    const size_t* outcome_indices, size_t num_outcomes,
    const size_t* regressor_indices, size_t num_regressors,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/// Train a local-linear regression forest.
grf_forest_t* grf_train_ll_regression(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index,
    double ll_split_lambda,
    bool ll_split_weight_penalty,
    const size_t* ll_split_variables, size_t n_ll_split_variables,
    size_t ll_split_cutoff,
    const double* sample_weights,
    const grf_train_opts_t* opts
);

/* ─────────────────────────── forest lifecycle ─────────────────────────── */

void grf_forest_free(grf_forest_t* forest);

/// Return the kind tag of a forest (one of: "regression", "causal", ...).
/// The pointer is to static storage and must NOT be freed.
const char* grf_forest_kind(const grf_forest_t* forest);

/// Number of trees in the forest.
size_t grf_forest_num_trees(const grf_forest_t* forest);

/// Pre-computed OOB predictions (column-major flat buffer of length
/// `n_train_rows * pred_length`), or NULL if the forest was trained with
/// compute_oob_predictions=false. Multi-output forests: pred_length is the
/// per-sample output dim.
///
/// On return, `*out_total_len` is set to the buffer length (i.e.
/// `n_train_rows * pred_length`), and `*out_pred_length` is set to the
/// per-sample output dim. Use `grf_forest_oob_num_samples` to recover the
/// sample count.
const double* grf_forest_oob_predictions(
    const grf_forest_t* forest,
    size_t* out_total_len,
    size_t* out_pred_length
);

/// Number of training samples the OOB predictions buffer was computed for.
/// Returns 0 if no OOB predictions were computed.
size_t grf_forest_oob_num_samples(const grf_forest_t* forest);

/* ─────────────────────────── prediction ─────────────────────────── */

/// Predict on new test data. `train_data` is needed for forests that use
/// train-time covariates at prediction time (regression, causal, etc).
/// `estimate_variance` requires ci_group_size >= 2 at training time.
grf_predictions_t* grf_predict(
    const grf_forest_t* forest,
    const double* train_data, size_t n_train_rows, size_t n_train_cols,
    size_t train_outcome_index,
    const double* test_data, size_t n_test_rows, size_t n_test_cols,
    bool estimate_variance,
    uint32_t num_threads
);

/// Out-of-bag prediction on the training set. Caller must pass train_data
/// matching what was used during training.
grf_predictions_t* grf_predict_oob(
    const grf_forest_t* forest,
    const double* train_data, size_t n_train_rows, size_t n_train_cols,
    size_t train_outcome_index,
    bool estimate_variance,
    uint32_t num_threads
);

void grf_predictions_free(grf_predictions_t* preds);

/* ─────────────────────────── prediction accessors ─────────────────────────── */

size_t grf_predictions_num_samples(const grf_predictions_t* preds);
size_t grf_predictions_pred_length(const grf_predictions_t* preds);

/// Predictions as a column-major matrix of shape (pred_length, num_samples).
/// Caller MUST NOT free; lifetime is tied to the prediction handle.
const double* grf_predictions_values(const grf_predictions_t* preds);

/// Variance estimates (only when estimate_variance=true), same shape as
/// values. Returns NULL if not requested.
const double* grf_predictions_variance(const grf_predictions_t* preds);

/// Debiased error (only when predict_oob was called).
const double* grf_predictions_debiased_error(const grf_predictions_t* preds);

/// Excess error (only when predict_oob was called).
const double* grf_predictions_excess_error(const grf_predictions_t* preds);

/* ─────────────────────────── serialization ─────────────────────────── */

/// Serialize a forest to a contiguous byte buffer.
/// `*out_len` receives the buffer length. The caller owns the buffer and
/// must free() it. Returns NULL on failure.
uint8_t* grf_forest_serialize(const grf_forest_t* forest, size_t* out_len);

/// Deserialize a forest from a byte buffer. Returns NULL on failure.
/// The kind tag stored in the buffer is checked against the header.
grf_forest_t* grf_forest_deserialize(const uint8_t* buf, size_t len);

/// Serialize a single tree (index-th) of a forest into a fresh byte buffer.
/// `*out_len` receives the buffer length; the caller owns it and must
/// free(). Returns NULL if index is out of range or on failure.
uint8_t* grf_forest_get_tree(const grf_forest_t* forest, size_t index, size_t* out_len);

/// Merge N forests into one (concatenate their trees). `forests` is an array
/// of `n` handles; all must share the same kind, ci_group_size and
/// num_variables. OOB predictions are dropped. Returns NULL on failure.
grf_forest_t* grf_forest_merge(const grf_forest_t* const* forests, size_t n);

/* ─────────────────────────── analysis tools ─────────────────────────── */

/// Number of split variables per tree (variable_importance / split_frequencies).
/// The flat row-major array `depths_x_features` has shape (max_depth, n_features)
/// and is freed by `grf_split_freq_free`.
grf_split_freq_t* grf_compute_split_frequencies(
    const grf_forest_t* forest, size_t max_depth
);

void grf_split_freq_free(grf_split_freq_t* sf);

/// Compute forest weights α(x) — for each test row, the per-train-row weight
/// based on co-leaf membership. Output is a dense column-major buffer of
/// shape (n_train_rows, n_test_rows), flattened. Caller frees via `free()`.
/// Returns NULL on failure.
double* grf_compute_weights(
    const grf_forest_t* forest,
    const double* train_data, size_t n_train_rows, size_t n_train_cols,
    const double* test_data, size_t n_test_rows, size_t n_test_cols,
    size_t* out_n_train, size_t* out_n_test,
    uint32_t num_threads
);

/// Out-of-bag variant of grf_compute_weights: computes α(x) using only
/// trees that did not use each training row. Returns a buffer of length
/// n_train_rows × n_train_rows (each row's weights over the OTHER rows).
double* grf_compute_weights_oob(
    const grf_forest_t* forest,
    const double* train_data, size_t n_train_rows, size_t n_train_cols,
    size_t* out_n_train,
    uint32_t num_threads
);

#ifdef __cplusplus
}
#endif

#endif // GRF_SHIM_H_