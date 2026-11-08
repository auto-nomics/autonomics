//! Live tests for the Open Targets DAG source nodes.
//!
//! These hit the real Open Targets API and are `#[ignore]` by default:
//!
//! ```sh
//! cargo test -p data-engine --test opentargets_node_live -- --ignored
//! ```

use data_engine::node_registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_io::source_opentargets::{
    OpentargetsAssociationsNodeFactory, OpentargetsSearchNodeFactory,
};
use std::sync::Arc;

fn node_ctx() -> NodeCtx {
    NodeCtx::new(SessionContext::new().runtime_env(), None)
}

/// Build the full registry and return a built node by kind + spec.
fn build_node(kind: &str, spec: serde_json::Value) -> Box<dyn data_engine::dag::DagNode> {
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        std::sync::Arc::new(dag_core::BundleRegistry::new()),
    );
    registry.build_node(kind, spec).expect("build_node failed")
}

#[test]
fn factories_are_registered() {
    // The two factories must be reachable through the registry (kind-agnostic
    // tool layer depends on this).
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        std::sync::Arc::new(dag_core::BundleRegistry::new()),
    );
    let kinds: Vec<_> = registry.list_nodes().into_iter().map(|n| n.kind).collect();
    assert!(kinds.contains(&"source_opentargets_associations".to_string()));
    assert!(kinds.contains(&"source_opentargets_search".to_string()));
    // Sanity: factory kind strings match.
    assert_eq!(
        OpentargetsAssociationsNodeFactory {}.kind(),
        "source_opentargets_associations"
    );
    assert_eq!(
        OpentargetsSearchNodeFactory {}.kind(),
        "source_opentargets_search"
    );
}

#[test]
fn build_node_smoke() {
    // No network: just confirms the spec deserializes and the node builds.
    let node = build_node(
        "source_opentargets_associations",
        serde_json::json!({"id": "ENSG00000012048", "fetch_all": false, "size": 1}),
    );
    assert_eq!(node.kind(), "source_opentargets_associations");
    let node = build_node(
        "source_opentargets_search",
        serde_json::json!({"query": "BRCA1"}),
    );
    assert_eq!(node.kind(), "source_opentargets_search");
    let node = build_node(
        "opentargets_genetic",
        serde_json::json!({"id_kind": "study", "id_column": "study_id"}),
    );
    assert_eq!(node.kind(), "opentargets_genetic");
}

async fn run_node(
    kind: &str,
    spec: serde_json::Value,
) -> datafusion::common::Result<Vec<arrow_array::RecordBatch>> {
    let mut node = build_node(kind, spec);
    let outputs = node
        .execute(
            &node_ctx(),
            &[],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await
        .expect("node execute failed");
    let df = outputs.dataframe(0).expect("missing output port 0").clone();
    df.collect().await
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associations_target_to_disease() {
    let batches = run_node(
        "source_opentargets_associations",
        serde_json::json!({
            "id": "ENSG00000012048",
            "b_filter": "breast",
            "min_score": 0.1,
            "fetch_all": true
        }),
    )
    .await
    .unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total > 0, "expected association rows for BRCA1");
    let schema = batches[0].schema();
    let cols: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        cols,
        vec![
            "target_id",
            "disease_id",
            "disease_name",
            "score",
            "novelty"
        ]
    );
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associations_disease_to_target() {
    let batches = run_node(
        "source_opentargets_associations",
        serde_json::json!({
            "direction": "disease_to_target",
            "id": "MONDO_0004975",
            "fetch_all": false,
            "size": 5
        }),
    )
    .await
    .unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total > 0);
    let schema = batches[0].schema();
    let cols: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        cols,
        vec![
            "disease_id",
            "target_id",
            "symbol",
            "approved_name",
            "biotype",
            "score",
            "novelty"
        ]
    );
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn search_node_runs() {
    let batches = run_node(
        "source_opentargets_search",
        serde_json::json!({"query": "BRCA1", "entity": ["target"], "size": 5}),
    )
    .await
    .unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total > 0);
}

