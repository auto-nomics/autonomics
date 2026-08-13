//! `dl.*` DAG nodes — deep learning methods (neural networks).
//!
//! Each node wraps an algorithm from the `dl` crate, adapting Arrow
//! RecordBatches ↔ Tensor and handling spec validation.
//!
//! ## Nodes (Phase 1–2)
//!
//! | Category | Nodes |
//! |----------|-------|
//! | L1: Training | `dl_mlp_train`, `dl_deepsurv_train` |
//! | L2: Inference | `dl_predict` |
//! | L3: Management | `dl_model_save`, `dl_model_load`, `dl_model_info` |
//! | L4: Evaluation | `dl_survival_metrics`, `dl_calibration`, `dl_nri` |
//! | L5: Data | `dl_train_val_test_split` |

pub mod common;
pub mod metrics_nodes;
pub mod model_nodes;
pub mod phase3_nodes;
pub mod predict_node;
pub mod split_node;
pub mod train_nodes;
pub mod transformer_nodes;

use dag_core::{NodePlugin, NodeRegistry};

/// Plugin entry point — implements [`NodePlugin`] for the DL bundle.
pub struct Plugin;

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "dl"
    }

    fn register(&self, registry: &mut NodeRegistry) {
        // ── L5: Data ────────────────────────────────────────────────────
        registry.register(Box::new(split_node::TrainValTestSplitFactory));

        // ── L1: Training ────────────────────────────────────────────────
        registry.register(Box::new(train_nodes::MlpTrainFactory));
        registry.register(Box::new(train_nodes::DeepSurvTrainFactory));
        registry.register(Box::new(transformer_nodes::TransformerTrainFactory));
        registry.register(Box::new(phase3_nodes::AutoEncoderTrainFactory));
        registry.register(Box::new(phase3_nodes::DeepHitTrainFactory));
        registry.register(Box::new(phase3_nodes::RnnTrainFactory));

        // ── L2: Inference + Embedding ───────────────────────────────────
        registry.register(Box::new(predict_node::PredictFactory));
        registry.register(Box::new(phase3_nodes::EmbedFactory));

        // ── L3: Management ──────────────────────────────────────────────
        registry.register(Box::new(model_nodes::ModelSaveFactory));
        registry.register(Box::new(model_nodes::ModelLoadFactory));
        registry.register(Box::new(model_nodes::ModelInfoFactory));

        // ── L4: Evaluation ──────────────────────────────────────────────
        registry.register(Box::new(metrics_nodes::SurvivalMetricsFactory));
        registry.register(Box::new(metrics_nodes::CalibrationFactory));
        registry.register(Box::new(metrics_nodes::NriFactory));
    }
}
