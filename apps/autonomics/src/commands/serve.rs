//! `autonomics serve` implementation — thin CLI shell over
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
    let daemon_config = if args.action.is_none() {
        let mut config = gateway::RuntimeConfig::default();
        stabilize_daemon_cwd(&mut config)?;
        Some(config)
    } else {
        None
    };
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async move {
        match args.action {
            Some(ServeAction::Status) => match manager::probe().await {
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
            },
            Some(ServeAction::Stop) => match manager::stop().await {
                Ok(()) => {
                    println!("gateway shutdown requested");
                    Ok(())
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(3);
                }
            },
            None => {
                run_foreground_or_daemon(
                    daemon_config.expect("daemon config was prepared"),
                    args.daemon,
                )
                .await
            }
        }
    })
}

async fn run_foreground_or_daemon(
    config: gateway::RuntimeConfig,
    daemon: bool,
) -> color_eyre::Result<()> {
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
                 it is probably running — check `autonomics serve status`, or stop it with `autonomics serve stop`",
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

/// Resolve launcher-relative paths before moving the long-lived daemon to a
/// stable host cwd. The daemon can outlive the frontend and the directory it
/// was launched from; child processes such as Podman resolve their inherited
/// cwd during startup.
fn stabilize_daemon_cwd(config: &mut gateway::RuntimeConfig) -> color_eyre::Result<()> {
    let launcher_cwd = std::env::current_dir().map_err(|error| {
        color_eyre::eyre::eyre!("cannot resolve launcher working directory: {error}")
    })?;
    let absolute = |path: &std::path::Path| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            launcher_cwd.join(path)
        }
    };

    config.data_dir = absolute(&config.data_dir);
    config.state_dir = absolute(&config.state_dir);
    config.dag_history_db = absolute(&config.dag_history_db);
    config.bib_db_path = absolute(&config.bib_db_path);
    config.writing_db_path = absolute(&config.writing_db_path);
    config.app_db_path = absolute(&config.app_db_path);
    config.agent_db = absolute(&config.agent_db);
    config.opengwas_cache_dir = config
        .opengwas_cache_dir
        .as_ref()
        .map(|path| absolute(path));

    std::env::set_current_dir("/").map_err(|error| {
        color_eyre::eyre::eyre!("cannot move gateway daemon to stable working directory: {error}")
    })
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
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"),
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
