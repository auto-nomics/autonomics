/*
 * grf_shim.cpp — implementation of grf_shim.h, the C ABI for the GRF core.
 *
 * Strategy: this file avoids Rcpp entirely. The grf core is pure C++17 +
 * Eigen + pthread, so we can construct grf::Data, grf::ForestOptions,
 * grf::ForestTrainer / grf::ForestPredictor directly and return opaque
 * handles. Internally we follow the same shape as grf's Rcpp bindings
 * (see r-package/grf/src/RegressionForestBindings.cpp), minus the Rcpp
 * converters.
 *
 * Forest serialization: we copy the layout used by RcppUtilities.cpp
 * (root_nodes, child_nodes, leaf_samples, split_vars, split_values,
 * drawn_samples, send_missing_left, pv_values, pv_num_types), prepend a
 * magic + kind tag, and write lengths explicitly so we can evolve the
 * format without breaking compatibility with old serialized forests.
 *
 * Lifetime: every grf::Forest* returned to C is owned by a heap-allocated
 * wrapper struct grf_forest_t { kind_tag, std::unique_ptr<grf::Forest> }.
 * The grf_forest_free function destructs via unique_ptr.
 */

#include "grf_shim.h"

#include <atomic>
#include <cstdint>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <sstream>
#include <string>
#include <vector>

// grf core headers — no Rcpp.
#include "commons/Data.h"
#include "commons/globals.h"
#include "forest/Forest.h"
#include "forest/ForestOptions.h"
#include "forest/ForestPredictors.h"
#include "forest/ForestTrainers.h"
#include "prediction/Prediction.h"
#include "tree/Tree.h"
#include "prediction/collector/TreeTraverser.h"
#include "analysis/SplitFrequencyComputer.h"
// Eigen sparse types for get_forest_weights.
#include <Eigen/Sparse>

