//! Internal events for decoupled communication between subsystems and the
//! main TUI loop. Modeled after codex's `AppEvent` + `AppEventSender` pattern.
//!
//! Components that don't have direct access to the `App` struct (e.g. future
//! file search, plugin watchers, background tasks) can push events through
//! an `AppEventSender` which the main loop drains each tick.

/// Events that can be processed by the main TUI loop.
///
/// Currently unused — this is scaffolding for decoupled subsystem→main-loop
/// communication (file search, plugin watchers, background tasks) modelled
/// after codex's `AppEvent` pattern. Kept so the channel plumbing in `App`
/// stays type-checked as it gets wired up.
#[allow(dead_code)]
pub(crate) enum AppEvent {
    /// An event from the agent runtime (text delta, tool call, etc.).
    ///
    /// Boxed because `AgentEvent` is large (it carries a full `Message`),
    /// which would otherwise dominate the enum size — see
    /// `clippy::large_enum_variant`.
    Agent(Box<agentik_sdk::types::AgentEvent>),
    /// Request to exit the application.
    Quit,
    /// Config data changed; the Config tab should reload from the database.
    ConfigReload,
    /// A new agent was spawned (or failed to spawn) from a profile.
    /// Sent by the background tokio task spawned in
    /// `App::spawn_agent_from_profile`.
    AgentSpawned {
        profile_name: String,
        result: std::result::Result<runtime::AgentHandle, String>,
    },
}
