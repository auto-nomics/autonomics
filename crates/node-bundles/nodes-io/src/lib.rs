//! DAG node bundle for file, DataFrame, and container I/O boundaries.

pub mod archive_nodes;
pub mod bundle_source;
pub mod container_command;
pub mod dataframe_to_file;
pub mod file_decompress;
pub mod file_reference;
pub mod file_set_select;
pub mod file_to_dataframe;
pub mod file_transform;
pub mod gmt_import;
pub mod h5ad_obs_to_dataframe;
pub mod http_fetch;
pub mod image_registry;
pub mod multiomic_concordance;
pub mod parquet_sql;
pub mod radiomics;
pub mod script_nodes;
pub mod source_chembl;
pub mod source_kegg;
pub mod source_openalex;
pub mod source_opentargets;
pub mod source_semantic_scholar;
pub mod spreadsheet;
pub use alphafold::nodes::prediction::{AlphaFoldPredictionNode, AlphaFoldPredictionNodeFactory};
pub use clinicaltrials::nodes::study::{ClinicalTrialsStudyNode, ClinicalTrialsStudyNodeFactory};
pub use crossref::nodes::works::{CrossrefWorksNode, CrossrefWorksNodeFactory};
pub use enrichr_sdk::nodes::Plugin as EnrichrPlugin;
pub use enrichr_sdk::nodes::{
    EnrichrBackgroundEnrichmentNode, EnrichrBackgroundEnrichmentNodeFactory, EnrichrEnrichmentNode,
    EnrichrEnrichmentNodeFactory, EnrichrGeneMapNode, EnrichrGeneMapNodeFactory,
    EnrichrLibrariesNode, EnrichrLibrariesNodeFactory, EnrichrViewListNode,
    EnrichrViewListNodeFactory,
};
pub use interpro::nodes::entry::{InterProEntryNode, InterProEntryNodeFactory};
pub use nhanes::nodes::download::{NhanesDownloadNode, NhanesDownloadNodeFactory};
pub use nhanes::nodes::files::{NhanesFilesNode, NhanesFilesNodeFactory};
pub use protocolio::nodes::materials::{ProtocolioMaterialsNode, ProtocolioMaterialsNodeFactory};
pub use protocolio::nodes::pdf::{ProtocolioPdfNode, ProtocolioPdfNodeFactory};
pub use protocolio::nodes::protocol::{ProtocolioProtocolNode, ProtocolioProtocolNodeFactory};
pub use protocolio::nodes::reagents::{ProtocolioReagentsNode, ProtocolioReagentsNodeFactory};
pub use protocolio::nodes::search::{ProtocolioSearchNode, ProtocolioSearchNodeFactory};
pub use protocolio::nodes::steps::{ProtocolioStepsNode, ProtocolioStepsNodeFactory};
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

/// Container-backed nodes moved to manifest plugins; the io bundle is pure
/// engine-side and carries no container infrastructure.
#[derive(Default)]
pub struct Plugin;

