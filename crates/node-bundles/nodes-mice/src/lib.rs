//! `mice.*` DAG nodes — Multivariate Imputation by Chained Equations.
//!
//! Each node wraps an algorithm from the `mice` crate, adapting Arrow
//! RecordBatches ↔ Rust vector slices and handling spec validation.
//!
//! Node catalogue:
//!
//! | Node kind             | R function           | Module             |
//! |-----------------------|----------------------|--------------------|
//! | `mice_impute_pmm`     | `mice.impute.pmm`    | `pmm_node`         |
//! | `mice_impute_norm`    | `mice.impute.norm`   | `norm_node`        |
//! | `mice_impute_logreg`  | `mice.impute.logreg` | `logreg_node`      |
//! | `mice_impute_mean`    | `mice.impute.mean`   | `mean_node`        |
//! | `mice_impute_sample`  | `mice.impute.sample` | `sample_node`      |
//! | `mice`                | `mice::mice`         | `orchestrator_node`|
//! | `mice_complete`       | `mice::complete`     | `complete_node`    |
//!
//! Each univariate imputation node takes a single upstream DataFrame with
//! the response column and the predictor columns and emits a single-column
//! DataFrame containing the imputed values for the missing rows (one row
//! per missing entry, in the original row order).
//!
//! The orchestrator node takes the full incomplete data frame and produces
//! `m` completed data frames in long format (matching R's
//! `complete(action = "all", include = FALSE)`).

pub mod common;
pub mod complete_node;
pub mod error;
pub mod logreg_node;
pub mod mean_node;
pub mod norm_node;
pub mod orchestrator_node;
pub mod pmm_node;
pub mod sample_node;

use dag_core::{NodePlugin, NodeRegistry};

/// Plugin entry point — implements [`NodePlugin`] for the MICE bundle.
pub struct Plugin;

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "mice"
    }

    fn register(&self, registry: &mut NodeRegistry) {
        // Univariate imputation methods
        registry.register(Box::new(pmm_node::MiceImputePmmNodeFactory));
        registry.register(Box::new(norm_node::MiceImputeNormNodeFactory));
        registry.register(Box::new(logreg_node::MiceImputeLogregNodeFactory));
        registry.register(Box::new(mean_node::MiceImputeMeanNodeFactory));
        registry.register(Box::new(sample_node::MiceImputeSampleNodeFactory));

        // Orchestrator + extractor
        registry.register(Box::new(orchestrator_node::MiceOrchestratorNodeFactory));
        registry.register(Box::new(complete_node::MiceCompleteNodeFactory));
    }
}