namespace {

// ─────────────────────────── thread-local error reporting ───────────────────────────

thread_local std::string tls_last_error;

const char* set_error(const std::string& msg) {
    tls_last_error = msg;
    return tls_last_error.c_str();
}

const char* set_error(const char* msg) {
    tls_last_error = msg;
    return tls_last_error.c_str();
}

// ─────────────────────────── opaque wrapper structs ───────────────────────────
// (Defined in grf_shim.h; this file just uses them.)

// Helper: flatten a vector<Prediction> into a column-major buffer (one column
// per Prediction, pred_length rows). Empty vectors for variance/error/etc.
// are returned as size==0 in the wrapper.
void flatten_predictions(const std::vector<grf::Prediction>& src,
                         grf_predictions_t* dst,
                         bool capture_variance,
                         bool capture_error) {
    if (src.empty()) return;
    dst->pred_length = src[0].size();
    const size_t n = src.size();
    dst->values.assign(dst->pred_length * n, 0.0);
    if (capture_variance && src[0].contains_variance_estimates()) {
        dst->has_variance = true;
        dst->variance.assign(dst->pred_length * n, 0.0);
    }
    if (capture_error && src[0].contains_error_estimates()) {
        dst->has_error = true;
        dst->debiased_error.assign(n, 0.0);
        dst->excess_error.assign(n, 0.0);
    }
    for (size_t i = 0; i < n; ++i) {
        const auto& p = src[i].get_predictions();
        for (size_t j = 0; j < p.size(); ++j) {
            dst->values[j * n + i] = p[j];
        }
        if (dst->has_variance) {
            const auto& v = src[i].get_variance_estimates();
            for (size_t j = 0; j < v.size(); ++j) {
                dst->variance[j * n + i] = v[j];
            }
        }
        if (dst->has_error) {
            dst->debiased_error[i] = src[i].get_error_estimates()[0];
            dst->excess_error[i] = src[i].get_excess_error_estimates()[0];
        }
    }
}

// Copy just the predictions column-major buffer (used for OOB capture at
// training time, when we don't care about variance / error wrappers).
void copy_predictions_to_buffer(const std::vector<grf::Prediction>& src,
                               std::vector<double>& buf,
                               size_t& pred_length) {
    if (src.empty()) { pred_length = 0; buf.clear(); return; }
    pred_length = src[0].size();
    const size_t n = src.size();
    buf.assign(pred_length * n, 0.0);
    for (size_t i = 0; i < n; ++i) {
        const auto& p = src[i].get_predictions();
        for (size_t j = 0; j < p.size(); ++j) {
            buf[j * n + i] = p[j];
        }
    }
}

// ─────────────────────────── ForestOptions construction ───────────────────────────

grf::ForestOptions make_options(const grf_train_opts_t* opts) {
    std::vector<size_t> clusters;
    if (opts->clusters != nullptr && opts->n_clusters > 0) {
        clusters.reserve(opts->n_clusters);
        for (size_t i = 0; i < opts->n_clusters; ++i) {
            clusters.push_back(static_cast<size_t>(opts->clusters[i]));
        }
    }
    return grf::ForestOptions(
        opts->num_trees,
        opts->ci_group_size,
        opts->sample_fraction,
        opts->mtry,
        opts->min_node_size,
        opts->honesty,
        opts->honesty_fraction,
        opts->honesty_prune_leaves,
        opts->alpha,
        opts->imbalance_penalty,
        opts->num_threads,
        opts->seed,
        opts->legacy_seed,
        clusters,
        opts->samples_per_cluster);
}

// ─────────────────────────── Data construction ───────────────────────────

grf::Data make_data(const double* data, size_t n_rows, size_t n_cols) {
    if (data == nullptr) {
        throw std::runtime_error("data pointer is null");
    }
    return grf::Data(data, n_rows, n_cols);
}

// Helper: convert sample_weights (R-style length-n double vector) into the
// weight column appended to the data matrix. Returns the augmented data
// matrix as a flat column-major vector with n_cols + (use_weights?1:0)
// columns. The outcome_index / treatment_index arguments point into the
// ORIGINAL data columns, not the augmented ones — we adjust internally.
std::vector<double> maybe_augment_weights(
    const double* data, size_t n_rows, size_t n_cols,
    const double* sample_weights, size_t n_weights) {
    std::vector<double> out;
    if (sample_weights == nullptr || n_weights == 0) {
        out.assign(data, data + n_rows * n_cols);
        return out;
    }
    if (n_weights != n_rows) {
        throw std::runtime_error("sample_weights length must equal n_rows");
    }
    out.assign(data, data + n_rows * n_cols);
    out.resize(n_rows * (n_cols + 1));
    for (size_t i = 0; i < n_rows; ++i) {
        out[n_cols * n_rows + i] = sample_weights[i];
    }
    return out;
}

// ─────────────────────────── survival outcome relabeling ───────────────────────────

// R's findInterval(x, vec) with rightmost.closed = FALSE: the number of grid
// values <= x. Equivalent to std::upper_bound over a sorted unique grid.
static inline size_t find_interval(double x, const std::vector<double>& grid) {
    return static_cast<size_t>(std::upper_bound(grid.begin(), grid.end(), x) - grid.begin());
}

// Build the failure-time grid when the caller left it empty (mirrors R's
// `failure.times <- sort(unique(Y[D == 1]))`): the sorted unique event times.
static std::vector<double> compute_failure_times(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t censor_index) {
    std::set<double> times;
    for (size_t i = 0; i < n_rows; ++i) {
        double censor = data[censor_index * n_rows + i];
        if (censor > 0.0) {
            times.insert(data[outcome_index * n_rows + i]);
        }
    }
    return std::vector<double>(times.begin(), times.end());
}

// Return a column-major copy of `data` with the outcome column replaced by
// findInterval(time, grid) so the survival predictor's integer indexing
// (0..num_failures) matches the grid.
static std::vector<double> relabel_survival_outcome(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, const std::vector<double>& grid) {
    std::vector<double> out(data, data + n_rows * n_cols);
    for (size_t i = 0; i < n_rows; ++i) {
        out[outcome_index * n_rows + i] = static_cast<double>(
            find_interval(data[outcome_index * n_rows + i], grid));
    }
    return out;
}

// ─────────────────────────── runtime context setup ───────────────────────────

void configure_runtime(const grf_train_opts_t* opts) {
    grf::runtime_context.verbose_stream = opts->verbose ? &std::cerr : nullptr;
    grf::runtime_context.interrupt_handler = []() {
        // No signal forwarding by default — Rust side owns cancellation.
        // The grf core checks this periodically during tree training.
    };
}

// ─────────────────────────── serialization helpers ───────────────────────────

constexpr uint32_t GRF_FOREST_MAGIC = 0x47524646; // 'GRFF'
constexpr uint32_t GRF_FOREST_VERSION = 1;

template <typename T>
void write_value(std::vector<uint8_t>& buf, const T& v) {
    const size_t off = buf.size();
    buf.resize(off + sizeof(T));
    std::memcpy(buf.data() + off, &v, sizeof(T));
}

void write_u64(std::vector<uint8_t>& buf, uint64_t v) { write_value(buf, v); }
void write_u32(std::vector<uint8_t>& buf, uint32_t v) { write_value(buf, v); }
void write_double(std::vector<uint8_t>& buf, double v) { write_value(buf, v); }
void write_size(std::vector<uint8_t>& buf, size_t v) { write_u64(buf, v); }

void write_string(std::vector<uint8_t>& buf, const std::string& s) {
    write_size(buf, s.size());
    buf.insert(buf.end(), s.begin(), s.end());
}

template <typename T>
bool read_value(const uint8_t*& p, const uint8_t* end, T& out) {
    if (p + sizeof(T) > end) return false;
    std::memcpy(&out, p, sizeof(T));
    p += sizeof(T);
    return true;
}

bool read_u64(const uint8_t*& p, const uint8_t* end, uint64_t& v) { return read_value(p, end, v); }
bool read_u32(const uint8_t*& p, const uint8_t* end, uint32_t& v) { return read_value(p, end, v); }
bool read_double(const uint8_t*& p, const uint8_t* end, double& v) { return read_value(p, end, v); }
bool read_size(const uint8_t*& p, const uint8_t* end, size_t& v) {
    uint64_t tmp;
    if (!read_u64(p, end, tmp)) return false;
    v = static_cast<size_t>(tmp);
    return true;
}

bool read_string(const uint8_t*& p, const uint8_t* end, std::string& s) {
    size_t len;
    if (!read_size(p, end, len)) return false;
    if (p + len > end) return false;
    s.assign(reinterpret_cast<const char*>(p), len);
    p += len;
    return true;
}

template <typename T>
void append_vec(std::vector<uint8_t>& buf, const std::vector<T>& v) {
    write_size(buf, v.size());
    const auto off = buf.size();
    buf.resize(off + v.size() * sizeof(T));
    if (!v.empty()) std::memcpy(buf.data() + off, v.data(), v.size() * sizeof(T));
}

// std::vector<bool> has no .data(); we encode booleans as 1-byte values.
template <>
void append_vec<bool>(std::vector<uint8_t>& buf, const std::vector<bool>& v) {
    write_size(buf, v.size());
    const auto off = buf.size();
    buf.resize(off + v.size());
    for (size_t i = 0; i < v.size(); ++i) {
        buf[off + i] = v[i] ? 1 : 0;
    }
}

template <typename T>
bool read_vec(const uint8_t*& p, const uint8_t* end, std::vector<T>& out) {
    size_t n;
    if (!read_size(p, end, n)) return false;
    if (n > (end - p) / sizeof(T)) return false;
    out.resize(n);
    if (n > 0) std::memcpy(out.data(), p, n * sizeof(T));
    p += n * sizeof(T);
    return true;
}

template <>
bool read_vec<bool>(const uint8_t*& p, const uint8_t* end, std::vector<bool>& out) {
    size_t n;
    if (!read_size(p, end, n)) return false;
    if (n > static_cast<size_t>(end - p)) return false;
    out.resize(n);
    for (size_t i = 0; i < n; ++i) {
        out[i] = (p[i] != 0);
    }
    p += n;
    return true;
}

// Serialize a single tree using grf's internal accessors. Layout matches
// RcppUtilities::serialize_forest (which the R bindings rely on for byte-
// level reproducibility).
void serialize_tree(const grf::Tree& tree, std::vector<uint8_t>& buf) {
    write_size(buf, tree.get_root_node());
    // child_nodes is a vector of 2 vectors; flatten to {left, right}.
    const auto& cn = tree.get_child_nodes();
    std::vector<size_t> flat;
    for (size_t i = 0; i < cn.size(); ++i) {
        flat.push_back(cn[i][0]);
        flat.push_back(cn[i][1]);
    }
    append_vec(buf, flat);
    // leaf_samples is a vector<vector<size_t>>; flatten with per-leaf length prefix.
    const auto& ls = tree.get_leaf_samples();
    write_size(buf, ls.size());
    for (const auto& leaf : ls) append_vec(buf, leaf);
    append_vec(buf, tree.get_split_vars());
    append_vec(buf, tree.get_split_values());
    append_vec(buf, tree.get_drawn_samples());
    append_vec(buf, tree.get_send_missing_left());
    // PredictionValues layout: num_nodes × num_types, flattened row-major.
    const auto& pv_nested = tree.get_prediction_values().get_all_values();
    write_size(buf, pv_nested.size());  // num_nodes
    write_size(buf, tree.get_prediction_values().get_num_types());
    std::vector<double> pv_flat;
    pv_flat.reserve(pv_nested.size() * tree.get_prediction_values().get_num_types());
    for (const auto& row : pv_nested) {
        pv_flat.insert(pv_flat.end(), row.begin(), row.end());
    }
    append_vec(buf, pv_flat);
}

bool deserialize_tree(const uint8_t*& p, const uint8_t* end, std::unique_ptr<grf::Tree>& out) {
    size_t root;
    if (!read_size(p, end, root)) return false;
    std::vector<size_t> flat_children;
    if (!read_vec(p, end, flat_children)) return false;
    std::vector<std::vector<size_t>> child_nodes(flat_children.size() / 2);
    for (size_t i = 0; i < child_nodes.size(); ++i) {
        child_nodes[i] = { flat_children[2 * i], flat_children[2 * i + 1] };
    }
    size_t n_leaf_slots;
    if (!read_size(p, end, n_leaf_slots)) return false;
    std::vector<std::vector<size_t>> leaf_samples(n_leaf_slots);
    for (size_t i = 0; i < n_leaf_slots; ++i) {
        if (!read_vec(p, end, leaf_samples[i])) return false;
    }
    std::vector<size_t> split_vars;
    if (!read_vec(p, end, split_vars)) return false;
    std::vector<double> split_values;
    if (!read_vec(p, end, split_values)) return false;
    std::vector<size_t> drawn_samples;
    if (!read_vec(p, end, drawn_samples)) return false;
    std::vector<bool> send_missing_left;
    if (!read_vec(p, end, send_missing_left)) return false;
    size_t pv_num_nodes;
    size_t pv_num_types;
    if (!read_size(p, end, pv_num_nodes)) return false;
    if (!read_size(p, end, pv_num_types)) return false;
    std::vector<double> pv_values_flat;
    if (!read_vec(p, end, pv_values_flat)) return false;
    // Reshape flat (num_nodes × num_types) into nested [num_nodes][num_types].
    std::vector<std::vector<double>> pv_values(pv_num_nodes);
    if (pv_num_nodes > 0 && pv_num_types > 0) {
        for (size_t i = 0; i < pv_num_nodes; ++i) {
            pv_values[i].resize(pv_num_types);
            for (size_t j = 0; j < pv_num_types; ++j) {
                pv_values[i][j] = pv_values_flat[i * pv_num_types + j];
            }
        }
    }
    out.reset(new grf::Tree(
        root, child_nodes, leaf_samples, split_vars, split_values,
        drawn_samples, send_missing_left,
        grf::PredictionValues(pv_values, pv_num_types)));
    return true;
}

} // anonymous namespace

