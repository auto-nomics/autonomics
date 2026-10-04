//! The skill control loop, the **only** mutation path in the daemon:
//!
//! - **Workflow** ([`run_evolution_cycle`]): the idempotent "what
//!   happens" — snapshot observations, distill proposals (create or
//!   update), apply the approval policy. Pure, synchronous; called
//!   only by the worker on this crate. Stays `pub(crate)` so the
//!   auto-approve loop inside the cycle can promote without routing
//!   through the channel (the worker is already on the executor —
//!   a channel round-trip would deadlock).
//! - **Commands** ([`SkillControlHandle`]): the unified control
//!   surface. Every external mutation — evolution triggers,
//!   proposal approve/reject, an explicit `RunCycle` — travels
//!   through one `mpsc::Sender<SkillCommand>` into a single worker
//!   task spawned by [`start`]. Trigger variants coalesce within a
//!   quiet window; inline mutations (approve/reject) and explicit
//!   cycles process immediately with a oneshot reply.
//!
//! ## Why one channel
//!
//! Before this refactor the gateway HTTP handlers and the agent
//! `skill_evolve` tool bypassed the channel with `spawn_blocking`,
//! and the CLI exposed a `distill` / `approve` / `reject` surface
//! that called the manager directly. Every path except the worker's
//! own timer/startup/observation feed had its own control code path.
//! Routing them all through one channel gives a single funnel for
//! mutation ordering, dropped/timeout observability, and the same
//! `try_send`-never-blocks contract the observation forwarder
//! already relies on.
//!
//! ## Reply semantics
//!
//! Mutations that need a result (approve/reject/run-cycle) carry an
//! `oneshot::Sender<SkillCommandOutcome<T>>`. The worker sends
//! exactly one outcome per command: `Ok`, `Err(SkillError)`, or
//! `WorkerGone` (worker exited before replying — distinct fault
//! class that maps to HTTP 503 at the boundary). Triggers are
//! fire-and-forget: a full channel drops the event and bumps the
//! dropped counter, never blocking the caller.
//!
//! ## Safety rails on automation
//!
//! [`EvolutionPolicy`] gates auto-approval separately from
//! distillation: by default the cycle only *writes pending
//! proposals* (human review stays in the loop), and even with
//! `auto_approve` enabled it is capped per cycle, only ever touches
//! loop-created skills (human-owned names are refused at distill
//! time), and every promotion still passes the full validation
//! contract. External approve commands (TUI clicks) are deliberately
//! unbounded — the cap exists to bound *unattended automation*, not
//! deliberate human action.

mod command;
mod handle;
mod model;
mod monitor;
mod worker;
mod workflow;

#[cfg(test)]
mod tests;

pub use command::{SkillCommand, SkillCommandOutcome};
pub use handle::SkillControlHandle;
pub use model::{EvolutionOptions, EvolutionPolicy, EvolutionReport, EvolutionTrigger};
pub use monitor::{CyclePhase, CycleStatus};
pub use worker::start;

pub(crate) use workflow::{run_evolution_cycle, run_evolution_cycle_labeled};