/// Variant-ID table → `opentargets_genetic` → credible sets + L2G genes.
/// `17_66232877_G_C` is a real GCST90018961 (LDL cholesterol) lead variant
/// with APOH among its L2G predictions.
#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn genetic_node_variant_mode() {
    let ctx = datafusion::prelude::SessionContext::new();
    let ids = arrow_array::RecordBatch::try_from_iter(vec![(
        "variant_id",
        Arc::new(arrow_array::StringArray::from(vec!["17_66232877_G_C"]))
            as Arc<dyn arrow_array::Array>,
    )])
    .unwrap();
    let input_df = ctx.read_batch(ids).unwrap();

    let mut node = build_node(
        "opentargets_genetic",
        serde_json::json!({"min_l2g": 0.5, "batch_size": 10}),
    );
    let outputs = node
        .execute(
            &node_ctx(),
            &[data_engine::dag::NodeInput::new_dataframe(0, input_df)],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await
        .expect("genetic node execute failed");
    let df = outputs.dataframe(0).expect("missing output port 0").clone();
    let batches = df.collect().await.unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total > 0, "expected credible-set rows for the lead variant");
    let schema = batches[0].schema();
    let cols: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        cols,
        vec![
            "input_id",
            "study_locus_id",
            "study_id",
            "trait",
            "study_type",
            "lead_variant_id",
            "lead_variant_rsid",
            "pval",
            "beta",
            "purity_r2",
            "gene_id",
            "gene_symbol",
            "l2g_score"
        ]
    );
}

/// Regression for run 305a975f: an upstream `core/sql` node built the
/// variant ID with `concat(...)`, which DataFusion types as `Utf8View` —
/// the node used to reject it with "must be UTF-8 (found Utf8View)".
/// Same pipeline as `genetic_node_variant_mode`, but the ID column comes
/// from a real SQL projection instead of a hand-built `StringArray`.
#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn genetic_node_accepts_sql_concat_utf8_view_ids() {
    let ctx = SessionContext::new();
    let parts = arrow_array::RecordBatch::try_from_iter(vec![
        (
            "chrom",
            Arc::new(arrow_array::StringArray::from(vec!["17"])) as Arc<dyn arrow_array::Array>,
        ),
        (
            "pos",
            Arc::new(arrow_array::Int64Array::from(vec![66232877i64]))
                as Arc<dyn arrow_array::Array>,
        ),
        (
            "ref",
            Arc::new(arrow_array::StringArray::from(vec!["G"])) as Arc<dyn arrow_array::Array>,
        ),
        (
            "alt",
            Arc::new(arrow_array::StringArray::from(vec!["C"])) as Arc<dyn arrow_array::Array>,
        ),
    ])
    .unwrap();
    ctx.register_batch("parts", parts).unwrap();
    // SQL-composed variant ID, pinned to the view layout the failing run
    // handed the node (concat's own coercion depends on the DataFusion
    // version and input layouts, so pin it explicitly).
    let input_df = ctx
        .sql(
            "select arrow_cast(concat(chrom, '_', pos, '_', ref, '_', alt), 'Utf8View') \
             as variant_id from parts",
        )
        .await
        .unwrap();
    let physical = input_df.clone().collect().await.unwrap().remove(0);
    assert_eq!(
        physical.column(0).data_type(),
        &arrow_schema::DataType::Utf8View
    );

    let mut node = build_node(
        "opentargets_genetic",
        serde_json::json!({"min_l2g": 0.5, "batch_size": 10}),
    );
    let outputs = node
        .execute(
            &node_ctx(),
            &[data_engine::dag::NodeInput::new_dataframe(0, input_df)],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await
        .expect("genetic node must accept Utf8View variant_id");
    let df = outputs.dataframe(0).expect("missing output port 0").clone();
    let batches = df.collect().await.unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total > 0, "expected credible-set rows for the lead variant");
    // The queried ID round-trips into input_id unchanged.
    let ids = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow_array::StringArray>()
        .unwrap();
    assert_eq!(ids.value(0), "17_66232877_G_C");
}

/// Study-ID mode: one GCST study → its fine-mapped loci with L2G genes.
#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn genetic_node_study_mode() {
    let ctx = datafusion::prelude::SessionContext::new();
    let ids = arrow_array::RecordBatch::try_from_iter(vec![(
        "study_id",
        Arc::new(arrow_array::StringArray::from(vec!["GCST90018961"]))
            as Arc<dyn arrow_array::Array>,
    )])
    .unwrap();
    let input_df = ctx.read_batch(ids).unwrap();

    let mut node = build_node(
        "opentargets_genetic",
        serde_json::json!({"id_kind": "study", "id_column": "study_id", "min_l2g": 0.9}),
    );
    let outputs = node
        .execute(
            &node_ctx(),
            &[data_engine::dag::NodeInput::new_dataframe(0, input_df)],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await
        .expect("genetic node execute failed");
    let df = outputs.dataframe(0).expect("missing output port 0").clone();
    let batches = df.collect().await.unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(
        total > 0,
        "expected high-confidence L2G rows for GCST90018961"
    );
}
