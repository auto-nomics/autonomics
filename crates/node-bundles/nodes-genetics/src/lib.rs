//! Statistical genetics DAG node bundle.
//!
//! The native CPASSOC port (`cpassoc` node + `bio_crates/cpassoc`) was
//! removed; the bundle now carries only the KEGG-side data-prep node that
//! feeds MAGMA gene-set analysis.

pub mod magma_kegg;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "genetics"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(magma_kegg::MagmaKeggAlignNodeFactory {}));
    }
}
