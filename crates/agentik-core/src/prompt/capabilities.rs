//! Tool-capability trait gating which system-prompt sections an agent gets.

/// Trait so [`build_system_prompt`] can accept either an
/// [`agentik_core::AgentKind`] or a [`RuntimeConfig`] — both carry the same
/// boolean tool-capability flags.
pub trait PromptCapabilities {
    fn enable_bibliography(&self) -> bool;
    fn enable_opengwas(&self) -> bool;
    fn enable_opentargets(&self) -> bool;
    fn enable_gwascatalog(&self) -> bool;
    fn enable_chembl(&self) -> bool;
    fn enable_rcsb(&self) -> bool;
    fn enable_string(&self) -> bool;
    fn enable_kegg(&self) -> bool;
    fn enable_dag_history(&self) -> bool;
    fn enable_plugin_rsi(&self) -> bool {
        false
    }
    fn enable_browser(&self) -> bool {
        false
    }
    fn enable_resource_downloads(&self) -> bool {
        false
    }
    /// Read-only wiki tools ship with every agent kind, so the section
    /// defaults on (mirrors the unconditional tool mount).
    fn enable_wiki_read(&self) -> bool {
        true
    }
    // Wiki write tools are keyed on the agent *kind* (curator only),
    // never on the global runtime config.
    fn enable_wiki_write(&self) -> bool {
        false
    }
    // HPC tools are keyed on the agent *kind* (hpc-ops only), never on
    // the global runtime config.
    fn enable_hpc(&self) -> bool {
        false
    }
}

// AgentKind carries the same tool-capability flags as the runtime config, so
// we can build a dynamic system prompt that only mentions tools the kind
// actually enables.
impl PromptCapabilities for crate::profile::AgentKind {
    fn enable_bibliography(&self) -> bool {
        (*self).enable_bibliography()
    }
    fn enable_opengwas(&self) -> bool {
        (*self).enable_opengwas()
    }
    fn enable_opentargets(&self) -> bool {
        (*self).enable_opentargets()
    }
    fn enable_gwascatalog(&self) -> bool {
        (*self).enable_gwascatalog()
    }
    fn enable_chembl(&self) -> bool {
        (*self).enable_chembl()
    }
    fn enable_rcsb(&self) -> bool {
        (*self).enable_rcsb()
    }
    fn enable_string(&self) -> bool {
        (*self).enable_string()
    }
    fn enable_kegg(&self) -> bool {
        (*self).enable_kegg()
    }
    fn enable_dag_history(&self) -> bool {
        (*self).enable_dag_history()
    }
    fn enable_plugin_rsi(&self) -> bool {
        (*self).enable_plugin_rsi()
    }
    fn enable_browser(&self) -> bool {
        (*self).enable_browser()
    }
    fn enable_resource_downloads(&self) -> bool {
        (*self).enable_resource_downloads()
    }
    fn enable_wiki_read(&self) -> bool {
        (*self).enable_wiki_read()
    }
    fn enable_wiki_write(&self) -> bool {
        (*self).enable_wiki_write()
    }
    fn enable_hpc(&self) -> bool {
        (*self).enable_hpc()
    }
}
