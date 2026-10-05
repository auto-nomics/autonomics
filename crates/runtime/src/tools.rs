//! Tool assembly functions for the default agent configuration.
//!
//! Each function returns a [`Vec<ToolRegistration>`]. Compose them
//! to build custom tool sets, or use [`tool_set_from_config`] to let a
//! [`RuntimeConfig`](crate::config::RuntimeConfig) decide which tools to
//! enable.

use std::sync::Arc;

use writing_base::LatexEngine;

use agentik_core::tools::ToolRegistration;
use alphafold::AlphaFoldClient;
use bib_base::BibBase;
use chembl::ChEMBLClient;
use clinicaltrials::ClinicalTrialsClient;
use data_engine::runtime::DataEngineClient;
use gwascatalog_sdk::GwasCatalogClient;
use interpro::InterProClient;
use kegg::KeggClient;
use opengwas::OpengwasClient;
use opentargets::OpenTargetsClient;
use protocolio::ProtocolioClient;
use pubchem::PubChemClient;
use rcsb::RcsbClient;
use string_sdk::StringDbClient;
use vfs::OpendalFileStorage;

use crate::config::RuntimeConfig;
use crate::error::Result;

/// OpenGWAS tools (GWAS catalog lookup).
///
/// Returns [`crate::Error::Opengwas`] if the OpenGWAS client cannot be constructed
/// (typically because `OPENGWAS_TOKEN` is unset).
pub fn opengwas_tools(file_storage: Arc<OpendalFileStorage>) -> Result<Vec<ToolRegistration>> {
    opengwas_tools_with_token(file_storage, None)
}

/// OpenGWAS tools with an explicit API token. Pass `None` to read from
/// the `OPENGWAS_TOKEN` env var (same as [`opengwas_tools`]).
pub fn opengwas_tools_with_token(
    file_storage: Arc<OpendalFileStorage>,
    token: Option<&str>,
) -> Result<Vec<ToolRegistration>> {
    let opengwas = Arc::new(OpengwasClient::new(token)?);
    Ok(opengwas::opengwas_registrations(opengwas, file_storage))
}

/// Open Targets Platform tools (target/disease/drug associations, search).
pub fn opentargets_tools() -> Vec<ToolRegistration> {
    let opentargets = Arc::new(OpenTargetsClient::new());
    opentargets::opentargets_registrations(opentargets)
}

/// ChEMBL tools (molecule/target search, bioactivity previews, and
/// mechanism/indication summaries).
pub fn chembl_tools() -> Vec<ToolRegistration> {
    let chembl = Arc::new(ChEMBLClient::new());
    chembl::chembl_registrations(chembl)
}

/// RCSB PDB tools (structure search, metadata summaries, polymer entities,
/// and bounded structure-file previews).
pub fn rcsb_tools() -> Vec<ToolRegistration> {
    rcsb_tools_with_client(Arc::new(RcsbClient::new()))
}

/// RCSB PDB tools backed by a process-shared client.
pub fn rcsb_tools_with_client(client: Arc<RcsbClient>) -> Vec<ToolRegistration> {
    rcsb::rcsb_registrations(client)
}

/// STRING protein-association tools (identifier resolution, interactions,
/// enrichment, biological summary, and network image preview).
pub fn string_tools() -> Vec<ToolRegistration> {
    let string = Arc::new(
        StringDbClient::builder()
            .build()
            .expect("default STRING client"),
    );
    string_sdk::string_registrations(string)
}

/// KEGG tools (database metadata, entry search/preview, links, ID conversion,
/// and drug interactions). The SDK client enforces KEGG's 3 request/second
/// academic-use limit.
pub fn kegg_tools() -> Vec<ToolRegistration> {
    let client = Arc::new(KeggClient::new());
    kegg::kegg_registrations(client)
}

/// Public biomedical reference APIs in the first resource-expansion batch.
/// None of these clients require credentials or provider-specific SDK setup.
pub fn biomedical_resources_tools() -> Vec<ToolRegistration> {
    let mut tools = Vec::new();
    tools.extend(alphafold::registrations(Arc::new(AlphaFoldClient::new())));
    tools.extend(interpro::registrations(Arc::new(InterProClient::new())));
    tools.extend(pubchem::registrations(Arc::new(PubChemClient::new())));
    tools.extend(clinicaltrials::registrations(Arc::new(
        ClinicalTrialsClient::new(),
    )));
    tools
}

/// GWAS Catalog tools (curated studies, associations, EFO traits, SNPs,
/// unpublished submissions, summary statistics, full summary-stats file
/// download, and Solr full-text search).
pub fn gwascatalog_tools(storage: Arc<OpendalFileStorage>) -> Vec<ToolRegistration> {
    let client = Arc::new(GwasCatalogClient::new());
    gwascatalog_sdk::gwascatalog_registrations(client, storage)
}

