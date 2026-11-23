//! System-prompt section constants and the capability-gated dynamic builder.
//! Merged from the former runtime::config::prompt — see crate::prompt for the
//! full module story.

use super::capabilities::PromptCapabilities;

// ---------------------------------------------------------------------------
// System-prompt section constants
// ---------------------------------------------------------------------------
//
// Each section is a self-contained `&str` so the dynamic builder can include
// only the sections that match the agent profile's enabled tool groups.

const PROMPT_HEADER: &str = include_str!("system_prompt/sections/header.md");

const PROMPT_BIBLIOGRAPHY: &str =
    concat!("\n", include_str!("system_prompt/sections/bibliography.md"));

const PROMPT_OPENGWAS: &str = concat!("\n", include_str!("system_prompt/sections/opengwas.md"));

const PROMPT_OPENTARGETS: &str =
    concat!("\n", include_str!("system_prompt/sections/opentargets.md"));

const PROMPT_GWASCATALOG: &str =
    concat!("\n", include_str!("system_prompt/sections/gwascatalog.md"));

const PROMPT_CHEMBL: &str = concat!("\n", include_str!("system_prompt/sections/chembl.md"));

const PROMPT_RCSB: &str = include_str!("system_prompt/sections/rcsb.md");

const PROMPT_STRING: &str = concat!("\n", include_str!("system_prompt/sections/string.md"));
const PROMPT_KEGG: &str = concat!("\n", include_str!("system_prompt/sections/kegg.md"));

const PROMPT_BIOMEDICAL_RESOURCES: &str = concat!(
    "\n",
    include_str!("system_prompt/sections/biomedical_resources.md")
);

const PROMPT_WIKI_READ: &str = concat!("\n", include_str!("system_prompt/sections/wiki_read.md"));

const PROMPT_WIKI_WRITE: &str = concat!("\n", include_str!("system_prompt/sections/wiki_write.md"));

const PROMPT_RESOURCE_DOWNLOADS: &str = concat!(
    "\n",
    include_str!("system_prompt/sections/resource_downloads.md")
);

const PROMPT_DAG_ENGINE: &str = concat!("\n", include_str!("system_prompt/sections/dag_engine.md"));

const PROMPT_DAG_HISTORY: &str =
    concat!("\n", include_str!("system_prompt/sections/dag_history.md"));

const PROMPT_PLUGIN_RSI: &str = concat!("\n", include_str!("system_prompt/sections/plugin_rsi.md"));

const PROMPT_RESEARCHER_DEVELOPMENT_HANDOFF: &str = concat!(
    "\n",
    include_str!("system_prompt/sections/researcher_development_handoff.md")
);

const PROMPT_DEVELOPER_HANDOFF: &str = concat!(
    "\n",
    include_str!("system_prompt/sections/developer_handoff.md")
);

const PROMPT_BROWSER: &str = concat!("\n", include_str!("system_prompt/sections/browser.md"));

const PROMPT_BROWSER_HANDOFF: &str = concat!(
    "\n",
    include_str!("system_prompt/sections/browser_handoff.md")
);

const PROMPT_SQL_CONVENTIONS: &str = concat!(
    "\n",
    include_str!("system_prompt/sections/sql_conventions.md")
);

const PROMPT_FILESYSTEM: &str = concat!("\n", include_str!("system_prompt/sections/filesystem.md"));

const PROMPT_GENERAL: &str = concat!("\n", include_str!("system_prompt/sections/general.md"));

const PROMPT_HPC: &str = concat!("\n", include_str!("system_prompt/sections/hpc.md"));

