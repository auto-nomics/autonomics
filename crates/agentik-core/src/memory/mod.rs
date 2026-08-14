//! Persistent cross-session memory for agentik.
//!
//! The design follows Codex's two-phase memory pipeline, adapted to this
//! runtime's model and Turso storage abstractions:
//!
//! - Phase 1 extracts a structured raw memory from an idle persisted session.
//! - Phase 2 consolidates bounded raw memories into a shared file workspace.
//! - A compact summary is injected into the system prompt while detailed
//!   memory remains available through read/search tools.

mod artifacts;
mod backend;
mod pipeline;
mod tools;

pub use artifacts::{
    MemoryArtifacts, MemoryConsolidation, MemoryExtraction, RawMemoryEntry, parse_json_object,
};
pub use backend::{MemoryBackend, memory_prompt_section};
pub use pipeline::{MEMORY_SCOPE_ID, run_memory_pipeline};
pub use tools::memory_registrations;

use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryConfig {
    /// Inject the compact summary and expose read/search memory tools.
    pub use_memory: bool,
    /// Run startup extraction and consolidation for root-level agents.
    pub generate_memory: bool,
    pub root: PathBuf,
    /// Maximum number of most-recent idle sessions considered per startup.
    pub max_source_sessions: usize,
    /// Inputs older than this are ignored.
    pub max_age_days: i64,
    /// Minimum idle time before an unfinished/crashed session is eligible.
    pub min_idle_hours: i64,
    /// Maximum phase-1 records passed to phase 2.
    pub phase2_inputs: usize,
    /// Maximum prompt-loaded summary size, in bytes.
    pub summary_max_bytes: usize,
    /// Job lease duration. Expired leases may be reclaimed by another worker.
    pub lease_seconds: i64,
}

impl MemoryConfig {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            use_memory: true,
            generate_memory: true,
            root: root.into(),
            max_source_sessions: 20,
            max_age_days: 90,
            min_idle_hours: 12,
            phase2_inputs: 100,
            summary_max_bytes: 16 * 1024,
            lease_seconds: 3600,
        }
    }

    #[must_use]
    pub fn is_root_agent(&self, path: &agentik_types::AgentPath) -> bool {
        // `/root/name` is a root-level agent; delegated children have at
        // least one more path segment (`/root/name/child`).
        path.segments().len() <= 2
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