/// protocols.io tools (protocol search/details/steps/materials and PDF export).
///
/// The API requires a Bearer token. If `PROTOCOLS_IO_ACCESS_TOKEN` is absent,
/// the tools are disabled rather than failing the entire runtime startup.
pub fn protocolio_tools(storage: Arc<OpendalFileStorage>) -> Vec<ToolRegistration> {
    match ProtocolioClient::new() {
        Ok(client) => {
            let client = Arc::new(client);
            protocolio::tools::registrations(client, storage)
        }
        Err(error) => {
            eprintln!("[runtime] WARNING: protocols.io tools disabled: {error}");
            Vec::new()
        }
    }
}

/// Default on-disk location for the bibliography database, mirroring the
/// TUI's `bib` subcommand default (`--db bib.db`). Overridable via the
/// `AUTONOMICS_BIB_DB` environment variable.
pub const DEFAULT_BIB_DB: &str = "bib.db";

/// Bibliography tools: local-library management (`bib_save` is a DAG node;
/// `bib_create_collection`, …, `bib_export`).
///
/// Literature *retrieval* (search / fetch / citation graph /
/// recommendations) is not a tool anymore — it flows through the DAG
/// evidence channel (`source_literature`, `source_literature_fetch`,
/// `source_literature_citations`, `source_s2_recommendations`).
///
/// `db_path` selects the libSQL file backing [`BibBase`]; pass
/// [`DEFAULT_BIB_DB`] for the conventional location.
///
/// **Note**: this is the single-agent helper. In a multi-agent host,
/// open a [`bib_base::BibShared`] once and reuse its handles — see
/// [`crate::RuntimeHost`]. Each call here builds a fresh
/// `EutilsClient`, `ArxivClient`, `reqwest::Client` and
/// `EuropePmcClient`, which is fine for one agent but wasteful for
/// many.
pub async fn bib_tools(
    db_path: &str,
    file_storage: Arc<OpendalFileStorage>,
) -> Result<Vec<ToolRegistration>> {
    let bib = Arc::new(BibBase::open(db_path).await?);
    Ok(bib_base::bib_all_registrations(bib, file_storage))
}

/// Resolves the bibliography DB path: the `AUTONOMICS_BIB_DB` env var if set,
/// otherwise [`DEFAULT_BIB_DB`].
pub fn resolve_bib_db_path() -> String {
    std::env::var("AUTONOMICS_BIB_DB").unwrap_or_else(|_| DEFAULT_BIB_DB.to_string())
}

/// Default on-disk location for the writing-system database.
pub const DEFAULT_WRITING_DB: &str = "writing.db";

/// Writing tools: document management (doc_create, doc_list, …), editing
/// (doc_insert_section, doc_insert_block, …), citation management
/// (doc_add_citation, doc_check_citations, …), and compilation (doc_compile).
pub async fn writing_tools(db_path: &str) -> Result<Vec<ToolRegistration>> {
    let store = Arc::new(writing_base::WritingStore::open(db_path).await?);
    let engine: Arc<dyn writing_base::LatexEngine> = {
        let x = writing_base::XelatexEngine::new();
        if writing_base::LatexEngine::is_available(&x) {
            Arc::new(x)
        } else {
            Arc::new(writing_base::NullEngine)
        }
    };
    Ok(writing_base::writing_all_registrations(
        store,
        None,
        Some(engine),
    ))
}

/// Resolves the writing DB path: the `AUTONOMICS_WRITING_DB` env var if set,
/// otherwise [`DEFAULT_WRITING_DB`].
pub fn resolve_writing_db_path() -> String {
    std::env::var("AUTONOMICS_WRITING_DB").unwrap_or_else(|_| DEFAULT_WRITING_DB.to_string())
}

/// The complete default tool set: File + OpenGWAS + Open Targets
/// + GWAS Catalog + ChEMBL + RCSB PDB + STRING + DataEngine + Bibliography.
///
/// Pass a shared [`OpendalFileStorage`] used by both the fs tools
/// and the OpenGWAS download tool.
///
/// **Note**: prefer [`tool_set_from_config`] for new code — it respects
/// per-agent feature flags and token overrides.
pub async fn default_tool_set(
    file_storage: Arc<OpendalFileStorage>,
    data_engine_client: Arc<DataEngineClient>,
) -> Result<Vec<ToolRegistration>> {
    let cfg = RuntimeConfig::default();
    tool_set_from_config(file_storage, data_engine_client, &cfg).await
}