impl Plugin {
    pub fn new() -> Self {
        Self
    }
}

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "io"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(bundle_source::BundleSourceNodeFactory {}));
        registry.register(Box::new(file_reference::FileReferenceNodeFactory {}));
        registry.register(Box::new(file_decompress::FileDecompressNodeFactory));
        registry.register(Box::new(file_to_dataframe::FileToDataFrameNodeFactory {}));
        registry.register(Box::new(file_set_select::FileSetSelectNodeFactory));
        registry.register(Box::new(archive_nodes::ArchiveInspectNodeFactory));
        registry.register(Box::new(archive_nodes::ArchiveExtractNodeFactory));
        registry.register(Box::new(http_fetch::HttpFetchNodeFactory));
        registry.register(Box::new(
            h5ad_obs_to_dataframe::H5adObsToDataFrameNodeFactory {},
        ));
        registry.register(Box::new(file_transform::FileTransformNodeFactory {}));
        // `python_script` and `r_script` are intentionally NOT registered — they
        // are removed from the DAG surface so the Agent cannot list or add them.
        // The `script_nodes` module is retained for unit/integration tests of
        // the underlying ScriptNodeFactory implementation; constructing one
        // directly still works, but `NodeRegistry::build_node(...)` will return
        // `Error::FactoryNotFound` for these kinds.
        registry.register(Box::new(gmt_import::GmtImportNodeFactory {}));
        registry.register(Box::new(dataframe_to_file::DataFrameToFileNodeFactory {}));
        registry.register(Box::new(parquet_sql::DataFusionSqlNodeFactory {}));
        registry.register(Box::new(radiomics::RadiomicsManifestNodeFactory));
        registry.register(Box::new(radiomics::RadiomicsStageFileSetNodeFactory));
        registry.register(Box::new(radiomics::RadiomicsDcmGlobNodeFactory));
        registry.register(Box::new(radiomics::RadiomicsQcNodeFactory));
        registry.register(Box::new(radiomics::RadiomicsFeatureSetAssembleNodeFactory));
        // `ldsc_h2`, `ldsc_munge`, and `ldsc_rg`
        // moved to the manifest plugin at `/mnt/projects/node-plugins/ldsc`;
        // their kinds are registered by the plugin loader at startup.
        // `magma_annotate`, `mrpresso`, `mvmr`, `coloc_abf`, `deseq2_de`,
        // the four `gcta_*` variants, `pathway_gsea`, `plink2_clump`, and
        // `visualization`, `single_cell_preprocessor`,
        // the ten `h5ad_*` variants, the radiomics container variants, and
        // the seven `pathology_*` variants likewise moved to manifest
        // plugins under `/mnt/projects/node-plugins/`.
        // `limma_voom` and `wgcna` (bulk-rnaseq family) and `hyprcoloc`
        // completed the sweep: every container-backed node now ships as a
        // manifest plugin, and this registry holds only in-process nodes.
        registry.register(Box::new(
            multiomic_concordance::MultiomicConcordanceNodeFactory,
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
        registry.register(Box::new(ProtocolioSearchNodeFactory));
        registry.register(Box::new(ProtocolioProtocolNodeFactory));
        registry.register(Box::new(ProtocolioStepsNodeFactory));
        registry.register(Box::new(ProtocolioMaterialsNodeFactory));
        registry.register(Box::new(ProtocolioReagentsNodeFactory));
        registry.register(Box::new(ProtocolioPdfNodeFactory));
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
        registry.register_plugin(&EnrichrPlugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_protocolio_source_nodes() {
        let ctx = dag_core::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let mut registry = NodeRegistry::new(ctx);
        registry.register_plugin(&Plugin::new());
        let expected = [
            "source_protocolio_protocols",
            "source_protocolio_protocol",
            "source_protocolio_steps",
            "source_protocolio_materials",
            "source_protocolio_reagents",
            "source_protocolio_pdf",
        ];
        for kind in expected {
            assert!(
                registry.list_nodes().iter().any(|node| node.kind == kind),
                "missing protocol.io node: {kind}"
            );
        }
    }

    #[test]
    fn plugin_registers_file_decompression_and_selection_nodes() {
        let ctx = dag_core::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let mut registry = NodeRegistry::new(ctx);
        registry.register_plugin(&Plugin::new());

        let decompress = registry
            .get_node_ports(file_decompress::FILE_DECOMPRESS_KIND)
            .expect("file_decompress is registered");
        assert_eq!(
            decompress.input_port(0).unwrap().data_type,
            dag_core::value::PortType::File
        );
        assert_eq!(
            decompress.output_port(0).unwrap().data_type,
            dag_core::value::PortType::File
        );

        let select = registry
            .get_node_ports(file_set_select::FILE_SET_SELECT_KIND)
            .expect("file_set_select is registered");
        assert_eq!(
            select.input_port(0).unwrap().data_type,
            dag_core::value::PortType::FileSet
        );
        assert_eq!(
            select.output_port(0).unwrap().data_type,
            dag_core::value::PortType::File
        );
    }

    #[test]
    fn plugin_registers_archive_nodes() {
        let ctx = dag_core::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let mut registry = NodeRegistry::new(ctx);
        registry.register_plugin(&Plugin::new());

        let inspect = registry
            .get_node_ports(archive_nodes::ARCHIVE_INSPECT_KIND)
            .expect("archive_inspect is registered");
        assert_eq!(
            inspect.output_port(0).unwrap().data_type,
            dag_core::value::PortType::DataFrame
        );
        let extract = registry
            .get_node_ports(archive_nodes::ARCHIVE_EXTRACT_KIND)
            .expect("archive_extract is registered");
        assert_eq!(
            extract.output_port(0).unwrap().data_type,
            dag_core::value::PortType::FileSet
        );
    }
}
