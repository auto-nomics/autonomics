//! `grf.*` DAG nodes — Generalized Random Forests, backed by the reference
//! C++ core via `grf-sys` (direct FFI, no R).
//!
//! Every node wraps a node spec from the `grf` crate (bio_crates/grf). Forests
//! are transported between nodes as single-row RecordBatches carrying the
//! serialized grf binary blob plus kind/feature metadata (see [`common`]).

pub mod causal_analysis;
pub mod common;
pub mod forest_analysis;
pub mod generate;
pub mod predict;
pub mod trainers;

use dag_core::{NodePlugin, NodeRegistry};

/// Plugin entry point — implements [`NodePlugin`] for the grf bundle.
pub struct Plugin;

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "grf"
    }

    fn register(&self, registry: &mut NodeRegistry) {
        // Forest trainers (12).
        registry.register(Box::new(trainers::RegressionNodeFactory));
        registry.register(Box::new(trainers::CausalNodeFactory));
        registry.register(Box::new(trainers::QuantileNodeFactory));
        registry.register(Box::new(trainers::ProbabilityNodeFactory));
        registry.register(Box::new(trainers::SurvivalNodeFactory));
        registry.register(Box::new(trainers::MultiRegressionNodeFactory));
        registry.register(Box::new(trainers::InstrumentalNodeFactory));
        registry.register(Box::new(trainers::LmNodeFactory));
        registry.register(Box::new(trainers::LlRegressionNodeFactory));
        registry.register(Box::new(trainers::BoostedRegressionNodeFactory));
        registry.register(Box::new(trainers::MultiArmCausalNodeFactory));
        registry.register(Box::new(trainers::CausalSurvivalNodeFactory));

        // Prediction.
        registry.register(Box::new(predict::PredictNodeFactory));

        // Causal analysis.
        registry.register(Box::new(causal_analysis::AverageTreatmentEffectNodeFactory));
        registry.register(Box::new(causal_analysis::BestLinearProjectionNodeFactory));
        registry.register(Box::new(causal_analysis::TestCalibrationNodeFactory));
        registry.register(Box::new(causal_analysis::GetScoresNodeFactory));

        // Forest analysis.
        registry.register(Box::new(forest_analysis::GetForestWeightsNodeFactory));
        registry.register(Box::new(forest_analysis::SplitFrequenciesNodeFactory));
        registry.register(Box::new(forest_analysis::VariableImportanceNodeFactory));
        registry.register(Box::new(forest_analysis::GetTreeNodeFactory));
        registry.register(Box::new(forest_analysis::MergeForestsNodeFactory));

        // Data generation.
        registry.register(Box::new(generate::GenerateCausalDataNodeFactory));
    }
}
