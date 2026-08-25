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

pub struct Plugin {
    container_execution: Arc<container_runtime::ContainerExecutionInfra>,
}

impl Plugin {
    pub fn new(container_execution: Arc<container_runtime::ContainerExecutionInfra>) -> Self {
        Self {
            container_execution,
        }
    }
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
            runtime: Arc::clone(&self.container_execution.k3s),
            panel_cache: Arc::clone(&self.container_execution.panel_cache),
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
