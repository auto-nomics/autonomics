//! DAG node bundle for file, DataFrame, and container I/O boundaries.

pub mod bundle_source;
pub mod coloc_abf_container;
pub mod container_command;
pub mod dataframe_to_file;
pub mod file_reference;
pub mod file_to_dataframe;
pub mod gcta_container;
pub mod hdl_l_container;
pub mod hdl_l_scan_container;
pub mod hyprcoloc_container;
pub mod image_registry;
pub mod lava_container;
pub mod lava_scan_container;
pub mod ldsc_h2_container;
pub mod ldsc_munge_container;
pub mod ldsc_rg_container;
pub mod magma_annotate_container;
pub mod mixer_container;
pub mod mrpresso_container;
pub mod mtag_container;
pub mod mvmr_container;
pub mod plink2_clump_container;
pub mod radiomics;
pub mod radiomics_container;
pub mod smr_heidi_container;
pub mod source_openalex;
pub mod source_opentargets;
pub mod source_semantic_scholar;
pub mod susie_rss_container;
pub mod twas_fusion_container;
pub mod visualization_container;
pub use crossref::nodes::works::{CrossrefWorksNode, CrossrefWorksNodeFactory};
pub use rcsb::nodes::entry::{RcsbEntryNode, RcsbEntryNodeFactory};
pub use rcsb::nodes::polymer_entity::{RcsbPolymerEntityNode, RcsbPolymerEntityNodeFactory};
pub use rcsb::nodes::search::{RcsbSearchNode, RcsbSearchNodeFactory};
pub use rcsb::nodes::structure::{RcsbStructureNode, RcsbStructureNodeFactory};
pub use uniprot::nodes::idmap::{UniprotIdmapNode, UniprotIdmapNodeFactory};
pub use uniprot::nodes::search::{UniprotSearchNode, UniprotSearchNodeFactory};
pub use uniprot::nodes::stream::{UniprotStreamNode, UniprotStreamNodeFactory};

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
        registry.register(Box::new(file_reference::FileReferenceNodeFactory {}));
        registry.register(Box::new(file_to_dataframe::FileToDataFrameNodeFactory {}));
        registry.register(Box::new(dataframe_to_file::DataFrameToFileNodeFactory {}));
        registry.register(Box::new(radiomics::RadiomicsManifestNodeFactory));
        registry.register(Box::new(radiomics::RadiomicsStageFileSetNodeFactory));
        registry.register(Box::new(radiomics::RadiomicsDcmGlobNodeFactory));
        registry.register(Box::new(radiomics::RadiomicsQcNodeFactory));
        registry.register(Box::new(radiomics::RadiomicsFeatureSetAssembleNodeFactory));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::image_ingest(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::mask_ingest(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::pair_validate(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::preprocess(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::extract(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::batch_extract(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::dicom_metadata(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::phi_scrub(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::voi_similarity(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::image_qc(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::rtstruct_geometry(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::ivh(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::shape_topology(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::register(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::delta_features(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
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
        registry.register(Box::new(hdl_l_container::HdlLContainerNodeFactory::new(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(
            hdl_l_scan_container::HdlLScanContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(lava_container::LavaContainerNodeFactory::new(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(
            lava_scan_container::LavaScanContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(mvmr_container::MvmrContainerNodeFactory::new(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(mtag_container::MtagContainerNodeFactory::new(
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
            visualization_container::VisualizationContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
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
        registry.register(Box::new(RcsbSearchNodeFactory {}));
        registry.register(Box::new(RcsbEntryNodeFactory {}));
        registry.register(Box::new(RcsbPolymerEntityNodeFactory {}));
        registry.register(Box::new(RcsbStructureNodeFactory {}));
        registry.register(Box::new(UniprotSearchNodeFactory {}));
        registry.register(Box::new(UniprotStreamNodeFactory {}));
        registry.register(Box::new(UniprotIdmapNodeFactory {}));
    }
}
