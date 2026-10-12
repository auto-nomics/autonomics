//! `autonomics serve` — the resident backend gateway daemon.
//!
//! - `autonomics serve` — run in the foreground (logs to stderr + state-dir file;
//!   Ctrl+C stops it gracefully);
//! - `autonomics serve --daemon` — detached mode (logs to the
//!   state-dir file only; stdio detaches once the server is bound);
//! - `autonomics serve status` / `autonomics serve stop` — inspect / gracefully stop a
//!   running daemon.

use clap::Subcommand;

#[derive(Debug, clap::Args)]
pub struct ServeArgs {
    /// Run detached from the terminal; logs go to the state-dir
    /// gateway log only.
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
