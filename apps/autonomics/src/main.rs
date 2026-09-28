use std::path::PathBuf;

use clap::Parser;
use cli::{CacheAction, Cli, Command, TuiArgs};
use time::macros::format_description;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::fmt::writer::MakeWriterExt;

// Pure binary layout: all application modules are declared here. There is no
// library target — reusable logic belongs in crates/ (e.g. `runtime`,
// `agentik-core`), not in this app.
mod app;
mod app_event;
mod app_event_sender;
mod cli;
mod clipboard_copy;
mod commands;
mod kms_tui;
mod state;
mod widgets;
mod xai_textarea;

fn init_logging(nocapture: bool) -> color_eyre::Result<()> {
    color_eyre::install()?;

    let log_dir = PathBuf::from("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "autonomics.log");
    let file_writer = file_appender.with_max_level(Level::DEBUG);

    let timer = OffsetTime::new(
        time::UtcOffset::current_local_offset().expect("timezone"),
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"),
    );

    // The TUI's `set_panic_hook` (app::terminal) wraps this hook and handles
    // terminal restoration + readable stderr output. Here we only need
    // to log the full panic + backtrace to the log file. The readable
    // message to stderr is handled by `set_panic_hook`.
    std::panic::set_hook(Box::new(|info| {
        // Recoverable extraction panics (pdf-extract on malformed fonts)
        // are handled at the source; keep CLI output quiet for them.
        if bib_base::is_expected_panic() {
            tracing::warn!(payload = %info, "suppressed recoverable extraction panic");
            return;
        }

        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            target: "panic",
            payload = %info,
            backtrace = %bt,
            "thread panicked"
        );
        // Directly write to stderr + flush so the panic is visible even if
        // the tracing subscriber's buffer is not flushed before exit.
        eprintln!();
        eprintln!("=== PANIC (init_logging hook) ===");
        eprintln!("{info}");
        eprintln!("{bt}");
        let _ = std::io::Write::flush(&mut std::io::stderr());
    }));

    // Targets are `module_path!()` strings, so a hyphenated crate name appears
    // with an underscore (`data_engine::runtime`) — match that spelling here,
    // exactly as agentik_core is spelled below.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "autonomics=debug,agentik_core=debug,agentik_sdk=debug,runtime=debug,\
             data_engine=info,nodes_ldsc=debug,ldsc=debug",
        )
    });

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_ansi(nocapture) // ANSI colors only when writing to a terminal
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE)
        .with_timer(timer);

    if nocapture {
        // Write to BOTH the log file and stderr so all output is visible
        // in the terminal during debugging.
        subscriber
            .with_writer(file_writer.and(std::io::stderr))
            .init();
    } else {
        subscriber.with_writer(file_writer).init();
    }

    tracing::info!(
        "logging initialized — logs directory: {} (nocapture: {nocapture})",
        log_dir.display()
    );
    Ok(())
}

fn main() -> color_eyre::Result<()> {
    // Parse CLI first so we can read --nocapture before init_logging.
    let cli = Cli::parse();

    // Determine nocapture: true if any subcommand (or the default Tui) has it.
    let nocapture = match &cli.command {
        Some(Command::Tui(args)) => args.nocapture,
        _ => false,
    };

    // `serve` initializes its own logging (absolute state-dir paths — the
    // daemon outlives the launching terminal, so a relative `logs/` dir
    // would be wherever the launcher happened to stand).
    if matches!(&cli.command, Some(Command::Serve(_))) {
        return commands::serve::run_serve(match cli.command {
            Some(Command::Serve(args)) => args,
            _ => unreachable!(),
        });
    }

    init_logging(nocapture)?;

    match cli.command.unwrap_or(Command::Tui(TuiArgs {
        config: None,
        nocapture: false,
    })) {
        Command::Tui(args) => commands::tui::run_tui(args),
        Command::Kms(args) => commands::kms::run_kms(args),
        Command::Cache(cache) => match cache.action {
            CacheAction::RefreshOpengwas(args) => commands::cache::run_refresh_opengwas(args),
            CacheAction::ClearOpengwas(args) => commands::cache::run_clear_opengwas(args),
        },
        Command::Panels(args) => commands::panels::run_panels(args),
        Command::Bib(bib) => {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
            runtime.block_on(commands::bib::run_bib(bib))
        }
        Command::Run(args) => commands::run::run_headless(args),
        Command::Serve(_) => unreachable!("dispatched before init_logging"),
    }
}
