use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Execute analysis tasks on compute nodes")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the worker protocol, source revision and executable SHA256 as JSON.
    Identity {
        #[arg(long)]
        plugins_root: Option<PathBuf>,
    },
    /// Execute one task supplied by the coordinator transport.
    TaskWorker {
        #[arg(long)]
        dispatch: PathBuf,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let result = match cli.command {
        Command::Identity { plugins_root } => {
            autonomics_worker::identity_report(plugins_root.as_deref())
                .map(|report| println!("{report}"))
        }
        Command::TaskWorker { dispatch } => autonomics_worker::run(&dispatch).await,
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("worker failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
