//! CLI argument definitions for `autonomics`.
//!
//! Kept separate from command implementations so subcommand behavior can be
//! changed without touching clap metadata.
//!
//! Layout mirrors `commands/`: each subcommand's clap types live in a sibling
//! module here (`cli/tui.rs`, `cli/bib.rs`, …), paired with its implementation
//! module (`commands/tui.rs`, `commands/bib.rs`, …). Adding a subcommand means
//! adding one module on each side.
//!
//! All argument types are re-exported at this module's root so importers can
//! keep using flat paths like `tui::cli::{Command, TuiArgs}`.

mod bib;
mod cache;
mod export;
mod kms;
mod panels;
mod run;
mod serve;
mod tui;

use clap::{Parser, Subcommand};

pub use bib::{
    BibAction, BibArgs, ExportArgs, InfoArgs, ListArgs, ReextractArgs, RequestsArgs, UploadArgs,
};
pub use cache::{CacheAction, CacheArgs, ClearOpengwasArgs, RefreshOpengwasArgs};
pub use export::ExportRunArgs;
pub use kms::KmsArgs;
pub use panels::{PanelsAction, PanelsArgs};
pub use run::RunArgs;
pub use serve::{ServeAction, ServeArgs};
pub use tui::TuiArgs;

#[derive(Debug, Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch the interactive TUI (default when no subcommand is given).
    Tui(TuiArgs),

    /// Launch the dedicated KMS tree TUI.
    Kms(KmsArgs),

    /// Local cache management helpers (refresh, inspect, purge).
    Cache(CacheArgs),

    /// Provision plugin panel data bundles (see `sync`).
    Panels(PanelsArgs),

    /// Bibliography management — upload full-text PDFs, list pending requests.
    Bib(BibArgs),

    /// Export a recorded DAG run as provenance evidence
    /// (RO-Crate directory / W3C PROV-JSON).
    ExportRun(ExportRunArgs),

    /// Headless mode
    Run(RunArgs),

    /// Run the resident backend gateway daemon (agents keep running
    /// after frontends disconnect). See `status` / `stop` actions.
    Serve(ServeArgs),
}