// ════════════════════════════════ C ABI entry points ════════════════════════════════

extern "C" {

const char* grf_last_error(void) { return tls_last_error.c_str(); }
void grf_clear_error(void) { tls_last_error.clear(); }

// ──── training ────

grf_forest_t* grf_train_regression(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(outcome_index);
        if (sample_weights) d.set_weight_index(n_cols); // last column is weights
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "regression";
        auto trainer = grf::regression_trainer();
        auto forest = trainer.train(d, options);

        std::vector<grf::Prediction> oob;
        if (opts->compute_oob_predictions) {
            grf::runtime_context.verbose_stream = nullptr;
            auto predictor = grf::regression_predictor(opts->num_threads);
            oob = predictor.predict_oob(forest, d, false);
        }

        auto* h = new grf_forest_t;
        h->kind = "regression";
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        if (!oob.empty()) {
            copy_predictions_to_buffer(oob, h->oob_predictions, h->oob_pred_length);
        }
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

// Placeholder — replaced by the helper below in the actual implementation.
// We keep this slot reserved to avoid reordering the API mid-development.
grf_forest_t* grf_train_causal_STUB(const double*, size_t, size_t, size_t, size_t,
                                     const double*, bool, const grf_train_opts_t*) {
    return nullptr;
}

// (Signatures below must match grf_shim.h. The header declares
//  `const double* sample_weights` between the data index args and `bool
//  stabilize_splits` for causal/instrumental/multi_causal, and before
//  `const grf_train_opts_t*` for the rest. The implementations below have
//  already been written to match; the only edit needed here was removing
//  this stub after the real implementations were added.)

// ──── additional forest trainers ────
// (All variants share the same skeleton: configure_runtime → augment weights
// → build Data + outcome/treatment/censor indices → make ForestOptions →
// build the grf trainer → optionally OOB → wrap into grf_forest_t.)
//
// We only implement trainers that are not yet covered above; the regressions
// forest is the canonical reference, and the others follow the same shape.
//
// Implemented below in trainer order: causal, instrumental, quantile,
// probability, survival, multi_regression, multi_causal, causal_survival,
// lm, ll_regression.

grf_forest_t* grf_train_causal(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t treatment_index,
    const double* sample_weights,
    bool stabilize_splits,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(outcome_index);
        d.set_treatment_index(treatment_index);
        d.set_instrument_index(treatment_index); // causal = instrumental with Z = W
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "causal";
        // R's causal_forest is instrumental_trainer with reduced_form_weight = 0
        // and instrument = treatment. The R-learner first stage (Y.hat, W.hat)
        // is computed at a higher layer in the Rust DAG node; we train on
        // (Y - Y.hat, W - W.hat).
        auto trainer = grf::instrumental_trainer(0.0, stabilize_splits);
        auto forest = trainer.train(d, options);

        std::vector<grf::Prediction> oob;
        if (opts->compute_oob_predictions) {
            grf::runtime_context.verbose_stream = nullptr;
            auto predictor = grf::instrumental_predictor(opts->num_threads);
            oob = predictor.predict_oob(forest, d, false);
        }
        auto* h = new grf_forest_t;
        h->kind = "causal";
        h->treatment_index = treatment_index;
        h->stabilize_splits = stabilize_splits;
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        if (!oob.empty()) {
            copy_predictions_to_buffer(oob, h->oob_predictions, h->oob_pred_length);
        }
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_instrumental(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t treatment_index, size_t instrument_index,
    double reduced_form_weight,
    bool stabilize_splits,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(outcome_index);
        d.set_treatment_index(treatment_index);
        d.set_instrument_index(instrument_index);
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "instrumental";
        auto trainer = grf::instrumental_trainer(reduced_form_weight, stabilize_splits);
        auto forest = trainer.train(d, options);

        std::vector<grf::Prediction> oob;
        if (opts->compute_oob_predictions) {
            grf::runtime_context.verbose_stream = nullptr;
            auto predictor = grf::instrumental_predictor(opts->num_threads);
            oob = predictor.predict_oob(forest, d, false);
        }
        auto* h = new grf_forest_t;
        h->kind = "instrumental";
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        if (!oob.empty()) {
            copy_predictions_to_buffer(oob, h->oob_predictions, h->oob_pred_length);
        }
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_quantile(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index,
    const double* quantiles, size_t n_quantiles,
    bool regression_splitting,
    const grf_train_opts_t* opts
) {
    (void)regression_splitting; // grf quantile trainer uses its own split criterion
    try {
        configure_runtime(opts);
        std::vector<double> q(quantiles, quantiles + n_quantiles);
        // Quantile forest does not support sample_weights in grf.
        grf::Data d(data, n_rows, n_cols);
        d.set_outcome_index(outcome_index);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "quantile";
        auto trainer = grf::quantile_trainer(q);
        auto forest = trainer.train(d, options);
        auto* h = new grf_forest_t;
        h->kind = "quantile";
        h->quantiles = q;
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        // quantile_trainer has no prediction_strategy; OOB skipped.
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_probability(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index,
    size_t num_classes,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(outcome_index);
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "probability";
        auto trainer = grf::probability_trainer(num_classes);
        auto forest = trainer.train(d, options);

        std::vector<grf::Prediction> oob;
        if (opts->compute_oob_predictions) {
            grf::runtime_context.verbose_stream = nullptr;
            auto predictor = grf::probability_predictor(opts->num_threads, num_classes);
            oob = predictor.predict_oob(forest, d, false);
        }
        auto* h = new grf_forest_t;
        h->kind = "probability";
        h->num_classes = num_classes;
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        if (!oob.empty()) {
            copy_predictions_to_buffer(oob, h->oob_predictions, h->oob_pred_length);
        }
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_survival(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t censor_index,
    const double* failure_times, size_t n_failure_times,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        std::vector<double> ft(failure_times, failure_times + n_failure_times);
        if (ft.empty()) {
            // Mirror R: `failure.times <- sort(unique(Y[D == 1]))`.
            ft = compute_failure_times(data, n_rows, n_cols, outcome_index, censor_index);
        }
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        // Relabel the outcome to findInterval(time, grid) so the survival
        // predictor's integer indexing (0..num_failures) matches the grid.
        std::vector<double> relabeled = relabel_survival_outcome(
            weights_aug.data(), n_rows, sample_weights ? n_cols + 1 : n_cols,
            outcome_index, ft);
        grf::Data d(relabeled.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(outcome_index);
        d.set_censor_index(censor_index);
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "survival";
        auto trainer = grf::survival_trainer(/*fast_logrank=*/false);
        auto forest = trainer.train(d, options);

        std::vector<grf::Prediction> oob;
        if (opts->compute_oob_predictions && !ft.empty()) {
            grf::runtime_context.verbose_stream = nullptr;
            auto predictor = grf::survival_predictor(opts->num_threads, ft.size(), 0);
            oob = predictor.predict_oob(forest, d, false);
        }
        auto* h = new grf_forest_t;
        h->kind = "survival";
        h->num_failures = ft.size();
        h->survival_prediction_type = 0;
        h->failure_times = ft;
        h->censor_index = censor_index;
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        if (!oob.empty()) {
            copy_predictions_to_buffer(oob, h->oob_predictions, h->oob_pred_length);
        }
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_multi_regression(
    const double* data, size_t n_rows, size_t n_cols,
    const size_t* outcome_indices, size_t num_outcomes,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        std::vector<size_t> idx(outcome_indices, outcome_indices + num_outcomes);
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(idx);
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "multi_regression";
        auto trainer = grf::multi_regression_trainer(num_outcomes);
        auto forest = trainer.train(d, options);

        std::vector<grf::Prediction> oob;
        if (opts->compute_oob_predictions) {
            grf::runtime_context.verbose_stream = nullptr;
            auto predictor = grf::multi_regression_predictor(opts->num_threads, num_outcomes);
            oob = predictor.predict_oob(forest, d, false);
        }
        auto* h = new grf_forest_t;
        h->kind = "multi_regression";
        h->num_outcomes = num_outcomes;
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        if (!oob.empty()) {
            copy_predictions_to_buffer(oob, h->oob_predictions, h->oob_pred_length);
        }
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_multi_causal(
    const double* data, size_t n_rows, size_t n_cols,
    const size_t* outcome_indices, size_t num_outcomes,
    size_t treatment_index,
    size_t num_treatments,
    const double* gradient_weights, size_t n_gradient_weights,
    bool stabilize_splits,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        std::vector<size_t> y_idx(outcome_indices, outcome_indices + num_outcomes);
        std::vector<double> gw;
        if (gradient_weights && n_gradient_weights > 0) {
            gw.assign(gradient_weights, gradient_weights + n_gradient_weights);
        }
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(y_idx);
        d.set_treatment_index(treatment_index);
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "multi_causal";
        auto trainer = grf::multi_causal_trainer(num_treatments, num_outcomes,
                                                 stabilize_splits, gw);
        auto forest = trainer.train(d, options);

        std::vector<grf::Prediction> oob;
        if (opts->compute_oob_predictions) {
            grf::runtime_context.verbose_stream = nullptr;
            auto predictor = grf::multi_causal_predictor(opts->num_threads,
                                                        num_treatments, num_outcomes);
            oob = predictor.predict_oob(forest, d, false);
        }
        auto* h = new grf_forest_t;
        h->kind = "multi_causal";
        h->num_treatments = num_treatments;
        h->num_outcomes = num_outcomes;
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        if (!oob.empty()) {
            copy_predictions_to_buffer(oob, h->oob_predictions, h->oob_pred_length);
        }
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_causal_survival(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, size_t treatment_index, size_t censor_index,
    int target, double horizon,
    const double* failure_times, size_t n_failure_times,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    (void)target; (void)horizon; (void)failure_times; (void)n_failure_times;
    try {
        configure_runtime(opts);
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(outcome_index);
        d.set_treatment_index(treatment_index);
        d.set_censor_index(censor_index);
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "causal_survival";
        auto trainer = grf::causal_survival_trainer(/*stabilize_splits=*/true);
        auto forest = trainer.train(d, options);

        auto* h = new grf_forest_t;
        h->kind = "causal_survival";
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        // OOB capture for causal_survival needs (target, horizon, failure_times);
        // Rust DAG layer can run predict_oob with those args.
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_lm(
    const double* data, size_t n_rows, size_t n_cols,
    const size_t* outcome_indices, size_t num_outcomes,
    const size_t* regressor_indices, size_t num_regressors,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        std::vector<size_t> y_idx(outcome_indices, outcome_indices + num_outcomes);
        std::vector<size_t> w_idx(regressor_indices, regressor_indices + num_regressors);
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(y_idx);
        d.set_treatment_index(w_idx);
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "lm";
        // LM is a multi_causal trainer with stabilize=true and W as treatment.
        auto trainer = grf::multi_causal_trainer(num_regressors, num_outcomes,
                                                 /*stabilize_splits=*/true);
        auto forest = trainer.train(d, options);

        std::vector<grf::Prediction> oob;
        if (opts->compute_oob_predictions) {
            grf::runtime_context.verbose_stream = nullptr;
            auto predictor = grf::multi_causal_predictor(opts->num_threads,
                                                        num_regressors, num_outcomes);
            oob = predictor.predict_oob(forest, d, false);
        }
        auto* h = new grf_forest_t;
        h->kind = "lm";
        h->num_treatments = num_regressors;
        h->num_outcomes = num_outcomes;
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        if (!oob.empty()) {
            copy_predictions_to_buffer(oob, h->oob_predictions, h->oob_pred_length);
        }
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_train_ll_regression(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index,
    double ll_split_lambda,
    bool ll_split_weight_penalty,
    const size_t* ll_split_variables, size_t n_ll_split_variables,
    size_t ll_split_cutoff,
    const double* sample_weights,
    const grf_train_opts_t* opts
) {
    try {
        configure_runtime(opts);
        std::vector<size_t> sv(ll_split_variables,
                               ll_split_variables + n_ll_split_variables);
        std::vector<double> overall_beta(n_cols, 0.0);
        auto weights_aug = maybe_augment_weights(data, n_rows, n_cols, sample_weights,
                                                 sample_weights ? n_rows : 0);
        grf::Data d(weights_aug.data(), n_rows,
                    sample_weights ? n_cols + 1 : n_cols);
        d.set_outcome_index(outcome_index);
        if (sample_weights) d.set_weight_index(n_cols);
        auto options = make_options(opts);
        grf::runtime_context.forest_name = "ll_regression";
        auto trainer = grf::ll_regression_trainer(ll_split_lambda,
                                                  ll_split_weight_penalty,
                                                  overall_beta,
                                                  ll_split_cutoff, sv);
        auto forest = trainer.train(d, options);

        auto* h = new grf_forest_t;
        h->kind = "ll_regression";
        h->forest = std::make_unique<grf::Forest>(std::move(forest));
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

// ──── forest metadata ────

void grf_forest_free(grf_forest_t* forest) { delete forest; }

const char* grf_forest_kind(const grf_forest_t* forest) {
    return forest ? forest->kind.c_str() : "";
}

size_t grf_forest_num_trees(const grf_forest_t* forest) {
    return forest && forest->forest ? forest->forest->get_trees().size() : 0;
}

const double* grf_forest_oob_predictions(
    const grf_forest_t* forest,
    size_t* out_total_len,
    size_t* out_pred_length
) {
    if (!forest || forest->oob_predictions.empty()) {
        if (out_total_len) *out_total_len = 0;
        if (out_pred_length) *out_pred_length = 0;
        return nullptr;
    }
    if (out_total_len) *out_total_len = forest->oob_predictions.size();
    if (out_pred_length) *out_pred_length = forest->oob_pred_length;
    return forest->oob_predictions.data();
}

size_t grf_forest_oob_num_samples(const grf_forest_t* forest) {
    if (!forest || forest->oob_predictions.empty() || forest->oob_pred_length == 0) {
        return 0;
    }
    return forest->oob_predictions.size() / forest->oob_pred_length;
}

// ──── prediction ────
//
// Dispatch on forest->kind to construct the correct ForestPredictor. Each
// forest type needs its own predictor constructor parameters (quantiles,
// num_classes, num_outcomes, etc.) which were stored in the forest handle
// at training time.

static grf::ForestPredictor make_predictor(const grf_forest_t* forest, uint num_threads) {
    const std::string& kind = forest->kind;
    if (kind == "regression") {
        return grf::regression_predictor(num_threads);
    } else if (kind == "causal" || kind == "instrumental") {
        return grf::instrumental_predictor(num_threads);
    } else if (kind == "quantile") {
        return grf::quantile_predictor(num_threads, forest->quantiles);
    } else if (kind == "probability") {
        return grf::probability_predictor(num_threads, forest->num_classes);
    } else if (kind == "survival") {
        return grf::survival_predictor(num_threads, forest->num_failures,
                                       forest->survival_prediction_type);
    } else if (kind == "multi_regression") {
        return grf::multi_regression_predictor(num_threads, forest->num_outcomes);
    } else if (kind == "multi_causal" || kind == "lm") {
        return grf::multi_causal_predictor(num_threads, forest->num_treatments,
                                           forest->num_outcomes);
    } else if (kind == "causal_survival") {
        return grf::causal_survival_predictor(num_threads);
    } else {
        // Default to regression (covers ll_regression etc.)
        return grf::regression_predictor(num_threads);
    }
}

grf_predictions_t* grf_predict(
    const grf_forest_t* forest,
    const double* train_data, size_t n_train_rows, size_t n_train_cols,
    size_t train_outcome_index,
    const double* test_data, size_t n_test_rows, size_t n_test_cols,
    bool estimate_variance,
    uint32_t num_threads
) {
    try {
        if (!forest || !forest->forest) {
            set_error("null forest handle");
            return nullptr;
        }
        // For survival forests, relabel the outcome to the failure-time grid
        // (same transform applied at training time) and set the censor index —
        // Data::is_failure reads censor_index and would throw otherwise.
        std::vector<double> relabeled;
        const double* effective_train = train_data;
        if (forest->kind == "survival" && !forest->failure_times.empty()) {
            relabeled = relabel_survival_outcome(
                train_data, n_train_rows, n_train_cols,
                train_outcome_index, forest->failure_times);
            effective_train = relabeled.data();
        }
        grf::Data train(effective_train, n_train_rows, n_train_cols);
        train.set_outcome_index(train_outcome_index);
        if (forest->kind == "survival") {
            train.set_censor_index(forest->censor_index);
        }
        if (forest->kind == "causal" || forest->kind == "instrumental") {
            train.set_treatment_index(forest->treatment_index);
            train.set_instrument_index(forest->treatment_index);
        }
        grf::Data test(test_data, n_test_rows, n_test_cols);
        grf::ForestPredictor predictor = make_predictor(forest, num_threads);
        auto preds = predictor.predict(*forest->forest, train, test, estimate_variance);
        auto* h = new grf_predictions_t;
        flatten_predictions(preds, h, estimate_variance, false);
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_predictions_t* grf_predict_oob(
    const grf_forest_t* forest,
    const double* train_data, size_t n_train_rows, size_t n_train_cols,
    size_t train_outcome_index,
    bool estimate_variance,
    uint32_t num_threads
) {
    try {
        if (!forest || !forest->forest) {
            set_error("null forest handle");
            return nullptr;
        }
        std::vector<double> relabeled;
        const double* effective_train = train_data;
        if (forest->kind == "survival" && !forest->failure_times.empty()) {
            relabeled = relabel_survival_outcome(
                train_data, n_train_rows, n_train_cols,
                train_outcome_index, forest->failure_times);
            effective_train = relabeled.data();
        }
        grf::Data train(effective_train, n_train_rows, n_train_cols);
        train.set_outcome_index(train_outcome_index);
        if (forest->kind == "survival") {
            train.set_censor_index(forest->censor_index);
        }
        if (forest->kind == "causal" || forest->kind == "instrumental") {
            train.set_treatment_index(forest->treatment_index);
            train.set_instrument_index(forest->treatment_index);
        }
        grf::ForestPredictor predictor = make_predictor(forest, num_threads);
        auto preds = predictor.predict_oob(*forest->forest, train, estimate_variance);
        auto* h = new grf_predictions_t;
        flatten_predictions(preds, h, estimate_variance, true);
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

void grf_predictions_free(grf_predictions_t* preds) { delete preds; }

size_t grf_predictions_num_samples(const grf_predictions_t* p) {
    if (!p || p->pred_length == 0) return 0;
    return p->values.size() / p->pred_length;
}

size_t grf_predictions_pred_length(const grf_predictions_t* p) {
    return p ? p->pred_length : 0;
}

const double* grf_predictions_values(const grf_predictions_t* p) {
    return p ? p->values.data() : nullptr;
}

const double* grf_predictions_variance(const grf_predictions_t* p) {
    return (p && p->has_variance) ? p->variance.data() : nullptr;
}

const double* grf_predictions_debiased_error(const grf_predictions_t* p) {
    return (p && p->has_error) ? p->debiased_error.data() : nullptr;
}

const double* grf_predictions_excess_error(const grf_predictions_t* p) {
    return (p && p->has_error) ? p->excess_error.data() : nullptr;
}

// ──── serialization ────

uint8_t* grf_forest_serialize(const grf_forest_t* forest, size_t* out_len) {
    if (!forest || !forest->forest) {
        set_error("null forest handle");
        return nullptr;
    }
    try {
        std::vector<uint8_t> buf;
        write_u32(buf, GRF_FOREST_MAGIC);
        write_u32(buf, GRF_FOREST_VERSION);
        write_string(buf, forest->kind);
        write_size(buf, forest->forest->get_ci_group_size());
        write_size(buf, forest->forest->get_num_variables());
        const auto& trees = forest->forest->get_trees();
        write_size(buf, trees.size());
        for (const auto& tree : trees) serialize_tree(*tree, buf);
        // OOB predictions.
        write_size(buf, forest->oob_pred_length);
        append_vec(buf, forest->oob_predictions);
        // Survival forest: failure-time grid + censor column index, so a
        // round-tripped forest can still relabel/predict identically.
        append_vec(buf, forest->failure_times);
        write_size(buf, forest->censor_index);
        // Causal forest: treatment column index (instrument = treatment).
        write_size(buf, forest->treatment_index);

        auto* out = static_cast<uint8_t*>(std::malloc(buf.size()));
        if (!out) { set_error("malloc failed"); return nullptr; }
        std::memcpy(out, buf.data(), buf.size());
        if (out_len) *out_len = buf.size();
        return out;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_forest_deserialize(const uint8_t* buf, size_t len) {
    try {
        const uint8_t* p = buf;
        const uint8_t* end = buf + len;
        uint32_t magic;
        if (!read_u32(p, end, magic) || magic != GRF_FOREST_MAGIC) {
            set_error("bad magic in serialized forest");
            return nullptr;
        }
        uint32_t version;
        if (!read_u32(p, end, version) || version != GRF_FOREST_VERSION) {
            set_error("unsupported forest format version");
            return nullptr;
        }
        std::string kind;
        if (!read_string(p, end, kind)) { set_error("missing kind"); return nullptr; }
        size_t ci_group_size, num_variables, num_trees;
        if (!read_size(p, end, ci_group_size) ||
            !read_size(p, end, num_variables) ||
            !read_size(p, end, num_trees)) {
            set_error("missing forest header fields");
            return nullptr;
        }
        std::vector<std::unique_ptr<grf::Tree>> trees;
        trees.reserve(num_trees);
        for (size_t i = 0; i < num_trees; ++i) {
            std::unique_ptr<grf::Tree> tree;
            if (!deserialize_tree(p, end, tree)) {
                set_error("tree deserialization failed");
                return nullptr;
            }
            trees.push_back(std::move(tree));
        }
        size_t oob_pred_length;
        std::vector<double> oob_predictions;
        if (!read_size(p, end, oob_pred_length) ||
            !read_vec(p, end, oob_predictions)) {
            set_error("oob predictions malformed");
            return nullptr;
        }
        // Survival grid + censor index (only present in v>=current blobs; the
        // reader treats a clean EOF as "not stored" to stay forward-tolerant).
        std::vector<double> failure_times;
        size_t censor_index = 0;
        size_t treatment_index = 0;
        if (read_vec(p, end, failure_times)) {
            if (!read_size(p, end, censor_index)) {
                set_error("survival grid malformed");
                return nullptr;
            }
            // Causal forest treatment_index (may not be present in older blobs).
            if (!read_size(p, end, treatment_index)) {
                treatment_index = 0; // tolerate older blobs
            }
        }

        auto* h = new grf_forest_t;
        h->kind = kind;
        h->oob_pred_length = oob_pred_length;
        h->oob_predictions = std::move(oob_predictions);
        h->failure_times = std::move(failure_times);
        h->censor_index = censor_index;
        h->treatment_index = treatment_index;
        // grf::Forest's ctor takes a non-const lvalue ref to the trees vector;
        // bind to a named local first.
        std::vector<std::unique_ptr<grf::Tree>> trees_lvalue = std::move(trees);
        h->forest = std::make_unique<grf::Forest>(
            trees_lvalue, num_variables, ci_group_size);
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

// ──── per-tree extraction & merge ────

uint8_t* grf_forest_get_tree(const grf_forest_t* forest, size_t index, size_t* out_len) {
    if (!forest || !forest->forest) {
        set_error("null forest handle");
        return nullptr;
    }
    try {
        const auto& trees = forest->forest->get_trees();
        if (index >= trees.size()) {
            set_error("tree index out of range");
            return nullptr;
        }
        std::vector<uint8_t> buf;
        serialize_tree(*trees[index], buf);
        auto* out = static_cast<uint8_t*>(std::malloc(buf.size()));
        if (!out) { set_error("malloc failed"); return nullptr; }
        std::memcpy(out, buf.data(), buf.size());
        if (out_len) *out_len = buf.size();
        return out;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

grf_forest_t* grf_forest_merge(const grf_forest_t* const* forests, size_t n) {
    if (!forests || n == 0) {
        set_error("no forests to merge");
        return nullptr;
    }
    try {
        // Deep-copy every tree via a serialize/deserialize round-trip so we
        // never mutate the caller's handles (Forest/Tree are move-only and
        // grf::Forest::merge would consume them). Concatenate into one vector.
        std::vector<std::unique_ptr<grf::Tree>> all_trees;
        size_t num_variables = 0;
        size_t ci_group_size = 0;
        for (size_t i = 0; i < n; ++i) {
            if (!forests[i] || !forests[i]->forest) {
                set_error("null forest in merge list");
                return nullptr;
            }
            if (i == 0) {
                num_variables = forests[0]->forest->get_num_variables();
                ci_group_size = forests[0]->forest->get_ci_group_size();
            } else {
                if (forests[i]->kind != forests[0]->kind) {
                    set_error("cannot merge forests of different kinds");
                    return nullptr;
                }
                if (forests[i]->forest->get_ci_group_size() != ci_group_size) {
                    set_error("all forests being merged must have the same ci_group_size");
                    return nullptr;
                }
            }
            const auto& trees = forests[i]->forest->get_trees();
            for (const auto& src : trees) {
                std::vector<uint8_t> tmp;
                serialize_tree(*src, tmp);
                const uint8_t* p = tmp.data();
                const uint8_t* end = p + tmp.size();
                std::unique_ptr<grf::Tree> tree;
                if (!deserialize_tree(p, end, tree)) {
                    set_error("tree round-trip failed during merge");
                    return nullptr;
                }
                all_trees.push_back(std::move(tree));
            }
        }

        auto* h = new grf_forest_t;
        h->kind = forests[0]->kind;
        h->oob_pred_length = 0;
        h->oob_predictions.clear();
        std::vector<std::unique_ptr<grf::Tree>> trees_lvalue = std::move(all_trees);
        h->forest = std::make_unique<grf::Forest>(
            trees_lvalue, num_variables, ci_group_size);
        return h;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

// ──── analysis: split frequencies ────

grf_split_freq_t* grf_compute_split_frequencies(
    const grf_forest_t* forest, size_t max_depth
) {
    if (!forest || !forest->forest) { set_error("null forest"); return nullptr; }
    try {
        grf::SplitFrequencyComputer comp;
        auto raw = comp.compute(*forest->forest, max_depth);
        // raw: depth × num_variables
        auto* out = new grf_split_freq_t;
        out->num_trees = forest->forest->get_trees().size();
        out->max_depth = raw.size();
        out->n_features = raw.empty() ? 0 : raw[0].size();
        const size_t total = out->max_depth * out->n_features;
        out->depths_x_features = new uint64_t[total];
        for (size_t d = 0; d < raw.size(); ++d) {
            for (size_t f = 0; f < raw[d].size(); ++f) {
                out->depths_x_features[d * out->n_features + f] = raw[d][f];
            }
        }
        return out;
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

void grf_split_freq_free(grf_split_freq_t* sf) {
    if (!sf) return;
    delete[] sf->depths_x_features;
    delete sf;
}

// ──── forest weights (for get_forest_weights) ────
//
// Replicates grf R's compute_sample_weights: for each query sample,
// returns the per-train-row weight based on how often each train row
// co-occupies a leaf with the query row across the forest's trees.

static double* flatten_sparse_to_dense_column_major(
    const Eigen::SparseMatrix<double>& sm, size_t n_rows, size_t n_cols
) {
    double* buf = static_cast<double*>(std::malloc(n_rows * n_cols * sizeof(double)));
    if (!buf) return nullptr;
    std::memset(buf, 0, n_rows * n_cols * sizeof(double));
    for (int k = 0; k < sm.outerSize(); ++k) {
        for (Eigen::SparseMatrix<double>::InnerIterator it(sm, k); it; ++it) {
            // Eigen sparse column-major: outer = col, inner = row.
            buf[k * n_rows + it.row()] = it.value();
        }
    }
    return buf;
}

double* grf_compute_weights(
    const grf_forest_t* forest,
    const double* train_data, size_t n_train_rows, size_t n_train_cols,
    const double* test_data, size_t n_test_rows, size_t n_test_cols,
    size_t* out_n_train, size_t* out_n_test,
    uint32_t num_threads
) {
    if (!forest || !forest->forest) { set_error("null forest"); return nullptr; }
    try {
        grf::Data train(train_data, n_train_rows, n_train_cols);
        grf::Data test(test_data, n_test_rows, n_test_cols);
        grf::TreeTraverser traverser(num_threads);
        auto leaf_nodes = traverser.get_leaf_nodes(*forest->forest, test, false);
        auto valid_trees = traverser.get_valid_trees_by_sample(*forest->forest, test, false);

        size_t n_neighbors = n_train_rows;
        Eigen::SparseMatrix<double> result(n_test_rows, n_neighbors);
        std::vector<Eigen::Triplet<double>> triplets;
        triplets.reserve(n_neighbors);

        const auto& trees = forest->forest->get_trees();
        size_t num_trees = trees.size();
        for (size_t s = 0; s < n_test_rows; ++s) {
            std::vector<std::pair<size_t, size_t>> train_leaf_counts;
            for (size_t t = 0; t < num_trees; ++t) {
                if (!valid_trees[s].empty() && !valid_trees[s][t]) continue;
                size_t leaf = leaf_nodes[s][t];
                if (leaf == 0) continue;
                const auto& leaf_samples = trees[t]->get_leaf_samples();
                if (leaf >= leaf_samples.size()) continue;
                for (size_t tr : leaf_samples[leaf]) {
                    train_leaf_counts.push_back({tr, 1});
                }
            }
            std::sort(train_leaf_counts.begin(), train_leaf_counts.end());
            for (size_t i = 0; i < train_leaf_counts.size(); ) {
                size_t j = i;
                while (j < train_leaf_counts.size()
                       && train_leaf_counts[j].first == train_leaf_counts[i].first) ++j;
                triplets.emplace_back(s, train_leaf_counts[i].first,
                                       double(j - i) / double(num_trees));
                i = j;
            }
        }
        result.setFromTriplets(triplets.data(), triplets.data() + triplets.size());

        if (out_n_train) *out_n_train = n_train_rows;
        if (out_n_test) *out_n_test = n_test_rows;
        return flatten_sparse_to_dense_column_major(result, n_test_rows, n_train_rows);
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

double* grf_compute_weights_oob(
    const grf_forest_t* forest,
    const double* train_data, size_t n_train_rows, size_t n_train_cols,
    size_t* out_n_train,
    uint32_t num_threads
) {
    if (!forest || !forest->forest) { set_error("null forest"); return nullptr; }
    try {
        grf::Data train(train_data, n_train_rows, n_train_cols);
        grf::TreeTraverser traverser(num_threads);
        auto leaf_nodes = traverser.get_leaf_nodes(*forest->forest, train, true);
        auto valid_trees = traverser.get_valid_trees_by_sample(*forest->forest, train, true);

        size_t n_neighbors = n_train_rows;
        Eigen::SparseMatrix<double> result(n_train_rows, n_neighbors);
        std::vector<Eigen::Triplet<double>> triplets;
        triplets.reserve(n_neighbors);

        const auto& trees = forest->forest->get_trees();
        size_t num_trees = trees.size();
        for (size_t s = 0; s < n_train_rows; ++s) {
            std::vector<std::pair<size_t, size_t>> train_leaf_counts;
            for (size_t t = 0; t < num_trees; ++t) {
                if (!valid_trees[s].empty() && !valid_trees[s][t]) continue;
                size_t leaf = leaf_nodes[s][t];
                if (leaf == 0) continue;
                const auto& leaf_samples = trees[t]->get_leaf_samples();
                if (leaf >= leaf_samples.size()) continue;
                for (size_t tr : leaf_samples[leaf]) {
                    if (tr == s) continue;
                    train_leaf_counts.push_back({tr, 1});
                }
            }
            std::sort(train_leaf_counts.begin(), train_leaf_counts.end());
            for (size_t i = 0; i < train_leaf_counts.size(); ) {
                size_t j = i;
                while (j < train_leaf_counts.size()
                       && train_leaf_counts[j].first == train_leaf_counts[i].first) ++j;
                triplets.emplace_back(s, train_leaf_counts[i].first,
                                       double(j - i) / double(num_trees));
                i = j;
            }
        }
        result.setFromTriplets(triplets.data(), triplets.data() + triplets.size());

        if (out_n_train) *out_n_train = n_train_rows;
        return flatten_sparse_to_dense_column_major(result, n_train_rows, n_train_rows);
    } catch (const std::exception& e) {
        set_error(e.what());
        return nullptr;
    }
}

} // extern "C"