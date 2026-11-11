//! Runtime-side prompt wiring.
//!
//! The section constants, [`PromptCapabilities`], and
//! [`build_system_prompt`] live in `agentik_core::prompt` (merged there so
//! prompt assembly has a single home); this module keeps the
//! `RuntimeConfig` capability impl and re-exports the core API so existing
//! `runtime::config::` paths keep working.

pub use agentik_core::prompt::{PromptCapabilities, build_system_prompt, default_system_prompt};

use super::runtime::RuntimeConfig;

impl PromptCapabilities for RuntimeConfig {
    fn enable_bibliography(&self) -> bool {
        self.enable_bibliography
    }
    fn enable_opengwas(&self) -> bool {
        self.enable_opengwas
    }
    fn enable_opentargets(&self) -> bool {
        self.enable_opentargets
    }
    fn enable_gwascatalog(&self) -> bool {
        self.enable_gwascatalog
    }
    fn enable_chembl(&self) -> bool {
        self.enable_chembl
    }
    fn enable_rcsb(&self) -> bool {
        self.enable_rcsb
    }
    fn enable_string(&self) -> bool {
        self.enable_string
    }
    fn enable_kegg(&self) -> bool {
        self.enable_kegg
    }
    fn enable_dag_history(&self) -> bool {
        self.enable_dag_history
    }
    // `enable_browser` is keyed on the agent *kind*, not the global
    // runtime config — the `AgentKind` impl of this trait is what
    // actually drives the system-prompt section. Returning `false` here
    // keeps the default-prompt path aligned with the legacy behavior.
}
