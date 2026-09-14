//! `autonomics serve` — the resident backend gateway daemon.
//!
//! - `autonomics serve` — run in the foreground (logs to stderr + state-dir file;
//!   Ctrl+C stops it gracefully);
//! - `autonomics serve --daemon` — detached mode, spawned by
//!   `gateway::manager::ensure_running` (null stdio, own process group,
//!   logs to the state-dir file only);
//! - `autonomics serve status` / `autonomics serve stop` — inspect / gracefully stop a
//!   running daemon.

use clap::Subcommand;

#[derive(Debug, clap::Args)]
pub struct ServeArgs {
    /// Run detached from the terminal (used by the auto-spawn path).
    #[arg(long)]
    pub daemon: bool,

    #[command(subcommand)]
    pub action: Option<ServeAction>,
}

#[derive(Debug, Subcommand)]
pub enum ServeAction {
    /// Print the running daemon's status (pid, uptime, agent count).
    Status,
    /// Ask a running daemon to shut down gracefully.
    Stop,
}
