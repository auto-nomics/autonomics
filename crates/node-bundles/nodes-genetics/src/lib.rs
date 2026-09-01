//! Statistical genetics DAG node bundle.

pub mod cpassoc;
pub mod genomic_sem;
pub mod magma;
pub mod magma_kegg;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "genetics"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(cpassoc::CpassocNodeFactory {}));
        registry.register(Box::new(magma::MagmaGeneNodeFactory {}));
        registry.register(Box::new(magma::MagmaSetNodeFactory {}));
        registry.register(Box::new(magma::MagmaMetaNodeFactory {}));
        registry.register(Box::new(magma_kegg::MagmaKeggAlignNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemMungeNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemLdscNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemUsermodelNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemCommonfactorNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemRgmodelNodeFactory {}));
    }
}
