//! Linear, binary/ordinal logistic, cox regression + chi-square DAG node bundle.

pub mod binary_logistic_regression;
pub mod chi_square;
pub mod cox_regression;
pub mod glinternet;
pub mod hiernet;
pub mod linear_regression;
pub mod ordinal_logistic_regression;
pub mod rlm;
pub mod rrr;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "regression"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(linear_regression::LinearRegressionNodeFactory {}));
        registry.register(Box::new(
            binary_logistic_regression::BinaryLogisticRegressionNodeFactory {},
        ));
        registry.register(Box::new(
            ordinal_logistic_regression::OrdinalLogisticRegressionNodeFactory {},
        ));
        registry.register(Box::new(cox_regression::CoxRegressionNodeFactory {}));
        registry.register(Box::new(chi_square::ChiSquareNodeFactory {}));
        registry.register(Box::new(glinternet::GlinternetNodeFactory));
        registry.register(Box::new(hiernet::HierNetNodeFactory));
        registry.register(Box::new(rlm::RlmNodeFactory {}));
        registry.register(Box::new(rrr::RrrNodeFactory {}));
    }
}
