//! DAG node implementations wrapping the [`grf`](crate) crate.
//!
//! Each node corresponds to one R-grf public function. The nodes live in
//! this crate rather than the global node bundle so the bio_crates/grf
//! dependency tree stays self-contained (grf is a heavy C++ build).

pub mod regression_forest;
pub mod predict_forest;
pub mod quantile_forest;
pub mod probability_forest;
pub mod survival_forest;
pub mod multi_regression_forest;
pub mod causal_forest;
pub mod dr_scores;
pub mod average_treatment_effect;
pub mod best_linear_projection;
pub mod test_calibration;

pub use regression_forest::{RegressionForestFactory, RegressionForestSpec};
pub use predict_forest::{PredictForestFactory, PredictForestSpec, PredictForestOutput};
pub use quantile_forest::{QuantileForestFactory, QuantileForestSpec, QuantileForestOutput};
pub use probability_forest::{ProbabilityForestFactory, ProbabilityForestSpec, ProbabilityForestOutput};
pub use survival_forest::{SurvivalForestFactory, SurvivalForestSpec, SurvivalForestOutput};
pub use multi_regression_forest::{
    MultiRegressionForestFactory, MultiRegressionForestSpec, MultiRegressionForestOutput,
};
pub use causal_forest::{
    CausalForestFactory, CausalForestSpec, CausalForestOutput,
};
pub use average_treatment_effect::{
    AverageTreatmentEffectFactory, AverageTreatmentEffectSpec, AverageTreatmentEffectOutput,
};
pub use best_linear_projection::{
    BestLinearProjectionFactory, BestLinearProjectionSpec, BestLinearProjectionOutput,
};
pub use test_calibration::{
    TestCalibrationFactory, TestCalibrationSpec, TestCalibrationOutput,
};