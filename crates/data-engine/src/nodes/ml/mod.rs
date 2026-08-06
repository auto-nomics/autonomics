//! `ml.*` DAG nodes — machine learning methods (scikit-learn-style).
//!
//! Each node wraps an algorithm from the `ml` crate, adapting Arrow
//! RecordBatches ↔ faer matrices and handling spec validation.
//!
//! See `docs/design/ml-nodes.md` for the full catalogue and architecture.

pub mod cluster_nodes;
pub mod common;
pub mod metrics_nodes;
pub mod model_nodes;
pub mod preprocess_nodes;
pub mod split_nodes;

/// Register all ML node factories with the engine's [`NodeRegistry`].
///
/// Called once at engine startup from `registry::NodeRegistry::new`.
pub fn register_all(registry: &mut crate::node_registry::registry::NodeRegistry) {
    // ── Preprocessing (10) ──────────────────────────────────────────────
    registry.register(Box::new(preprocess_nodes::StandardizeFactory));
    registry.register(Box::new(preprocess_nodes::MinMaxScaleFactory));
    registry.register(Box::new(preprocess_nodes::RobustScaleFactory));
    registry.register(Box::new(preprocess_nodes::NormalizeRowsFactory));
    registry.register(Box::new(preprocess_nodes::PowerTransformFactory));
    registry.register(Box::new(preprocess_nodes::ImputeFactory));
    registry.register(Box::new(preprocess_nodes::OneHotEncodeFactory));
    registry.register(Box::new(preprocess_nodes::LabelEncodeFactory));
    registry.register(Box::new(preprocess_nodes::PolyFeaturesFactory));
    registry.register(Box::new(preprocess_nodes::KBinsDiscretizeFactory));

    // ── Clustering (5) ──────────────────────────────────────────────────
    registry.register(Box::new(cluster_nodes::KMeansFactory));
    registry.register(Box::new(cluster_nodes::DbscanFactory));
    registry.register(Box::new(cluster_nodes::GmmFactory));
    registry.register(Box::new(cluster_nodes::HierarchicalFactory));
    registry.register(Box::new(cluster_nodes::SpectralClusteringFactory));

    // ── Split & CV (3) ──────────────────────────────────────────────────
    registry.register(Box::new(split_nodes::TrainTestSplitFactory));
    registry.register(Box::new(split_nodes::KFoldFactory));
    registry.register(Box::new(split_nodes::StratifiedKFoldFactory));

    // ── Metrics (2) ─────────────────────────────────────────────────────
    registry.register(Box::new(metrics_nodes::ClassificationMetricsFactory));
    registry.register(Box::new(metrics_nodes::RegressionMetricsFactory));

    // ── Model artifact (2) ──────────────────────────────────────────────
    registry.register(Box::new(model_nodes::ModelSaveFactory));
    registry.register(Box::new(model_nodes::ModelLoadFactory));
}
