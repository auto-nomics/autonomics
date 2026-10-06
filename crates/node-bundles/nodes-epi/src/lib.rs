//! Epidemiology DAG node bundle (RCS, ROC, LASSO, WQS, E-value,
//! multistate, GBTM, LCA, RF+SHAP, CFA).

pub mod epi_cfa;
pub mod epi_gbtm;
pub mod epi_lasso;
pub mod epi_lca;
pub mod epi_multistate;
pub mod epi_rcs;
pub mod epi_rf_shap;
pub mod epi_roc;
pub mod epi_wqs;
pub mod evalue;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "epi"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(epi_rcs::EpiRcsNodeFactory {}));
        registry.register(Box::new(epi_roc::EpiRocNodeFactory {}));
        registry.register(Box::new(epi_lasso::EpiLassoNodeFactory {}));
        registry.register(Box::new(epi_wqs::EpiWqsNodeFactory {}));
        registry.register(Box::new(evalue::EvalueNodeFactory {}));
        // Stat-side gaps from the dag-generalization survey §2.4
        // (`competing_risk` is already covered by nodes-survival's
        // `cuminc` + `fine_gray`).
        registry.register(Box::new(epi_multistate::EpiMultistateNodeFactory));
        registry.register(Box::new(epi_gbtm::EpiGbtmNodeFactory));
        registry.register(Box::new(epi_lca::EpiLcaNodeFactory));
        registry.register(Box::new(epi_rf_shap::EpiRfShapNodeFactory));
        registry.register(Box::new(epi_cfa::EpiCfaNodeFactory));
    }
}
