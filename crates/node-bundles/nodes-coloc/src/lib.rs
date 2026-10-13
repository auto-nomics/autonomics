//! Colocalisation DAG node bundle.
//!
//! `coloc_abf` runs through the manifest plugin at
//! `/mnt/projects/node-plugins/coloc`; the native arrow-based implementation
//! in [`coloc`] stays as unregistered reference code. The native BKMR port
//! (`bkmr` node + `bio_crates/bkmr`) was removed.

pub mod coloc;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "coloc"
    }
    fn register(&self, _registry: &mut NodeRegistry) {
        // `coloc_abf` moved to the manifest plugin at
        // `/mnt/projects/node-plugins/coloc`; the plugin loader registers
        // that kind at startup, and a duplicate registration here would
        // silently shadow it (last-write-wins). The native arrow-based
        // implementation stays in this crate as reference code.
        // registry.register(Box::new(coloc::ColocAbfNodeFactory {}));
        // The native BKMR port that used to register here was removed.
    }
}
