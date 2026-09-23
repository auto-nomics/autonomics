mod cli;
mod commands;
mod common;

use clap::Parser;
use data_catalog::error::Result;

use cli::{Cli, Command};

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Build(args) => commands::build::run_build(args),
        Command::Validate(args) => commands::build::run_validate(args),
        Command::Publish(args) => commands::publish::run(args).await,
        Command::Search(args) => commands::search::run(args).await,
        Command::Install(args) => commands::install::run_install(args).await,
        Command::Update(args) => commands::install::run_update(args).await,
        Command::Migrate(args) => commands::migrate::run(args).await,
        Command::List(args) => commands::inspect::run_list(args),
        Command::Mounts(args) => commands::inspect::run_mounts(args),
        Command::Init(args) => commands::init::run_init(args),
    }
}
