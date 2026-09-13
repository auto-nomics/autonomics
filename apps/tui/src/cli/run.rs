//! Arguments for the headless run subcommand.

use clap::Args;

#[derive(Debug, Args)]
pub struct RunArgs {
    pub prompt: String,
}
