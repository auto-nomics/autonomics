#[derive(Debug)]
pub enum SkillCommandOutcome<T> {
    /// Operation succeeded with the produced value.
    Ok(T),
    /// Operation failed with a domain error (bad proposal name,
    /// validation refused, etc.). Maps to 400/404 at the HTTP layer.
    Err(SkillError),
    /// Worker exited before this command was processed. Maps to 503.
    WorkerGone,
}

impl<T> SkillCommandOutcome<T> {
    pub fn is_ok(&self) -> bool {
        matches!(self, SkillCommandOutcome::Ok(_))
    }
}

/// One unit of work the worker processes. The worker owns the
/// `mpsc::Receiver`; every external mutation travels through this
/// enum.
///
/// Trigger variants are fire-and-forget — the worker never replies.
/// Mutation variants carry an `oneshot::Sender`; the worker sends
/// exactly one `SkillCommandOutcome<T>` back before dropping the
/// command (or `WorkerGone` if it shut down first).
pub enum SkillCommand {
    /// Coalesced with other `Evolution` variants in the quiet window
    /// before one cycle. The four sub-variants come straight from
    /// [`EvolutionTrigger`].
    Evolution(EvolutionTrigger),
    /// Process a pending proposal approval inline (between batch
    /// collection and cycle run). The worker replies with
    /// `ApproveOutcome` on success.
    ApproveProposal {
        name: String,
        reply: oneshot::Sender<SkillCommandOutcome<ApproveOutcome>>,
    },
    /// Process a pending proposal rejection inline. Replies with
    /// the updated [`Proposal`].
    RejectProposal {
        name: String,
        reply: oneshot::Sender<SkillCommandOutcome<Proposal>>,
    },
    /// Run a full cycle now (no coalescing) with the supplied
    /// policy. Used by the gateway `POST /skills/evolution/trigger`
    /// handler and the agent `skill_evolve` tool. Replies with the
    /// produced [`EvolutionReport`].
    RunCycle {
        policy: EvolutionPolicy,
        reply: oneshot::Sender<SkillCommandOutcome<EvolutionReport>>,
    },
}

/// Cheap, cloneable handle for every skill mutation in the daemon.
/// `try_send` semantics for the trigger path: a full channel drops
/// the event and bumps the dropped counter, never blocking the
/// caller (the same "subscriber cannot break the mutation path"
/// contract the manager's broadcast carries). The `call_*` methods
/// await a oneshot reply and are intended for request/response
/// callers (HTTP handlers, the agent tool).
use tokio::sync::oneshot;

use crate::error::SkillError;
use crate::proposals::{ApproveOutcome, Proposal};

use super::model::{EvolutionPolicy, EvolutionReport, EvolutionTrigger};
