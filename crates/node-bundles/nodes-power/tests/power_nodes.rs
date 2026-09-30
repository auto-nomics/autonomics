use arrow_array::{Float64Array, RecordBatch, UInt64Array};
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_power::common::{
    noncentral_chisq_cdf, noncentral_f_cdf, noncentral_t_cdf, output_schema,
};
use nodes_power::{correlation, counts, linear_models, means, survival_cluster};

async fn run(factory: &dyn NodeFactory, spec: serde_json::Value) -> RecordBatch {
    let session = SessionContext::new();
    let ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = factory.build(spec, ctx.clone()).expect("valid spec");
    let outputs = node
        .execute(&ctx, &[], &NodeReporter::noop())
        .await
        .expect("node executes");
    let mut batches = outputs
        .dataframe(0)
        .unwrap()
        .clone()
        .collect()
        .await
        .unwrap();
    assert_eq!(batches.len(), 1);
    batches.remove(0)
}

fn power(batch: &RecordBatch) -> f64 {
    batch
        .column_by_name("power")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap()
        .value(0)
}

fn u64_value(batch: &RecordBatch, name: &str) -> u64 {
    batch
        .column_by_name(name)
        .unwrap()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap()
        .value(0)
}

fn assert_common_schema(batch: &RecordBatch) {
    assert_eq!(batch.schema().as_ref(), output_schema().as_ref());
    assert_eq!(batch.num_rows(), 1);
}

#[tokio::test]
async fn t_test_power_and_rounded_sample_size_match_noncentral_t() {
    let direct = run(
        &means::PowerTTestNodeFactory,
        serde_json::json!({
            "solve_for": "power",
            "design": "two_sample",
            "n1": 30,
            "n2": 30,
            "effect_size": 0.5
        }),
    )
    .await;
    assert_common_schema(&direct);
    assert!((power(&direct) - 0.4778965).abs() < 1e-6);

    let sampled = run(
        &means::PowerTTestNodeFactory,
        serde_json::json!({
            "solve_for": "sample_size",
            "target_power": 0.8,
            "design": "two_sample",
            "effect_size": 0.5
        }),
    )
    .await;
    assert_eq!(u64_value(&sampled, "n1"), 64);
    assert_eq!(u64_value(&sampled, "n2"), 64);
    assert!(power(&sampled) >= 0.8);

    let one_sided = run(
        &means::PowerTTestNodeFactory,
        serde_json::json!({
            "solve_for": "power",
            "design": "one_sample",
            "alternative": "greater",
            "n": 30,
            "effect_size": 0.5
        }),
    )
    .await;
    assert!(power(&one_sided) > power(&direct));
}

#[tokio::test]
async fn proportion_and_correlation_use_declared_approximations() {
    let prop = run(
        &counts::PowerPropTestNodeFactory,
        serde_json::json!({
            "solve_for": "sample_size",
            "target_power": 0.8,
            "p1": 0.5,
            "p2": 0.6
        }),
    )
    .await;
    assert_common_schema(&prop);
    assert_eq!(u64_value(&prop, "n1"), 388);
    assert_eq!(u64_value(&prop, "n2"), 388);
    assert!(power(&prop) >= 0.8);

    let cor = run(
        &correlation::PowerCorrelationNodeFactory,
        serde_json::json!({
            "solve_for": "power",
            "n": 50,
            "correlation": 0.3
        }),
    )
    .await;
    assert!((power(&cor) - 0.5643676).abs() < 1e-7);
}

#[tokio::test]
async fn noncentral_model_nodes_agree_with_reference_cdfs() {
    let chisq = run(
        &counts::PowerChisqTestNodeFactory,
        serde_json::json!({
            "solve_for": "power",
            "n": 200,
            "probabilities": [[0.3, 0.2], [0.2, 0.3]]
        }),
    )
    .await;
    assert_common_schema(&chisq);
    assert!(power(&chisq) > 0.8);

    let anova = run(
        &linear_models::PowerAnovaNodeFactory,
        serde_json::json!({
            "solve_for": "power",
            "group_ns": [30, 30, 30],
            "means": [-0.25, 0.0, 0.25],
            "sd": 1.0
        }),
    )
    .await;
    assert!((power(&anova) - 0.380531).abs() < 3e-6);

    let regression = run(
        &linear_models::PowerRegressionNodeFactory,
        serde_json::json!({
            "solve_for": "power",
            "n": 50,
            "predictors_total": 3,
            "r_squared": 0.2
        }),
    )
    .await;
    assert!((power(&regression) - 0.8220164).abs() < 3e-6);
}

#[tokio::test]
async fn survival_and_cluster_report_design_specific_counts() {
    let survival = run(
        &survival_cluster::PowerSurvivalNodeFactory,
        serde_json::json!({
            "solve_for": "sample_size",
            "target_power": 0.8,
            "hazard_ratio": 0.7,
            "control_hazard": 0.1,
            "follow_up_duration": 5.0
        }),
    )
    .await;
    assert_common_schema(&survival);
    assert!(u64_value(&survival, "events") >= 240);
    assert!(power(&survival) >= 0.8);
    assert!(u64_value(&survival, "total_n") > u64_value(&survival, "events"));

    let cluster = run(
        &survival_cluster::PowerClusterNodeFactory,
        serde_json::json!({
            "solve_for": "sample_size",
            "target_power": 0.8,
            "outcome": "continuous",
            "mean_difference": 0.5,
            "sd": 1.0,
            "mean_cluster_size": 20.0,
            "icc": 0.05
        }),
    )
    .await;
    assert!(u64_value(&cluster, "total_clusters") > 10);
    assert!(u64_value(&cluster, "total_n") >= u64_value(&cluster, "total_clusters") * 20);
    assert!(power(&cluster) >= 0.8);
}

#[test]
fn noncentral_cdfs_match_r_reference_values() {
    assert!((noncentral_t_cdf(2.0, 20.0, 1.0).unwrap() - 0.8238733).abs() < 1e-6);
    assert!((noncentral_t_cdf(-1.0, 8.0, 2.0).unwrap() - 0.001939038).abs() < 1e-6);
    assert!((noncentral_f_cdf(2.0, 3.0, 50.0, 1.0).unwrap() - 0.7734781).abs() < 1e-6);
    assert!((noncentral_chisq_cdf(3.0, 4.0, 1.0).unwrap() - 0.3313813).abs() < 1e-6);
}

#[tokio::test]
async fn solving_for_power_rejects_target_power_input() {
    let session = SessionContext::new();
    let ctx = NodeCtx::new(session.runtime_env(), None);
    let mut node = means::PowerTTestNodeFactory
        .build(
            serde_json::json!({
                "solve_for": "power",
                "target_power": 0.8,
                "design": "one_sample",
                "n": 30,
                "effect_size": 0.5
            }),
            ctx.clone(),
        )
        .unwrap();
    let result = node.execute(&ctx, &[], &NodeReporter::noop()).await;
    assert!(result.is_err());
}
