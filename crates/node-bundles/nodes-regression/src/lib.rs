//! Linear, logistic, cox regression + chi-square DAG node bundle.

pub mod chi_square;
pub mod cox_regression;
pub mod linear_regression;
pub mod logistic_regression;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "regression" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(linear_regression::LinearRegressionNodeFactory {}));
        registry.register(Box::new(logistic_regression::LogisticRegressionNodeFactory {}));
        registry.register(Box::new(cox_regression::CoxRegressionNodeFactory {}));
        registry.register(Box::new(chi_square::ChiSquareNodeFactory {}));
    }
}
