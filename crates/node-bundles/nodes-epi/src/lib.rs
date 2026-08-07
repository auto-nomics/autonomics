//! Epidemiology DAG node bundle (RCS, ROC, LASSO, WQS, E-value).

pub mod epi_lasso;
pub mod epi_rcs;
pub mod epi_roc;
pub mod epi_wqs;
pub mod evalue;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "epi" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(epi_rcs::EpiRcsNodeFactory {}));
        registry.register(Box::new(epi_roc::EpiRocNodeFactory {}));
        registry.register(Box::new(epi_lasso::EpiLassoNodeFactory {}));
        registry.register(Box::new(epi_wqs::EpiWqsNodeFactory {}));
        registry.register(Box::new(evalue::EvalueNodeFactory {}));
    }
}