/// Build the default system prompt dynamically, including only the sections for
/// tool groups that are actually enabled. This prevents the prompt from
/// advertising tools the agent cannot access.
pub fn build_system_prompt<C: PromptCapabilities>(caps: &C) -> String {
    let mut s = String::from(PROMPT_HEADER);

    if caps.enable_bibliography() {
        s.push_str(PROMPT_BIBLIOGRAPHY);
    }
    if caps.enable_opengwas() {
        s.push_str(PROMPT_OPENGWAS);
    }
    if caps.enable_opentargets() {
        s.push_str(PROMPT_OPENTARGETS);
    }
    if caps.enable_gwascatalog() {
        s.push_str(PROMPT_GWASCATALOG);
    }
    if caps.enable_chembl() {
        s.push_str(PROMPT_CHEMBL);
    }
    if caps.enable_rcsb() {
        s.push_str(PROMPT_RCSB);
    }
    if caps.enable_string() {
        s.push_str(PROMPT_STRING);
    }
    if caps.enable_kegg() {
        s.push_str(PROMPT_KEGG);
    }
    if caps.enable_bibliography()
        || caps.enable_opengwas()
        || caps.enable_opentargets()
        || caps.enable_gwascatalog()
        || caps.enable_chembl()
        || caps.enable_rcsb()
        || caps.enable_string()
        || caps.enable_kegg()
    {
        s.push_str(PROMPT_BIOMEDICAL_RESOURCES);
    }

    if caps.enable_resource_downloads() {
        s.push_str(PROMPT_RESOURCE_DOWNLOADS);
    }

    if caps.enable_wiki_read() {
        s.push_str(PROMPT_WIKI_READ);
    }
    if caps.enable_wiki_write() {
        s.push_str(PROMPT_WIKI_WRITE);
    }
    if caps.enable_hpc() {
        s.push_str(PROMPT_HPC);
    }

    // DAG engine, SQL conventions, and general sections are always included —
    // the data-engine tools and filesystem tools are always registered.
    s.push_str(PROMPT_DAG_ENGINE);

    if caps.enable_dag_history() {
        s.push_str(PROMPT_DAG_HISTORY);
    }
    if caps.enable_plugin_rsi() {
        s.push_str(PROMPT_PLUGIN_RSI);
        s.push_str(PROMPT_DEVELOPER_HANDOFF);
    } else if caps.is_illustrator() {
        s.push_str("\n### Illustration Collaboration\nYou own editable figure composition and visual refinement. Ask the researcher for missing scientific conclusions or analysis; use delegate_to with profile_segment=\"researcher\" when appropriate. Delegate plugin implementation to profile_segment=\"developer\". Work from the active canvas context, apply edits directly to the current canvas, inspect its preview, and preserve source attribution.\n");
    } else if !caps.enable_browser() {
        // Browser agents participate in neither collaboration: they get
        // their own handoff section below instead of the
        // researcher/developer material.
        s.push_str(PROMPT_RESEARCHER_DEVELOPMENT_HANDOFF);
    }

    if caps.enable_browser() {
        s.push_str(PROMPT_BROWSER);
        s.push_str(PROMPT_BROWSER_HANDOFF);
    }

    s.push_str(PROMPT_SQL_CONVENTIONS);

    s.push_str(PROMPT_FILESYSTEM);
    s.push_str(PROMPT_GENERAL);
    s
}

/// The full default system prompt with all sections enabled.
///
/// Prefer [`build_system_prompt`] when you have an
/// [`AgentKind`](crate::profile::AgentKind) or a runtime config — that function omits
/// sections for disabled tool groups.
pub fn default_system_prompt() -> String {
    struct AllEnabled;
    impl PromptCapabilities for AllEnabled {
        fn enable_bibliography(&self) -> bool {
            true
        }
        fn enable_opengwas(&self) -> bool {
            true
        }
        fn enable_opentargets(&self) -> bool {
            true
        }
        fn enable_gwascatalog(&self) -> bool {
            true
        }
        fn enable_chembl(&self) -> bool {
            true
        }
        fn enable_rcsb(&self) -> bool {
            true
        }
        fn enable_string(&self) -> bool {
            true
        }
        fn enable_kegg(&self) -> bool {
            true
        }
        fn enable_dag_history(&self) -> bool {
            true
        }
        fn enable_plugin_rsi(&self) -> bool {
            true
        }
        fn enable_browser(&self) -> bool {
            true
        }
        fn enable_wiki_write(&self) -> bool {
            true
        }
        fn enable_hpc(&self) -> bool {
            true
        }
    }
    build_system_prompt(&AllEnabled)
}

#[cfg(test)]
mod filesystem_prompt_tests {
    #[test]
    fn every_agent_kind_gets_canonical_filesystem_guidance() {
        for kind in crate::profile::AgentKind::ALL {
            let prompt = super::build_system_prompt(&kind);
            assert!(prompt.contains("### Filesystem domains"), "{kind:?}");
            assert!(prompt.contains("local://work/input.tsv"));
            assert!(prompt.contains("not a hostname"));
            assert!(prompt.to_ascii_lowercase().contains("permission submounts"));
            assert!(!prompt.contains("over `vfs://` paths"));
            assert!(!prompt.contains("`/browser-downloads"));
        }
    }
}

#[cfg(test)]
mod wiki_entry_content_tests {
    use crate::profile::AgentKind;

    /// The curator is the only role that writes entries, so it is the only
    /// one that gets the entry-content rules.
    #[test]
    fn curator_gets_entry_content_rules() {
        let prompt = super::build_system_prompt(&AgentKind::Curator);
        assert!(prompt.contains("### Entry content rules"), "{prompt}");
        assert!(
            prompt.contains("Facts and pointers, not a work log"),
            "{prompt}"
        );
        assert!(prompt.contains("No first person"), "{prompt}");
        assert!(
            prompt.contains("Structure carries the hierarchy"),
            "{prompt}"
        );
    }

    /// Non-curators propose entry text to the curator, so they must not be
    /// handed the write-path rules (they have no write tools).
    #[test]
    fn non_curator_kinds_do_not_get_entry_content_rules() {
        for kind in AgentKind::ALL {
            if kind == AgentKind::Curator {
                continue;
            }
            let prompt = super::build_system_prompt(&kind);
            assert!(
                !prompt.contains("### Entry content rules"),
                "{kind:?} must not receive write-path rules"
            );
        }
    }
}
