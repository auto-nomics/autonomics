//! Source and sink DAG node bundle.

pub mod bundle_source;
pub mod container_command;
pub mod sink_file;
pub mod source_file;
pub mod source_openalex;
pub mod source_opentargets;
pub mod source_semantic_scholar;
pub use crossref::nodes::works::{CrossrefWorksNode, CrossrefWorksNodeFactory};

use dag_core::{NodePlugin, NodeRegistry};
use std::sync::Arc;

#[derive(Default)]
pub struct Plugin {
    container_runtime: Arc<container_runtime::K3sRuntime>,
    panel_cache: Arc<container_runtime::PanelCache>,
}

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "io"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(bundle_source::BundleSourceNodeFactory {}));
        registry.register(Box::new(source_file::FileSourceNodeFactory {}));
        registry.register(Box::new(sink_file::FileSinkNodeFactory {}));
        registry.register(Box::new(container_command::ContainerCommandNodeFactory {
            runtime: Arc::clone(&self.container_runtime),
            panel_cache: Arc::clone(&self.panel_cache),
        }));
        registry.register(Box::new(
            source_opentargets::OpentargetsAssociationsNodeFactory {},
        ));
        registry.register(Box::new(
            source_opentargets::OpentargetsSearchNodeFactory {},
        ));
        registry.register(Box::new(source_openalex::OpenAlexWorksNodeFactory {}));
        registry.register(Box::new(source_openalex::OpenAlexGroupByNodeFactory {}));
        registry.register(Box::new(
            source_semantic_scholar::S2PaperSearchNodeFactory {},
        ));
        registry.register(Box::new(
            source_semantic_scholar::S2AuthorSearchNodeFactory {},
        ));
        registry.register(Box::new(
            source_opentargets::OpentargetsAssociationsNodeFactory {},
        ));
        registry.register(Box::new(
            source_opentargets::OpentargetsSearchNodeFactory {},
        ));
        registry.register(Box::new(CrossrefWorksNodeFactory {}));
    }
}
