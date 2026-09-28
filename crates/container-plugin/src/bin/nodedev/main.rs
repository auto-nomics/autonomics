mod init_command;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Scaffold a new container-plugin manifest family.
    Init(init_command::InitArgs),
}

fn main() {
    if let Err(error) = run() {
        // Display (thiserror's message), not Debug: the user needs the
        // failure reason, not the enum shape. Exit code 1 for scripts/CI.
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> container_plugin::error::Result<()> {
    match Cli::parse().command {
        Commands::Init(init) => init_command::init_plugin_project(&init.plugin_name, init.dir_path),
    }
}
