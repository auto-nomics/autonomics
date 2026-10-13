//! `autonomics serve` — start the resident backend gateway daemon.
//!
//! - `autonomics serve` — spawn the daemon detached (own session, stdio
//!   closed), wait until it is ready, print its status, and return;
//!   the daemon keeps running after the CLI exits;
//! - `autonomics serve status` / `autonomics serve stop` — inspect /
//!   gracefully stop a running daemon.
//!
//! Logs always live under the state dir (`logs/autonomics-gateway.log`);
//! there is no foreground mode.

use clap::Subcommand;

#[derive(Debug, clap::Args)]
pub struct ServeArgs {
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
