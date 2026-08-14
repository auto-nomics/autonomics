//! Verifies every `grf.*` node kind is registered and buildable in the default
//! DAG registry (via the `nodes-grf` bundle).

use std::sync::Arc;

use datafusion::prelude::SessionContext;

use dag_core::registry::NodeRegistry;

fn registry() -> NodeRegistry {
    // The datalake config reads these env vars at construction; the test
    // doesn't connect to a real catalog, so dummies are fine.
    if std::env::var("ICEBERG_REST_URI").is_err() {
        // SAFETY: single-threaded test, no other code reads these vars
        // concurrently.
        unsafe {
            std::env::set_var("ICEBERG_REST_URI", "http://localhost:8181/catalog");
            std::env::set_var("ICEBERG_S3_ACCESS_KEY_ID", "test");
            std::env::set_var("ICEBERG_S3_SECRET_ACCESS_KEY", "test");
        }
    }
    let ctx = SessionContext::new();
    data_engine::default_registry::build_default_registry(
        ctx.runtime_env(),
        None,
    )
}

/// Minimal spec per node kind (just enough required fields to deserialize and
/// build the node; no data is passed at build time).
fn spec_for(kind: &str) -> serde_json::Value {
    use serde_json::json;
    match kind {
        // Trainers
        "grf_regression_forest" => json!({"y_column_name":"y"}),
        "grf_causal_forest" => json!({"y_column_name":"y","w_column_name":"w"}),
        "grf_quantile_forest" => json!({"y_column_name":"y","quantiles":[0.5]}),
        "grf_probability_forest" => json!({"y_column_name":"y","num_classes":2}),
        "grf_survival_forest" => json!({"time_column_name":"time","censor_column_name":"censor"}),
        "grf_multi_regression_forest" => json!({"y_column_names":["y0"]}),
        "grf_instrumental_forest" => {
            json!({"y_column_name":"y","w_column_name":"w","z_column_name":"z"})
        }
        "grf_lm_forest" => json!({"y_column_names":["y0"],"w_column_names":["w0"]}),
        "grf_ll_regression_forest" => {
            json!({"y_column_name":"y","ll_split_lambda":0.1,"ll_split_weight_penalty":false,"ll_split_variables":[]})
        }
        "grf_boosted_regression_forest" => {
            json!({"y_column_name":"y","boost_trees_tune":1,"boost_error_reduction":0.0})
        }
        "grf_multi_arm_causal_forest" => json!({"y_column_names":["y0"],"w_column_name":"w"}),
        "grf_causal_survival_forest" => {
            json!({"time_column_name":"time","w_column_name":"w","censor_column_name":"censor","target":0,"horizon":1.0})
        }
        // Prediction + causal analysis
        "grf_predict_forest" => json!({}),
        "grf_average_treatment_effect" => json!({"target_sample":"all","method":"AIPW"}),
        "grf_best_linear_projection" => json!({"a_column_names":[]}),
        "grf_test_calibration" => json!({"vcov_type":"HC3"}),
        "grf_get_scores" => json!({"estimate_variance":false,"num_trees_for_weights":0}),
        // Forest analysis
        "grf_get_forest_weights" => json!({"num_threads":1,"max_depth":0}),
        "grf_split_frequencies" => json!({"max_depth":0}),
        "grf_variable_importance" => json!({"max_depth":0,"decay_exponent":0.0}),
        "grf_get_tree" => json!({"index":0}),
        "grf_merge_forests" => json!({}),
        // Data generation
        "grf_generate_causal_data" => json!({"n":100,"p":3,"seed":42}),
        _ => panic!("unknown grf kind: {kind}"),
    }
}

const GRF_KINDS: &[&str] = &[
    "grf_regression_forest",
    "grf_causal_forest",
    "grf_quantile_forest",
    "grf_probability_forest",
    "grf_survival_forest",
    "grf_multi_regression_forest",
    "grf_instrumental_forest",
    "grf_lm_forest",
    "grf_ll_regression_forest",
    "grf_boosted_regression_forest",
    "grf_multi_arm_causal_forest",
    "grf_causal_survival_forest",
    "grf_predict_forest",
    "grf_average_treatment_effect",
    "grf_best_linear_projection",
    "grf_test_calibration",
    "grf_get_scores",
    "grf_get_forest_weights",
    "grf_split_frequencies",
    "grf_variable_importance",
    "grf_get_tree",
    "grf_merge_forests",
    "grf_generate_causal_data",
];

#[test]
fn grf_nodes_are_registered_and_buildable() {
    let reg = registry();
    for kind in GRF_KINDS {
        let factory = reg
            .get_factory(kind)
            .unwrap_or_else(|e| panic!("{kind} not registered: {e}"));
        // A minimal valid spec must build a node without error.
        if reg.build_node(kind, spec_for(kind)).is_err() {
            panic!("{kind} failed to build");
        }
        // Sanity: kind matches.
        assert_eq!(factory.kind(), *kind);
    }
}
