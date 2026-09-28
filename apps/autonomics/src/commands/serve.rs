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
use std::time::Duration;

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

use container_plugin::loader;
use dag_core::NodePlugin;
use container_plugin::sync::{self, EntryOutcome};

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
    })?;
    // Bound the time the runtime waits for background tasks once the
    // foreground future returns. Without this, `Runtime::drop` blocks
    // indefinitely on tasks that don't observe the daemon's
    // `CancellationToken` — e.g. `spawn_container_gc`'s infinite sleep
    // loop — pinning the 25 GB `SharedInfra` and preventing the process
    // from exiting after a graceful `serve stop`. Five seconds is enough
    // for `bib-extraction-sweep` (one-shot) and the GC's current sweep;
    // anything still running is forcibly cancelled.
    runtime.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn run_foreground_or_daemon(
    config: gateway::RuntimeConfig,
    daemon: bool,
) -> color_eyre::Result<()> {
    init_gateway_logging(&config.state_dir, daemon)?;

    // Plugin self-check phase: install every declared family and report
    // what registered, before the daemon owns the process. Fail-closed —
    // a declared-but-broken plugin aborts startup naming the offender.
    // (SharedInfra::open repeats the sync inside the daemon; it is
    // idempotent and offline-safe once everything is installed.)
    plugin_preflight(&config).await?;

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

/// Plugin preflight: sync every declared family into the plugin root, then
/// load the root to validate every manifest and report the registered kinds.
/// Runs in the launcher process so the report is visible even in `--daemon`
/// mode (before stdio detaches). A missing `plugins.toml` is not an error —
/// the deployment simply starts with built-in nodes only.
async fn plugin_preflight(config: &gateway::RuntimeConfig) -> color_eyre::Result<()> {
    let config_path = config.state_dir.join(sync::PLUGIN_CONFIG_FILE);
    let root = config.state_dir.join("plugins");

    if !config_path.is_file() {
        println!(
            "plugins: {} not present — starting with built-in nodes only",
            config_path.display()
        );
        return Ok(());
    }

    println!("plugins: syncing from {}", config_path.display());
    let report = sync::sync(&config_path, &root)
        .map_err(|error| color_eyre::eyre::eyre!("plugin sync failed: {error}"))?;
    for (name, outcome) in &report.outcomes {
        match outcome {
            EntryOutcome::Installed => println!("  + {name} installed"),
            EntryOutcome::Updated => println!("  ~ {name} updated"),
            EntryOutcome::Unchanged => {}
        }
    }
    if report.outcomes.is_empty() {
        println!("  (no plugin sources declared)");
    }

    // Loading validates every manifest fail-closed and constructs the
    // families; the connection is only carried for factory construction,
    // no container is spawned here.
    let connection: std::sync::Arc<dyn container_runtime::PodmanConnection> =
        std::sync::Arc::new(container_runtime::PodmanRuntime::new(
            container_runtime::PodmanConfig {
                program: "podman".into(),
                workspace_root: root.join("workspace"),
                panel_cache_root: root.join("panels"),
            },
        ));
    let panel_cache = std::sync::Arc::new(container_runtime::PanelCache::new(
        root.join("panels"),
    ));
    let plugins = loader::load(&root, connection, panel_cache)
        .map_err(|error| color_eyre::eyre::eyre!("plugin load failed: {error}"))?;

    let mut total_kinds = 0usize;
    for plugin in &plugins {
        let kinds = plugin.registered_kinds();
        println!("  ok {} [{}]", plugin.name(), kinds.join(", "));
        total_kinds += kinds.len();
    }
    println!(
        "plugins: {} famil{} verified, {} node kinds",
        plugins.len(),
        if plugins.len() == 1 { "y" } else { "ies" },
        total_kinds
    );
    Ok(())
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

    setup_network_allowlist(&config.state_dir)?;
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

/// Materialize the deny-by-default HTTP fetch allowlist for the `http_fetch`
/// node: create the commented template on first launch and point the node's
/// environment variable at it, so every fetch names the file it must be
/// added to.
fn setup_network_allowlist(state_dir: &std::path::Path) -> color_eyre::Result<()> {
    let allowlist_path = state_dir.join(nodes_io::http_fetch::ALLOWLIST_FILE_NAME);
    if !allowlist_path.exists() {
        std::fs::write(
            &allowlist_path,
            nodes_io::http_fetch::default_allowlist_toml(),
        )
        .map_err(|error| {
            color_eyre::eyre::eyre!("cannot write {}: {error}", allowlist_path.display())
        })?;
        tracing::info!(
            allowlist = %allowlist_path.display(),
            "created default HTTP fetch network allowlist (deny-by-default)"
        );
    }
    // SAFETY: single-threaded daemon startup — no worker threads exist yet to
    // race this write, matching set_current_dir above.
    unsafe {
        std::env::set_var(nodes_io::http_fetch::ENV_NETWORK_ALLOWLIST, &allowlist_path);
    }
    Ok(())
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

    // Targets are `module_path!()` strings, so a hyphenated crate name appears
    // with an underscore (`data_engine::...`) — match that spelling here,
    // exactly as agentik_core is spelled below.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "gateway=debug,runtime=debug,agentik_core=debug,agentik_sdk=debug,\
             data_engine=info",
        )
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
