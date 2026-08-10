//! DAG node implementations wrapping the [`grf`](crate) crate.
//!
//! Each node corresponds to one R-grf public function. The nodes live in
//! this crate rather than the global node bundle so the bio_crates/grf
//! dependency tree stays self-contained (grf is a heavy C++ build).
//!
//! Right now we expose the minimal P1 pair — `grf_regression_forest` and
//! `grf_predict_forest`. Subsequent phases (P2-P5 in `docs/grf_analysis.md`)
//! add the rest.

pub mod regression_forest;
pub mod predict_forest;

pub use regression_forest::{RegressionForestFactory, RegressionForestSpec};
pub use predict_forest::{PredictForestFactory, PredictForestSpec, PredictForestOutput};