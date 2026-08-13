//! DAG node implementations wrapping the [`grf`](crate) crate.
//!
//! Each node corresponds to one R-grf public function. The nodes live in
//! this crate rather than the global node bundle so the bio_crates/grf
//! dependency tree stays self-contained (grf is a heavy C++ build).

pub mod average_treatment_effect;
pub mod best_linear_projection;
pub mod boosted_regression_forest;
pub mod causal_forest;
pub mod causal_survival_forest;
pub mod dr_scores;
pub mod forest_analysis;
pub mod generate_causal_data;
pub mod get_scores;
pub mod instrumental_forest;
pub mod ll_regression_forest;
pub mod lm_forest;
pub mod multi_arm_causal_forest;
pub mod multi_regression_forest;
pub mod predict_forest;
pub mod probability_forest;
pub mod quantile_forest;
pub mod regression_forest;
pub mod survival_forest;
pub mod test_calibration;

pub use average_treatment_effect::{
    AverageTreatmentEffectFactory, AverageTreatmentEffectOutput, AverageTreatmentEffectSpec,
};
pub use best_linear_projection::{
    BestLinearProjectionFactory, BestLinearProjectionOutput, BestLinearProjectionSpec,
};
pub use boosted_regression_forest::{
    BoostedRegressionForestFactory, BoostedRegressionForestOutput, BoostedRegressionForestSpec,
};
pub use causal_forest::{CausalForestFactory, CausalForestOutput, CausalForestSpec};
pub use causal_survival_forest::{
    CausalSurvivalForestFactory, CausalSurvivalForestOutput, CausalSurvivalForestSpec,
};
pub use forest_analysis::{
    ForestWeightsOutput, GetForestWeightsFactory, GetForestWeightsSpec, GetTreeFactory,
    GetTreeOutput, GetTreeSpec, MergeForestsFactory, MergeForestsOutput, MergeForestsSpec,
    SplitFrequenciesFactory, SplitFrequenciesOutput, SplitFrequenciesSpec,
    VariableImportanceFactory, VariableImportanceOutput, VariableImportanceSpec,
};
pub use generate_causal_data::{
    GenerateCausalDataFactory, GenerateCausalDataOutput, GenerateCausalDataSpec,
};
pub use get_scores::{GetScoresFactory, GetScoresOutput, GetScoresSpec};
pub use instrumental_forest::{
    InstrumentalForestFactory, InstrumentalForestOutput, InstrumentalForestSpec,
};
pub use ll_regression_forest::{
    LlRegressionForestFactory, LlRegressionForestOutput, LlRegressionForestSpec,
};
pub use lm_forest::{LmForestFactory, LmForestOutput, LmForestSpec};
pub use multi_arm_causal_forest::{
    MultiArmCausalForestFactory, MultiArmCausalForestOutput, MultiArmCausalForestSpec,
};
pub use multi_regression_forest::{
    MultiRegressionForestFactory, MultiRegressionForestOutput, MultiRegressionForestSpec,
};
pub use predict_forest::{PredictForestFactory, PredictForestOutput, PredictForestSpec};
pub use probability_forest::{
    ProbabilityForestFactory, ProbabilityForestOutput, ProbabilityForestSpec,
};
pub use quantile_forest::{QuantileForestFactory, QuantileForestOutput, QuantileForestSpec};
pub use regression_forest::{RegressionForestFactory, RegressionForestSpec};
pub use survival_forest::{SurvivalForestFactory, SurvivalForestOutput, SurvivalForestSpec};
pub use test_calibration::{TestCalibrationFactory, TestCalibrationOutput, TestCalibrationSpec};
