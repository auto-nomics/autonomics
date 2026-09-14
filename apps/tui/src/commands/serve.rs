//! `tui serve` implementation — thin CLI shell over
//! `gateway::run_daemon` / `gateway::manager`.
//!
//! Logging differs from the interactive TUI: the daemon must not depend
//! on the launcher's working directory (it outlives the terminal that
//! spawned it), so its file log lives under the absolute state dir.
//! Foreground mode additionally mirrors to stderr for interactive
//! debugging; `--daemon` mode writes to the file only (its stdio is
//! null).

use std::path::PathBuf;

use time::macros::format_description;
use tokio_util::sync::CancellationToken;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use gateway::daemon::{DaemonOptions, run_daemon};
use gateway::manager;

use crate::cli::{ServeAction, ServeArgs};

pub fn run_serve(args: ServeArgs) -> color_eyre::Result<()> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async move {
        match args.action {
            Some(ServeAction::Status) => {
                match manager::probe().await {
                    Ok(Some(status)) => {
                        println!(
                            "gateway running: pid {} (v{}, up {}s, {} agents, last_seq {})",
                            status.pid,
                            status.version,
                            status.uptime_secs,
                            status.agent_count,
                            status.last_seq
                        );
                        Ok(())
                    }
                    Ok(None) => {
                        println!("gateway not running");
                        std::process::exit(3);
                    }
                    Err(error) => {
                        eprintln!("error: {error}");
                        std::process::exit(3);
                    }
                }
            }
            Some(ServeAction::Stop) => {
                match manager::stop().await {
                    Ok(()) => {
                        println!("gateway shutdown requested");
                        Ok(())
                    }
                    Err(error) => {
                        eprintln!("error: {error}");
                        std::process::exit(3);
                    }
                }
            }
            None => run_foreground_or_daemon(args.daemon).await,
        }
    })
}

async fn run_foreground_or_daemon(daemon: bool) -> color_eyre::Result<()> {
    let config = runtime::RuntimeConfig::default();
    init_gateway_logging(&config.state_dir, daemon)?;

    let shutdown = CancellationToken::new();
    if !daemon {
        // Foreground: Ctrl+C is a graceful stop (same as POST
        // /gateway/shutdown). Daemon mode has no terminal — only the
        // HTTP endpoint stops it.
        let ctrl_shutdown = shutdown.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("ctrl-c received; shutting down gateway");
                ctrl_shutdown.cancel();
            }
        });
    }

    let opts = DaemonOptions {
        config,
        addr: gateway::daemon::env_addr(),
        env_token: gateway::daemon::env_token(),
    };

    match run_daemon(opts, shutdown).await {
        Ok(()) => Ok(()),
        Err(gateway::DaemonError::InstanceLockHeld { path }) => {
            eprintln!(
                "another gateway already owns the state directory ({})\n\
                 it is probably running — check `tui serve status`, or stop it with `tui serve stop`",
                path.display()
            );
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

/// File log under the absolute state dir (independent of the launcher's
/// cwd); foreground also mirrors to stderr.
fn init_gateway_logging(state_dir: &std::path::Path, foreground: bool) -> color_eyre::Result<()> {
    color_eyre::install()?;

    let log_dir: PathBuf = state_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_appender =
        RollingFileAppender::new(Rotation::DAILY, &log_dir, "autonomics-gateway.log");
    let file_writer = file_appender.with_max_level(Level::DEBUG);

    let timer = OffsetTime::new(
        time::UtcOffset::current_local_offset().expect("timezone"),
        format_description!(
            "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
        ),
    );

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("gateway=debug,runtime=debug,agentik_core=debug,agentik_sdk=debug")
    });

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_ansi(foreground)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE)
        .with_timer(timer);

    if foreground {
        subscriber
            .with_writer(file_writer.and(std::io::stderr))
            .init();
    } else {
        subscriber.with_writer(file_writer).init();
    }

    tracing::info!(
        "gateway logging initialized — logs directory: {} (foreground: {foreground})",
        log_dir.display()
    );
    Ok(())
}
