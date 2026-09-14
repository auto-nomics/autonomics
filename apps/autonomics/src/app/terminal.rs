//! Terminal lifecycle and panic handling.

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
        // Recoverable extraction panics (pdf-extract on malformed fonts)
        // are converted to errors at the source via bib-base's panic
        // guard. They must not restore the terminal, print the banner, or
        // stop the render loop — a single bad PDF cannot kill the TUI.
        if bib_base::is_expected_panic() {
            tracing::warn!(payload = %info, "suppressed recoverable extraction panic");
            return;
        }

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
        // existing tokio runtime that also hosts the client tasks.
        let runtime = self.runtime.take().expect("runtime already consumed");
        let result = runtime.block_on(self.run_loop(&mut terminal));

        // Thin-client exit: restore the terminal and disconnect. Agents
        // keep running in the gateway daemon — that is the point of the
        // resident-backend architecture; `autonomics serve stop` is the only
        // thing that shuts them down.
        let _ = restore_terminal();

        eprintln!(
            "Gateway daemon 仍在后台运行（agents 未受影响）；`autonomics serve stop` 可停止。"
        );

        result?;
        Ok(())
    }
}
