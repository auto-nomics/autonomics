//! `autonomics serve` implementation — thin CLI shell over
//! `gateway::run_daemon` / `gateway::manager`.
//!
//! The daemon always runs detached: the CLI re-executes itself as a
//! child (own process group, stdin/stdout closed), waits until the
//! child's gateway is bound, prints its status, and exits — the daemon
//! outlives the terminal that spawned it. Until the gateway is bound
//! the child's stderr is a pipe back to the launcher and its log is
//! mirrored there, so a startup death reports the daemon's own
//! diagnostics instead of a bare exit code. The daemon's file log lives
//! under the absolute state dir; the gateway server points stdio at
//! /dev/null as soon as it is ready.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

use crate::cli::{ServeAction, ServeArgs};

/// Set on the detached daemon child so it recognizes itself and does
/// not re-spawn another launcher.
const DAEMON_CHILD_ENV: &str = "AUTONOMICS_DAEMON_CHILD";

/// How long the launcher waits for the detached daemon to become ready
/// before giving up. Cold starts are dominated by `SharedInfra`
/// (embedding model etc.) and can take tens of seconds.
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(90);

pub fn run_serve(args: ServeArgs) -> color_eyre::Result<()> {
    // The daemon child (re-exec'd below) runs the real daemon in this
    // process; the launcher spawn-waits and returns.
    if args.action.is_none() && std::env::var_os(DAEMON_CHILD_ENV).is_none() {
        return spawn_detached_and_wait();
    }

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
            // Daemon child (DAEMON_CHILD_ENV set): build the config and
            // run the daemon until shutdown.
            let mut config = gateway::RuntimeConfig::default();
            stabilize_daemon_cwd(&mut config)?;
            run_daemon_child(config).await
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
pub(crate) fn print_serve_status(status: &GatewayStatus) {
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

/// Re-execute this binary as a detached daemon child and wait until its
/// gateway is ready, then return so the CLI exits and the terminal is
/// freed. The child runs in its own process group with stdin/stdout
/// closed and stderr piped back to the launcher — the daemon mirrors its
/// startup log to stderr and only points fd 2 at /dev/null once bound
/// (`detach_stdio_on_ready`), so a child that dies mid-startup can
/// explain itself below. The daemon still survives the launcher, the
/// terminal, and Ctrl+C; only `autonomics serve stop`
/// (POST /gateway-shutdown) stops it.
fn spawn_detached_and_wait() -> color_eyre::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe()
        .map_err(|e| color_eyre::eyre::eyre!("cannot resolve current executable: {e}"))?;

    let mut child = Command::new(exe)
        .arg("serve")
        .env(DAEMON_CHILD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| color_eyre::eyre::eyre!("cannot spawn daemon child: {e}"))?;
    let child_pid = child.id();
    let stderr_tap = child.stderr.take().map(StderrTap::spawn);

    // The appender rotates daily (`autonomics-gateway.YYYY-MM-DD`), so
    // point at the directory rather than a dated filename.
    let log_hint = || gateway::RuntimeConfig::default().state_dir.join("logs");

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + DAEMON_READY_TIMEOUT;
        loop {
            // A dead child loses the race against any probe result:
            // check it first so a startup failure is never masked by a
            // stale daemon's status response.
            if let Some(status) = child.try_wait().ok().flatten() {
                let tail = startup_output_tail(stderr_tap.as_ref()).await;
                return Err(color_eyre::eyre::eyre!(
                    "daemon exited during startup (code {})\n{tail}  see logs: {}",
                    status
                        .code()
                        .map_or_else(|| "signal".to_owned(), |c| c.to_string()),
                    log_hint().display()
                ));
            }
            if let Ok(Some(status)) = manager::probe().await {
                // Require the status to belong to *our* child: a token
                // file left by a previous run could answer for a stale
                // daemon otherwise.
                if status.pid == child_pid {
                    print_serve_status(&status);
                    println!("  logs        {}", log_hint().display());
                    return Ok(());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.kill();
                let tail = startup_output_tail(stderr_tap.as_ref()).await;
                return Err(color_eyre::eyre::eyre!(
                    "daemon did not become ready within {}s\n{tail}  see logs: {}",
                    DAEMON_READY_TIMEOUT.as_secs(),
                    log_hint().display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
}

async fn startup_output_tail(tap: Option<&StderrTap>) -> String {
    match tap {
        Some(tap) => tap.tail().await,
        None => String::new(),
    }
}

/// Live view of the daemon child's stderr, drained on a parked thread
/// while the launcher waits for readiness.
struct StderrTap {
    buffer: Arc<Mutex<Vec<u8>>>,
    done: Arc<AtomicBool>,
}

/// Lines kept for the CLI's failure message — enough to hold a startup
/// readiness report plus the fatal error below it.
const STDERR_TAIL_LINES: usize = 64;

impl StderrTap {
    fn spawn(pipe: std::process::ChildStderr) -> Self {
        let tap = Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new(AtomicBool::new(false)),
        };
        let buffer = tap.buffer.clone();
        let done = tap.done.clone();
        // A plain OS thread rather than `spawn_blocking`: on the success
        // path the launcher tears its runtime down without joining, and
        // a pre-detach grandchild (podman, git) could hold the pipe's
        // write end open past that point. Draining continuously also
        // keeps the pipe from filling and stalling the child mid-log.
        std::thread::spawn(move || {
            use std::io::Read;
            let mut pipe = pipe;
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut buffer = buffer.lock().expect("stderr tap lock");
                        buffer.extend_from_slice(&chunk[..n]);
                        // Only the tail is ever shown; don't let a chatty
                        // startup grow the capture without bound across
                        // the ready-timeout window.
                        let excess = buffer.len().saturating_sub(512 * 1024);
                        if excess > 0 {
                            buffer.drain(..excess);
                        }
                    }
                }
            }
            done.store(true, Ordering::Release);
        });
        tap
    }

    /// The last [`STDERR_TAIL_LINES`] lines the child wrote, waiting
    /// briefly for the reader to drain the pipe — the write end EOFs as
    /// soon as the (dead or killed) child and its pre-detach
    /// subprocesses are gone.
    async fn tail(&self) -> String {
        for _ in 0..20 {
            if self.done.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let text =
            String::from_utf8_lossy(&self.buffer.lock().expect("stderr tap lock")).into_owned();
        format_startup_tail(&text)
    }
}

/// Render the captured startup output for the launcher's failure
/// message: the last [`STDERR_TAIL_LINES`] lines, indented under a
/// marker header; empty capture renders as nothing at all so the
/// failure message stays clean when the child died before logging.
fn format_startup_tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let skipped = lines.len().saturating_sub(STDERR_TAIL_LINES);
    let mut out = String::from("\ndaemon startup output");
    if skipped > 0 {
        out.push_str(&format!(" (first {skipped} lines cut)"));
    }
    out.push_str(":\n");
    for line in &lines[skipped..] {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod startup_tail_tests {
    use super::*;

    #[test]
    fn empty_capture_renders_nothing() {
        assert_eq!(format_startup_tail(""), String::new());
        assert_eq!(format_startup_tail("\n"), String::new());
    }

    #[test]
    fn short_capture_is_indented_under_header() {
        let rendered = format_startup_tail("plugins  ok\ndaemon error: boom");
        assert!(rendered.starts_with("\ndaemon startup output:\n"));
        assert!(rendered.contains("  plugins  ok\n"));
        assert!(rendered.contains("  daemon error: boom\n"));
        assert!(rendered.ends_with('\n'));
        assert!(!rendered.contains("cut"));
    }

    #[test]
    fn long_capture_keeps_only_the_tail_and_counts_the_cut() {
        let text: String = (0..STDERR_TAIL_LINES + 10)
            .map(|i| format!("line {i}\n"))
            .collect();
        let rendered = format_startup_tail(&text);
        assert!(rendered.contains("(first 10 lines cut)"));
        assert!(!rendered.contains("line 9\n"));
        assert!(rendered.contains(&format!("  line {}\n", STDERR_TAIL_LINES + 9)));
    }
}

async fn run_daemon_child(config: gateway::RuntimeConfig) -> color_eyre::Result<()> {
    init_gateway_logging(&config.state_dir)?;

    // Startup diagnostic gate — the exact same offline checks as
    // `autonomics prepare --check`. Zero network; a fatal gap (plugins,
    // model, images) aborts before the daemon owns the process and
    // before the instance lock, so nothing starts half-provisioned.
    // Provisioning is `prepare`'s job; the daemon-internal plugin sync
    // (`SharedInfra::open` → materialize_registry) stays as an
    // idempotent, network-free no-op backstop once the gate passed.
    // The report is mirrored to stderr (piped back to the launcher), so
    // a fatal gate abort shows it in the CLI error; the file log keeps
    // the full record.
    let report = crate::diagnostics::check_readiness(&config).await;
    tracing::info!("startup readiness report:\n{}", report.render());
    if report.has_fatal_gaps() {
        tracing::error!("daemon not started — run `autonomics prepare` to provision the gaps");
        std::process::exit(1);
    }

    // No Ctrl+C handler: the daemon child runs in its own process group
    // with no controlling terminal — only `autonomics serve stop` (the
    // shutdown token via the HTTP endpoint) stops it.
    let shutdown = CancellationToken::new();

    let opts = DaemonOptions {
        config,
        addr: gateway::daemon::env_addr(),
        env_token: gateway::daemon::env_token(),
        detach_stdio_on_ready: true,
    };

    // Startup failures are mirrored to stderr — which the launcher
    // captures and prints back — and recorded in the file log; the CLI
    // process exits nonzero either way.
    match run_daemon(opts, shutdown).await {
        Ok(()) => Ok(()),
        Err(gateway::DaemonError::InstanceLockHeld { path }) => {
            tracing::error!(
                "another gateway already owns the state directory ({}) — \
                 check `autonomics serve status`, or stop it with `autonomics serve stop`",
                path.display()
            );
            std::process::exit(1);
        }
        Err(error) => {
            tracing::error!("daemon error: {error}");
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
/// cwd). The daemon child's stdio is closed, so the file is the only
/// log sink.
fn init_gateway_logging(state_dir: &std::path::Path) -> color_eyre::Result<()> {
    // Tolerate a prior `color_eyre::install()` — only the first install
    // wins, which is all we need.
    let _ = color_eyre::install();

    let log_dir: PathBuf = state_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_appender =
        RollingFileAppender::new(Rotation::DAILY, &log_dir, "autonomics-gateway.log");
    let file_writer = file_appender.with_max_level(Level::DEBUG);

    // Mirror INFO+ to stderr during startup: the launcher pipes the
    // child's stderr back to the user's terminal, so a daemon that dies
    // before binding reports its own diagnostics — the readiness
    // report, instance-lock/protocol errors, panics. Once the gateway
    // binds, `detach_stdio` points fd 2 at /dev/null and this writer
    // becomes a silent sink for the rest of the daemon's life.
    let stderr_writer = std::io::stderr.with_max_level(Level::INFO);

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
        .with_ansi(false)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE)
        .with_timer(timer);

    subscriber
        .with_writer(file_writer.and(stderr_writer))
        .init();

    tracing::info!(
        "gateway logging initialized — logs directory: {}",
        log_dir.display()
    );
    Ok(())
}
