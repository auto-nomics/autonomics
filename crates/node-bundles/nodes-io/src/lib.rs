//! Source and sink DAG node bundle.

pub mod bundle_source;
pub mod coloc_abf_container;
pub mod container_command;
pub mod file_ref_source;
pub mod gcta_container;
pub mod hyprcoloc_container;
pub mod image_registry;
pub mod lava_container;
pub mod ldsc_h2_container;
pub mod ldsc_munge_container;
pub mod ldsc_rg_container;
pub mod magma_annotate_container;
pub mod mixer_container;
pub mod mrpresso_container;
pub mod mvmr_container;
pub mod plink2_clump_container;
pub mod sink_file;
pub mod smr_heidi_container;
pub mod source_file;
pub mod source_openalex;
pub mod source_opentargets;
pub mod source_semantic_scholar;
pub mod susie_rss_container;
pub mod twas_fusion_container;
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
        registry.register(Box::new(file_ref_source::FileRefSourceNodeFactory {}));
        registry.register(Box::new(source_file::FileSourceNodeFactory {}));
        registry.register(Box::new(sink_file::FileSinkNodeFactory {}));
        registry.register(Box::new(container_command::ContainerCommandNodeFactory {
            runtime: Arc::clone(&self.container_execution.runtime),
            panel_cache: Arc::clone(&self.container_execution.panel_cache),
        }));
        registry.register(Box::new(
            ldsc_h2_container::LdscH2ContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            ldsc_munge_container::LdscMungeContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            ldsc_rg_container::LdscRgContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            magma_annotate_container::MagmaAnnotateContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            mrpresso_container::MrpressoContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            coloc_abf_container::ColocAbfContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            hyprcoloc_container::HyPrColocContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(lava_container::LavaContainerNodeFactory::new(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(mvmr_container::MvmrContainerNodeFactory::new(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(
            plink2_clump_container::Plink2ClumpContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            mixer_container::MixerFit1ContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            mixer_container::MixerFit2ContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            susie_rss_container::SusieRssContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            twas_fusion_container::TwasFusionContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            smr_heidi_container::SmrHeidiContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            gcta_container::GctaContainerNodeFactory::cojo_select(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(gcta_container::GctaContainerNodeFactory::sblup(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(gcta_container::GctaContainerNodeFactory::fastbat(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(gcta_container::GctaContainerNodeFactory::acat(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
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
