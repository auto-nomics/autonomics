//! Skill system for autonomics.
//!
//! A **skill** is a directory with a `SKILL.md` (YAML frontmatter +
//! operational body), following the same contract as the Anthropic
//! `skills` and EvoScientist `EvoSkills` ecosystems so third-party
//! packs install unchanged.
//!
//! Layout:
//!
//! - [`format`] — parse and validate SKILL.md
//! - [`registry`] — tiered scan (builtin / global / workspace) with
//!   name-based shadowing; always fresh, never cached
//! - [`manager`] — the central process-wide manager: generation
//!   counter, change broadcast on every mutation, usage telemetry
//!   (the evolution fitness signal)
//! - [`evolution`] — the unified control loop: every skill mutation
//!   (evolution triggers, proposal approve/reject, explicit cycle)
//!   travels through one `mpsc::Sender<SkillCommand>` into a single
//!   worker task; the worker coalesces trigger bursts in a quiet
//!   window and replies to inline mutations via oneshot. The cycle
//!   body itself stays `pub(crate)` and the worker calls it directly.
//! - [`observation`] — the distillation feedstock: durable
//!   content-hashed records of failures/fixes/recipes
//! - [`distill`] — deterministic clustering and proposal synthesis
//!   (the RSI loop's no-LLM core)
//! - [`proposals`] — the staging area and approve/reject lifecycle
//!   between distillation and the live library
//! - [`inject`] — the one-line-per-skill system-prompt index
//! - [`tools`] — skill discovery and authoring tools; the runtime layers the
//!   unified `evo_observe` entry over this crate's shared observation store
//! - [`workflow`] — parameterized DAG templates (`workflow/*.toml`):
//!   parse, validate, render with checked params
//! - [`eval`] — eval cases (`evals/*.toml`): run a workflow with fixed
//!   params and check the run report
//! - [`install`] — install from local paths or git, with a
//!   `.installed.toml` provenance sidecar per tier
//! - [`builtin`] — skills compiled into the binary
//!
//! Skills are surfaced to agents through structured tools, not the
//! VFS: the tools bound pagination and size caps and keep a single
//! access point for future usage telemetry.

pub mod builtin;
pub mod distill;
pub mod embedding;
pub mod error;
pub mod eval;
pub mod evolution;
pub mod format;
pub mod inject;
pub mod install;
pub mod manager;
pub mod observation;
pub mod proposals;
pub mod registry;
pub mod tools;
pub mod workflow;

pub use distill::{Candidate, DistillReport, distill};
pub use error::SkillError;
pub use eval::{Check, EvalCase, EvalReport};
pub use evolution::{
    CyclePhase, CycleStatus, EvolutionOptions, EvolutionPolicy, EvolutionReport, EvolutionTrigger,
    SkillCommand, SkillCommandOutcome, SkillControlHandle, start,
};
pub use format::SkillMeta;
pub use inject::prompt_section;
pub use manager::{SkillManager, UsageKind, UsageRecord};
pub use observation::{
    Observation, ObservationInput, ObservationKind, ObservationSource, ObservationStore,
};
pub use proposals::{ApproveOutcome, Proposal, ProposalStatus, Proposals};
pub use registry::{SkillDocument, SkillEntry, SkillRegistry, SkillTier};
pub use tools::skill_registrations;
pub use workflow::{RenderedWorkflow, WorkflowTemplate};

/// Resolve the default skills state directory: `$AUTONOMICS_STATE_DIR`
/// or `~/.autonomics`, mirroring the runtime config precedence.
pub fn default_state_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("AUTONOMICS_STATE_DIR")
        && !dir.trim().is_empty()
    {
        return std::path::PathBuf::from(dir);
    }
    home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".autonomics")
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_dir_honors_env_then_home() {
        // Not asserting the exact value (env-dependent); just that the
        // env override wins when set.
        let saved = std::env::var("AUTONOMICS_STATE_DIR").ok();
        unsafe { std::env::set_var("AUTONOMICS_STATE_DIR", "/tmp/skills-test-state") };
        assert_eq!(
            default_state_dir(),
            std::path::PathBuf::from("/tmp/skills-test-state")
        );
        match saved {
            Some(v) => unsafe { std::env::set_var("AUTONOMICS_STATE_DIR", v) },
            None => unsafe { std::env::remove_var("AUTONOMICS_STATE_DIR") },
        }
    }
}
