//! `workflow-editor` CLI entry point.
//!
//!
//!
//! Phase 0: only `--check <file>` exists — validates a JSON workflow
//! manifest. Phase 2 adds the `tui` subcommand.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(name = "workflow-editor", version, about = "Visual workflow editor with skills = DAG subgraphs.", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Validate a JSON workflow manifest against the schema and basic invariants.
    Check {
        /// Path to a JSON file containing a `WorkflowManifest`.
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Launch the TUI editor (Phase 2).
    Tui,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.cmd {
        None | Some(Cmd::Tui) => {
            eprintln!("TUI subcommand lands in Phase 2. Use `check <file>` for now.");
            ExitCode::from(2)
        }
        Some(Cmd::Check { file }) => match check(&file) {
            Ok(()) => {
                println!("OK: {}", file.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("FAIL: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn check(path: &std::path::Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    let m: workflow_editor::model::WorkflowManifest = serde_json::from_slice(&bytes)?;
    m.validate()?;
    Ok(())
}