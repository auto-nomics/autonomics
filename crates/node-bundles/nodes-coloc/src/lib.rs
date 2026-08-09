//! Colocalisation DAG node bundle (coloc.abf, BKMR).

pub mod bkmr;
pub mod coloc;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "coloc"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(coloc::ColocAbfNodeFactory {}));
        registry.register(Box::new(bkmr::BkmrNodeFactory {}));
    }
}
