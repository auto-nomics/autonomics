//! DAG node bundle for file, DataFrame, and container I/O boundaries.

pub mod bundle_source;
pub mod coloc_abf_container;
pub mod container_command;
pub mod dataframe_to_file;
pub mod deseq2_container;
pub mod file_reference;
pub mod file_to_dataframe;
pub mod file_transform;
pub mod gcta_container;
pub mod gmt_import;
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
pub mod mutation_analysis_container;
pub mod mvmr_container;
pub mod parquet_sql;
pub mod pathway_gsea_container;
pub mod pathology_container;
pub mod plink2_clump_container;
pub mod radiomics;
pub mod radiomics_container;
pub mod script_nodes;
pub mod single_cell_container;
pub mod single_cell_h5ad;
pub mod smr_heidi_container;
pub mod source_chembl;
pub mod source_kegg;
pub mod source_openalex;
pub mod source_opentargets;
pub mod source_semantic_scholar;
pub mod susie_rss_container;
pub mod timesfm_container;
pub mod twas_fusion_container;
pub mod twosamplemr_container;
pub mod visualization_container;
pub use alphafold::nodes::prediction::{AlphaFoldPredictionNode, AlphaFoldPredictionNodeFactory};
pub use clinicaltrials::nodes::study::{ClinicalTrialsStudyNode, ClinicalTrialsStudyNodeFactory};
pub use crossref::nodes::works::{CrossrefWorksNode, CrossrefWorksNodeFactory};
pub use nhanes::nodes::download::{NhanesDownloadNode, NhanesDownloadNodeFactory};
pub use nhanes::nodes::files::{NhanesFilesNode, NhanesFilesNodeFactory};
pub use interpro::nodes::entry::{InterProEntryNode, InterProEntryNodeFactory};
pub use pubchem::nodes::compound::{PubChemCompoundNode, PubChemCompoundNodeFactory};
pub use rcsb::nodes::assembly::{RcsbAssemblyNode, RcsbAssemblyNodeFactory};
pub use rcsb::nodes::entry::{RcsbEntryNode, RcsbEntryNodeFactory};
pub use rcsb::nodes::polymer_entity::{RcsbPolymerEntityNode, RcsbPolymerEntityNodeFactory};
pub use rcsb::nodes::search::{RcsbSearchNode, RcsbSearchNodeFactory};
pub use rcsb::nodes::structure::{RcsbStructureNode, RcsbStructureNodeFactory};
pub use reactome::nodes::analysis::{ReactomeAnalysisNode, ReactomeAnalysisNodeFactory};
pub use reactome::nodes::mapping::{ReactomeMappingNode, ReactomeMappingNodeFactory};
pub use reactome::nodes::participants::{
    ReactomeParticipantsNode, ReactomeParticipantsNodeFactory,
};
pub use reactome::nodes::pathways::{ReactomePathwaysNode, ReactomePathwaysNodeFactory};
pub use string_sdk::nodes::{Plugin as StringPlugin, StringIdMapNode, StringIdMapNodeFactory};
pub use string_sdk::nodes::{
    StringEnrichmentNode, StringEnrichmentNodeFactory, StringNetworkNode, StringNetworkNodeFactory,
};
pub use string_sdk::nodes::{StringPpiEnrichmentNode, StringPpiEnrichmentNodeFactory};
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
        registry.register(Box::new(file_transform::FileTransformNodeFactory {}));
        registry.register(Box::new(script_nodes::ScriptNodeFactory::python(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(script_nodes::ScriptNodeFactory::r(
            Arc::clone(&self.container_execution.runtime),
            Arc::clone(&self.container_execution.panel_cache),
        )));
        registry.register(Box::new(gmt_import::GmtImportNodeFactory {}));
        registry.register(Box::new(dataframe_to_file::DataFrameToFileNodeFactory {}));
        registry.register(Box::new(parquet_sql::DataFusionSqlNodeFactory {}));
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
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::bias_correct(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::robust_normalize(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::peritumoral_ring(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::habitat_fit(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::habitat_assign(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            radiomics_container::RadiomicsContainerNodeFactory::perturb_stability(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            pathology_container::PathologyContainerNodeFactory::wsi_ingest(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            pathology_container::PathologyContainerNodeFactory::wsi_qc(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            pathology_container::PathologyContainerNodeFactory::patch_sample(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            pathology_container::PathologyContainerNodeFactory::wsi_embed(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            pathology_container::PathologyContainerNodeFactory::domain_check(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            pathology_container::PathologyContainerNodeFactory::ihc_quant(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            pathology_container::PathologyContainerNodeFactory::qupath_import(
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
            deseq2_container::Deseq2DeContainerNodeFactory::new(
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
        registry.register(Box::new(
            pathway_gsea_container::PathwayGseaContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        registry.register(Box::new(
            twosamplemr_container::TwoSampleMrContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
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
            timesfm_container::TimesfmForecastContainerNodeFactory::new(
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
            single_cell_container::SingleCellPreprocessorContainerNodeFactory::new(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ));
        for factory in [
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::qc_filter(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::embed_cluster(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::celltypist(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::obs_projection(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::subset(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::dense_ingest(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::rank_genes_groups(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::cluster_mean_expression(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
            single_cell_h5ad::SingleCellH5adContainerNodeFactory::gene_set_score(
                Arc::clone(&self.container_execution.runtime),
                Arc::clone(&self.container_execution.panel_cache),
            ),
        ] {
            registry.register(Box::new(factory));
        }
        registry.register(Box::new(
            mutation_analysis_container::MutationAnalysisContainerNodeFactory::new(
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
        registry.register(Box::new(source_chembl::ChemblActivitiesNodeFactory));
        registry.register(Box::new(source_chembl::ChemblMoleculesNodeFactory));
        registry.register(Box::new(source_openalex::OpenAlexWorksNodeFactory {}));
        registry.register(Box::new(source_kegg::KeggSearchNodeFactory));
        registry.register(Box::new(source_kegg::KeggRelationsNodeFactory));
        registry.register(Box::new(source_kegg::KeggGenePathwaysNodeFactory));
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
        registry.register(Box::new(NhanesFilesNodeFactory {}));
        registry.register(Box::new(NhanesDownloadNodeFactory {}));
        registry.register(Box::new(AlphaFoldPredictionNodeFactory));
        registry.register(Box::new(InterProEntryNodeFactory));
        registry.register(Box::new(PubChemCompoundNodeFactory));
        registry.register(Box::new(ClinicalTrialsStudyNodeFactory));
        registry.register(Box::new(RcsbSearchNodeFactory {}));
        registry.register(Box::new(RcsbEntryNodeFactory {}));
        registry.register(Box::new(RcsbPolymerEntityNodeFactory {}));
        registry.register(Box::new(RcsbAssemblyNodeFactory {}));
        registry.register(Box::new(RcsbStructureNodeFactory {}));
        registry.register(Box::new(UniprotSearchNodeFactory {}));
        registry.register(Box::new(UniprotStreamNodeFactory {}));
        registry.register(Box::new(UniprotIdmapNodeFactory {}));
        registry.register(Box::new(ReactomePathwaysNodeFactory {}));
        registry.register(Box::new(ReactomeMappingNodeFactory {}));
        registry.register(Box::new(ReactomeAnalysisNodeFactory {}));
        registry.register(Box::new(ReactomeParticipantsNodeFactory {}));
        registry.register_plugin(&StringPlugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_the_deseq2_container_node() {
        let ctx = dag_core::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let mut registry = NodeRegistry::new(ctx);
        registry.register_plugin(&Plugin::new(Arc::new(
            container_runtime::ContainerExecutionInfra::from_env(),
        )));
        assert!(
            registry
                .list_nodes()
                .iter()
                .any(|node| node.kind == deseq2_container::DESEQ2_DE_CONTAINER_KIND)
        );
    }
}
