//! Analytic power and sample-size DAG nodes.
//!
//! Each node answers a prospective design question for a specified effect:
//! power, sample size, or minimum detectable effect. Nodes emit one common
//! result row rather than reusing observed-data hypothesis-test rows.

pub mod common;
pub mod correlation;
pub mod counts;
pub mod linear_models;
pub mod means;
pub mod survival_cluster;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "power"
    }

    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(means::PowerTTestNodeFactory));
        registry.register(Box::new(counts::PowerPropTestNodeFactory));
        registry.register(Box::new(counts::PowerChisqTestNodeFactory));
        registry.register(Box::new(correlation::PowerCorrelationNodeFactory));
        registry.register(Box::new(linear_models::PowerAnovaNodeFactory));
        registry.register(Box::new(linear_models::PowerRegressionNodeFactory));
        registry.register(Box::new(survival_cluster::PowerSurvivalNodeFactory));
        registry.register(Box::new(survival_cluster::PowerClusterNodeFactory));
    }
}
