//! Terminal lifecycle, panic handling, and local HTTP API startup.

use super::*;
/// Restore the terminal to its normal state: disable mouse capture, leave
/// alternate screen, and disable raw mode. Called on both normal exit and
/// panic (via the panic hook).
fn restore_terminal() -> std::io::Result<()> {
    crossterm::execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        crossterm::terminal::LeaveAlternateScreen,
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

/// Global flag set by the panic hook so the main render loop can detect
/// a panic on a *background* thread and stop drawing before the restored
/// terminal gets garbled.
pub(super) static PANIC_OCCURRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Install a panic hook that restores the terminal before running the
/// original hook. This ensures the user's shell is usable even if the TUI
/// panics.
///
/// **Thread-safety**: `Once` guarantees `restore_terminal` runs exactly
/// once even when multiple threads panic simultaneously. After restoring
/// we flush stdout so that any buffered TUI draw commands are fully
/// drained *before* the panic message is written to stderr — otherwise
/// the two streams interleave and produce garbled output.
fn set_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANIC_OCCURRED.store(true, std::sync::atomic::Ordering::SeqCst);

        static RESTORE: std::sync::Once = std::sync::Once::new();
        RESTORE.call_once(|| {
            let _ = restore_terminal();
            // Drain any buffered TUI output on stdout so it doesn't
            // interleave with the stderr panic message below.
            let _ = std::io::stdout().flush();
        });

        // Always print a readable panic message to stderr — the logging
        // hook (installed by `init_logging`) writes to the log file only
        // in normal mode, so without this the crash would be completely
        // silent to the user.
        eprintln!();
        eprintln!("══════════════════════════════════════════════");
        eprintln!("  Autonomics TUI panicked");
        eprintln!("══════════════════════════════════════════════");
        eprintln!("{info}");
        eprintln!();

        // Delegate to the logging hook (tracing + backtrace capture).
        hook(info);
    }));
}

impl App {
    pub fn start(&mut self) -> color_eyre::Result<()> {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(
            stdout(),
            crossterm::terminal::EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
        )?;
        stdout().flush()?;

        set_panic_hook();

        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

        // The main loop is async (tokio::select! driven); run it on the
        // existing tokio runtime that also hosts the agent task.
        let runtime = self._runtime.take().expect("runtime already consumed");
        let result = runtime.block_on(self.run_loop(&mut terminal));

        // Stop accepting external API requests before agents and shared
        // infrastructure begin shutdown.
        if let Some(server) = self.http_server.take() {
            if let Err(error) = runtime.block_on(server.shutdown()) {
                tracing::warn!(error = %error, "failed to shut down HTTP API server");
            }
        }

        // Gracefully shut down all agents: pause sessions, persist
        // snapshots, flush WAL. Must be inside `block_on` so the agent
        // tasks can run to completion before the runtime is dropped.
        if let Some(host) = self.host.as_mut() {
            runtime.block_on(host.shutdown_all_agents_and_wait());
        }

        // Restore terminal on exit (whether normal or error).
        let _ = restore_terminal();

        result?;
        Ok(())
    }

    pub(super) fn start_http_server(
        runtime: &tokio::runtime::Runtime,
        host: Option<&RuntimeHost>,
    ) -> Option<tui_http::HttpServerHandle> {
        let Some(host) = host else {
            tracing::warn!("HTTP API disabled: runtime host unavailable");
            return None;
        };

        let addr = std::env::var("AUTONOMICS_HTTP_API_ADDR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| tui_http::DEFAULT_HTTP_API_ADDR.to_owned());
        let bearer_token = std::env::var("AUTONOMICS_HTTP_API_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let shared = host.infra().bib.as_ref().clone();

        match runtime.block_on(async {
            tui_http::start(tui_http::api_router_with_auth(shared, bearer_token), &addr).await
        }) {
            Ok(server) => {
                tracing::info!(
                    addr = %server.addr(),
                    "TUI HTTP API started at http://{}",
                    server.addr()
                );
                Some(server)
            }
            Err(error) => {
                tracing::error!(
                    addr = %addr,
                    error = %error,
                    "failed to start TUI HTTP API"
                );
                None
            }
        }
    }
}
