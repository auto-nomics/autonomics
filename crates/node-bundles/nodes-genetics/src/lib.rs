//! Statistical genetics DAG node bundle.

pub mod cpassoc;
pub mod magma_kegg;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "genetics"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(cpassoc::CpassocNodeFactory {}));
        registry.register(Box::new(magma_kegg::MagmaKeggAlignNodeFactory {}));
    }
}
