//! Mendelian randomisation DAG node bundle.

pub mod mrpresso;
pub mod mrlap;
pub mod mvmr;
pub mod two_sample_mr;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "mr" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(two_sample_mr::TwoSampleMrNodeFactory {}));
        registry.register(Box::new(mrlap::MrlapNodeFactory {}));
        registry.register(Box::new(mrpresso::MrpressoNodeFactory {}));
        registry.register(Box::new(mvmr::MvmrNodeFactory {}));
    }
}
