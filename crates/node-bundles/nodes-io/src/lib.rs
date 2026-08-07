//! Source and sink DAG node bundle.

pub mod sink_file;
pub mod sink_iceberg;
pub mod source_file;
pub mod source_iceberg;
pub mod source_opengwas;
pub mod source_opengwas_tophits;
pub mod source_opentargets;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "io" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(source_file::FileSourceNodeFactory {}));
        registry.register(Box::new(source_iceberg::IcebergSourceNodeFactory {}));
        registry.register(Box::new(sink_file::FileSinkNodeFactory {}));
        registry.register(Box::new(sink_iceberg::IcebergSinkNodeFactory {}));
        registry.register(Box::new(source_opengwas_tophits::OpengwasTophitsNodeFactory {}));
        registry.register(Box::new(source_opentargets::OpentargetsAssociationsNodeFactory {}));
        registry.register(Box::new(source_opentargets::OpentargetsSearchNodeFactory {}));
        // opengwas source nodes
        registry.register(Box::new(source_opengwas::OpengwasAssociationsNodeFactory {}));
        registry.register(Box::new(source_opengwas::OpengwasPhewasNodeFactory {}));
        registry.register(Box::new(source_opengwas::OpengwasGwasinfoNodeFactory {}));
        registry.register(Box::new(source_opengwas::OpengwasGwasinfoSearchNodeFactory {}));
        registry.register(Box::new(source_opengwas::OpengwasVariantsRsidNodeFactory {}));
        registry.register(Box::new(source_opengwas::OpengwasVariantsChrposNodeFactory {}));
        registry.register(Box::new(source_opengwas::OpengwasLdClumpNodeFactory {}));
    }
}