/// Build a tool set from a [`RuntimeConfig`], enabling/disabling each
/// tool group according to the config's feature flags and using the
/// config's token overrides.
pub async fn tool_set_from_config(
    file_storage: Arc<OpendalFileStorage>,
    data_engine_client: Arc<DataEngineClient>,
    config: &RuntimeConfig,
) -> Result<Vec<ToolRegistration>> {
    // Filesystem / shell tools — always enabled.
    let mut tools = vfs::vbash_registrations(file_storage.clone());

    if config.enable_opengwas {
        match opengwas_tools_with_token(file_storage.clone(), config.opengwas_token.as_deref()) {
            Ok(opengwas) => tools.extend(opengwas),
            Err(e) => {
                eprintln!("[runtime] WARNING: OpenGWAS tools disabled (token error): {e}");
            }
        }
    }

    if config.enable_opentargets {
        tools.extend(opentargets_tools());
    }

    if config.enable_rcsb {
        tools.extend(rcsb_tools());
    }

    if config.enable_gwascatalog {
        tools.extend(gwascatalog_tools(file_storage.clone()));
    }
    if config.enable_string {
        tools.extend(string_tools());
    }

    if config.enable_chembl {
        tools.extend(chembl_tools());
    }

    if config.enable_kegg {
        tools.extend(kegg_tools());
    }

    tools.extend(biomedical_resources_tools());
    tools.extend(protocolio_tools(file_storage.clone()));

    tools.extend(data_engine_tools::registrations(data_engine_client));

    if config.enable_bibliography {
        match bib_tools(&config.bib_db_path.to_string_lossy(), file_storage.clone()).await {
            Ok(bib) => tools.extend(bib),
            Err(e) => {
                eprintln!("[runtime] WARNING: bibliography tools disabled: {e}");
            }
        }
    }

    if config.enable_writing {
        match writing_tools(&config.writing_db_path.to_string_lossy()).await {
            Ok(wt) => tools.extend(wt),
            Err(e) => {
                eprintln!("[runtime] WARNING: writing tools disabled: {e}");
            }
        }
    }

    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chembl_tools_expose_expected_registrations() {
        let names: Vec<_> = chembl_tools()
            .into_iter()
            .map(|tool| tool.definition.name)
            .collect();
        for name in [
            "chembl_search",
            "chembl_molecule_summary",
            "chembl_target_summary",
            "chembl_activities",
            "chembl_mechanisms",
            "chembl_indications",
        ] {
            assert!(names.contains(&name.to_string()), "missing tool: {name}");
        }
    }

    #[test]
    fn rcsb_tools_register_preview_and_summary_tools() {
        let tools = rcsb_tools();
        let names = tools
            .iter()
            .map(|tool| tool.definition.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"rcsb_search"));
        assert!(names.contains(&"rcsb_entry"));
        assert!(names.contains(&"rcsb_polymer"));
        assert!(names.contains(&"rcsb_structure_preview"));
        assert_eq!(tools.len(), 4);
    }

    #[tokio::test]
    #[ignore = "live RCSB API test"]
    async fn rcsb_entry_tool_executes_through_registration() {
        let tools = rcsb_tools();
        let tool = tools
            .into_iter()
            .find(|tool| tool.definition.name == "rcsb_entry")
            .expect("rcsb_entry registration");

        let result = tool
            .implementation
            .execute(serde_json::json!({ "entry_id": "4HHB" }))
            .await
            .expect("RCSB entry tool should execute");

        assert!(result.is_error.is_none());
        match result.content {
            agentik_sdk::types::ToolResultContent::Text(markdown) => {
                assert!(markdown.contains("4HHB"));
                assert!(markdown.contains("X-RAY DIFFRACTION"));
            }
            other => panic!("expected text result, got {other:?}"),
        }
    }

    #[test]
    fn string_tool_registrations_are_wired() {
        let registrations = string_tools();
        let names: Vec<_> = registrations
            .iter()
            .map(|registration| registration.definition.name.as_str())
            .collect();
        for name in [
            "string_resolve_identifiers",
            "string_network_interactions",
            "string_functional_enrichment",
            "string_network_summary",
            "string_network_image",
        ] {
            assert!(names.contains(&name), "missing tool: {name}");
        }
    }

    #[test]
    fn kegg_tools_are_registered_with_nonempty_schemas() {
        let registrations = kegg_tools();
        let names: Vec<_> = registrations
            .iter()
            .map(|registration| registration.definition.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "kegg_info",
                "kegg_find",
                "kegg_entry_preview",
                "kegg_link",
                "kegg_convert",
                "kegg_ddi"
            ]
        );
        assert!(
            registrations.iter().all(|registration| {
                !registration.definition.input_schema.properties.is_empty()
            })
        );
    }

    #[test]
    fn biomedical_resources_tools_are_registered_with_nonempty_schemas() {
        let registrations = biomedical_resources_tools();
        let names = registrations
            .iter()
            .map(|registration| registration.definition.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "alphafold_lookup",
                "interpro_lookup",
                "pubchem_compound_lookup",
                "clinicaltrials_study_lookup"
            ]
        );
        assert!(
            registrations.iter().all(|registration| !registration
                .definition
                .input_schema
                .properties
                .is_empty())
        );
    }
}
