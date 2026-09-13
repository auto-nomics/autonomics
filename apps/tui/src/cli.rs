//! CLI argument definitions for `autonomics-tui`.
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
mod kms;
mod run;
mod tui;

use clap::{Parser, Subcommand};

pub use bib::{BibAction, BibArgs, ExportArgs, InfoArgs, ListArgs, RequestsArgs, UploadArgs};
pub use cache::{CacheAction, CacheArgs, ClearOpengwasArgs, RefreshOpengwasArgs};
pub use kms::KmsArgs;
pub use run::RunArgs;
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

    /// Bibliography management — upload full-text PDFs, list pending requests.
    Bib(BibArgs),

    /// Headless mode
    Run(RunArgs),
}
