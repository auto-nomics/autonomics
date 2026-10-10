//! `autonomics serve` implementation — thin CLI shell over
//! `gateway::run_daemon` / `gateway::manager`.
//!
//! Logging differs from the interactive TUI: the daemon must not depend
//! on the launcher's working directory (it outlives the terminal that
//! spawned it), so its file log lives under the absolute state dir.
//! Foreground mode additionally mirrors to stderr for interactive
//! debugging; `--daemon` mode writes to the file only. During auto-spawn
//! its startup diagnostics inherit the launcher's stdio, then stdio is
//! detached as soon as the gateway server is ready.

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
use gateway::proto::GatewayStatus;

use container_plugin::bundles;
use container_plugin::factory::Plugin;
use container_plugin::sync::{self, EntryOutcome};
use dag_core::NodePlugin;
use data_catalog::LocalCatalog;

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
        if let Some(ServeAction::Status) = args.action {
            match manager::probe().await {
                Ok(Some(status)) => {
                    print_serve_status(&status);
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
        } else if let Some(ServeAction::Stop) = args.action {
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
        } else {
            run_foreground_or_daemon(
                daemon_config.expect("daemon config was prepared"),
                args.daemon,
            )
            .await
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

/// Render the running gateway as a multi-line key/value block.
///
/// Two pieces of information the daemon already knows aren't surfaced on
/// the bare `GatewayStatus`: the bib web frontend is mounted at the same
/// root as the API, so its URL is `http://<addr>/` (a host:port string
/// is otherwise ambiguous in the terminal — `127.0.0.1:8765` is an
/// address, not a clickable link). The uptime is also raw seconds, which
/// is the second thing everyone reformats by hand.
fn print_serve_status(status: &GatewayStatus) {
    let base_url = web_base_url(&status.addr);
    let uptime = format_uptime(status.uptime_secs);

    println!(
        "gateway running — pid {} (v{}, up {})",
        status.pid, status.version, uptime
    );
    println!("  addr        {}", status.addr);
    println!("  bib web     {}/", base_url);
    println!("  agents      {}", status.agent_count);
    println!("  last seq    {}", status.last_seq);
}

/// Build the URL a human (or browser) opens to reach the bib web
/// frontend. The daemon's bind string is bare `host:port` — `probe`
/// reads the same env var the daemon bound to, so this can't drift.
/// Falls back to `127.0.0.1` for the loopback case where the daemon
/// bound to `0.0.0.0`, which is bind-only and not a valid URL host.
fn web_base_url(addr: &str) -> String {
    if addr.contains("://") {
        return addr.trim_end_matches('/').to_owned();
    }
    let host_part = if addr.starts_with("0.0.0.0:") {
        format!("127.0.0.1:{}", addr.trim_start_matches("0.0.0.0:"))
    } else {
        addr.to_owned()
    };
    format!("http://{}", host_part)
}

/// Render seconds as `Hh Mm Ss`, dropping zero-leading units so an
/// uptime of 47s doesn't print as `0h 0m 47s`.
fn format_uptime(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
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
        detach_stdio_on_ready: daemon,
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

/// Plugin preflight: sync every declared family into the v2 runtime root,
/// then validate every manifest and report the registered kinds.
/// Runs in the launcher process so the report is visible even in `--daemon`
/// mode (before stdio detaches). A missing `plugins.toml` is not an error —
/// the deployment simply starts with built-in nodes only.
async fn plugin_preflight(config: &gateway::RuntimeConfig) -> color_eyre::Result<()> {
    let config_path = config.state_dir.join(sync::PLUGIN_CONFIG_FILE);
    let layout = plugin_rsi::PluginStateLayout::open(&config.state_dir);
    layout
        .ensure_v2_directories()
        .map_err(|error| color_eyre::eyre::eyre!("invalid plugin layout: {error}"))?;
    let root = layout.runtime_root();

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
    let plugins = crate::commands::panels::load_installed_plugins(&config.state_dir)?;

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
    // Mirror the headline to the tracing log: a daemon auto-spawned by a
    // frontend runs with nulled stdio, so its println report would vanish.
    tracing::info!(
        families = plugins.len(),
        kinds = total_kinds,
        "plugin preflight verified"
    );

    panel_bundle_preflight(&plugins, &config.state_dir).await?;
    Ok(())
}

/// Environment opt-in that runs the full panel-bundle provisioning phase
/// inline during startup (truthy: `1`/`true`/`yes`/`on`), before the daemon
/// builds its bundle registry. Unattended deployments get a one-command
/// cold start; interactive ones should prefer `autonomics panels sync`.
pub const PANEL_SYNC_ENV: &str = "AUTONOMICS_PANEL_SYNC";

/// Panel-bundle preflight: a **local-only** presence check of every
/// `[[panels]]` bundle against the verified catalog cache. Zero network
/// access, so daemon readiness stays bounded no matter the network state —
/// frontends auto-starting the daemon (`gateway::manager::ensure_running`)
/// rely on that.
///
/// Missing bundles are reported with a pointer at the independent
/// provisioning phase (`autonomics panels sync`); the affected nodes fail
/// closed at build time and remain installable at runtime via the catalog
/// tools. `AUTONOMICS_PANEL_SYNC=1` switches this phase to full inline
/// provisioning instead (`panels::provision_missing_bundles`).
async fn panel_bundle_preflight(
    plugins: &[Plugin],
    state_dir: &std::path::Path,
) -> color_eyre::Result<()> {
    let bundle_ids = bundles::collect_panel_bundles(plugins);
    if bundle_ids.is_empty() {
        return Ok(());
    }
    if let Some(value) = std::env::var_os(PANEL_SYNC_ENV) {
        if matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ) {
            crate::commands::panels::provision_missing_bundles(state_dir).await?;
            return Ok(());
        }
    }

    let local = LocalCatalog::open(data_catalog::default_panel_cache_root())
        .map_err(|error| color_eyre::eyre::eyre!("cannot open panel cache: {error}"))?;
    let report = bundles::check_panel_bundles(&bundle_ids, &local);
    let counts = report.counts();
    if counts.unavailable == 0 {
        println!(
            "panels: {} data bundle{} cached",
            bundle_ids.len(),
            if bundle_ids.len() == 1 { "" } else { "s" }
        );
        tracing::info!(bundles = bundle_ids.len(), "panel bundles all cached");
        return Ok(());
    }
    // Mirror to tracing: an auto-spawned daemon's stdio is nulled, and
    // missing bundles are exactly what its operator needs to learn about.
    for repo in report.missing_repos() {
        println!("  ! {repo} not cached");
        tracing::warn!(
            repo,
            "panel bundle not cached — 'autonomics panels sync' fetches it"
        );
    }
    println!(
        "panels: {} of {} data bundles cached — run 'autonomics panels sync' to fetch the rest \
         (nodes needing them fail at build time until then)",
        counts.cached,
        bundle_ids.len()
    );
    tracing::warn!(
        cached = counts.cached,
        total = bundle_ids.len(),
        "panel bundles incomplete — run 'autonomics panels sync'"
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
    config.kms_db_path = absolute(&config.kms_db_path);
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
    // First writer on a fresh state dir: at this point in startup nothing
    // has created it yet (the log dir and the instance lock come later),
    // so a first-ever launch — including the auto-spawned daemon on a
    // brand-new machine — dies here without this.
    std::fs::create_dir_all(state_dir).map_err(|error| {
        color_eyre::eyre::eyre!("cannot create {}: {error}", state_dir.display())
    })?;
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
             data_engine=info,autonomics=info",
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
