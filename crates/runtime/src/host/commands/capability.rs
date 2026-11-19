//! Capability extraction: deriving the advertised
//! [`AgentInfo`](crate::control::AgentInfo) (tags, expertise, summary)
//! from an [`AgentKind`]'s feature flags.

use agentik_core::profile::AgentKind;

/// Build [`AgentInfo`](crate::control::AgentInfo) from an [`AgentKind`],
/// auto-extracting tags, expertise, and tool list from the kind's feature
/// flags.
pub(in crate::host) fn capability_from_kind(
    name: &str,
    path: &str,
    kind: AgentKind,
) -> crate::control::AgentInfo {
    let mut tags = Vec::new();
    let mut expertise = Vec::new();

    if kind == AgentKind::Researcher {
        tags.push("research".into());
        tags.push("analysis".into());
        tags.push("dag-analysis".into());
        tags.push("data-analysis".into());
        tags.push("dag-execution".into());
        expertise.push("research-analysis".into());
        expertise.push("dag-workflow-analysis".into());
        expertise.push("scientific-interpretation".into());
    }

    if kind.enable_bibliography() {
        tags.push("literature".into());
        tags.push("bibliography".into());
        expertise.push("literature-search".into());
        expertise.push("citation-management".into());
    }
    if kind.enable_writing() {
        tags.push("writing".into());
        tags.push("latex".into());
        tags.push("manuscript".into());
        expertise.push("document-editing".into());
        expertise.push("citation-insertion".into());
        expertise.push("latex-compilation".into());
    }
    if kind.enable_opengwas() {
        tags.push("gwas".into());
        tags.push("genetics".into());
        expertise.push("gwas-analysis".into());
    }
    if kind.enable_opentargets() {
        tags.push("drug-targets".into());
        expertise.push("target-identification".into());
    }
    if kind.enable_gwascatalog() {
        tags.push("gwas-catalog".into());
        expertise.push("variant-lookup".into());
    }
    if kind.enable_chembl() {
        tags.push("chembl".into());
        expertise.push("bioactivity-data".into());
    }
    if kind.enable_rcsb() {
        tags.push("pdb".into());
        tags.push("structural-biology".into());
        expertise.push("protein-structure-lookup".into());
        expertise.push("structural-biology-analysis".into());
    }
    if kind.enable_string() {
        tags.push("protein-networks".into());
        expertise.push("string-analysis".into());
    }
    if kind.enable_kegg() {
        tags.push("kegg".into());
        expertise.push("pathway-analysis".into());
    }
    if kind.enable_dag_history() {
        tags.push("pipeline".into());
        expertise.push("dag-execution".into());
    }
    if kind.enable_wiki_read() {
        tags.push("wiki".into());
        expertise.push("knowledge-lookup".into());
    }
    if kind.enable_wiki_write() {
        tags.push("knowledge".into());
        tags.push("knowledge-base".into());
        tags.push("curation".into());
        tags.push("note-taking".into());
        expertise.push("knowledge-management".into());
        expertise.push("wiki-curation".into());
        expertise.push("documentation".into());
    }
    if kind.enable_plugin_rsi() {
        tags.push("plugin".into());
        tags.push("node".into());
        tags.push("node-development".into());
        tags.push("plugin-validation".into());
        expertise.push("plugin-development".into());
        expertise.push("node-development".into());
        expertise.push("plugin-lifecycle".into());
        expertise.push("plugin-validation".into());
    }
    if kind.enable_browser() {
        tags.push("browser".into());
        tags.push("web".into());
        tags.push("web-browsing".into());
        expertise.push("web-navigation".into());
        expertise.push("web-extraction".into());
        expertise.push("web-interaction".into());
    }

    crate::control::AgentInfo {
        name: name.into(),
        path: path.into(),
        agent_id: None,
        summary: kind.description().into(),
        tags,
        expertise,
        tools: Vec::new(), // populated at runtime if needed
        status: agentik_types::AgentLifecycleStatus::Idle,
        last_event: None,
        model: None,
        context_length: None,
    }
}
