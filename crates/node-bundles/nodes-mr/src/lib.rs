//! Mendelian randomisation DAG node bundle.

pub mod ldsc_common;
pub mod mrlap;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "mr"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(mrlap::MrlapNodeFactory {}));
    }
}
