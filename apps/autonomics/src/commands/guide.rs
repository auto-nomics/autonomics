//! Bare `autonomics` — the getting-started guide.
//!
//! No subcommand given: probe the daemon, run the offline readiness
//! checks, and print suggested next commands. Never starts anything,
//! never prompts, always exits 0 — the readiness exit contract belongs
//! to `prepare --check`.

use color_eyre::Result;

use crate::diagnostics;

pub fn run_guide() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async move { guide().await })
}

async fn guide() -> Result<()> {
    let config = gateway::RuntimeConfig::default();

    // ── Gateway state ─────────────────────────────────────────────
    println!("gateway");
    let daemon_running = match gateway::manager::probe().await {
        Ok(Some(status)) => {
            crate::commands::serve::print_serve_status(&status);
            true
        }
        Ok(None) => {
            println!("gateway: not running");
            false
        }
        Err(error) => {
            println!("gateway: cannot probe ({error}) — check 'autonomics serve status'");
            false
        }
    };

    // ── Local readiness (offline checks) ─────────────────────────
    println!("\nlocal readiness (offline checks)");
    let report = diagnostics::check_readiness(&config).await;
    report.print();

    // ── Suggested next steps ─────────────────────────────────────
    println!("\nnext steps");
    if report.has_fatal_gaps() {
        println!("  1. `autonomics prepare`     # provision the gaps above");
        if !daemon_running {
            println!("  2. `autonomics serve`       # then start the daemon");
            println!("  3. `autonomics tui`         # then launch the TUI");
        } else {
            println!("  2. `autonomics serve stop`  # the running daemon predates prepare");
            println!("  3. `autonomics serve`       # restart it on the provisioned state");
        }
    } else if daemon_running {
        println!("  `autonomics tui` — the daemon is up and readiness is clean");
        println!("  `autonomics serve stop` when you are done");
    } else {
        println!("  1. `autonomics serve`       # readiness is clean — start the daemon");
        println!("     (`autonomics serve --daemon` to run it detached)");
        println!("  2. `autonomics tui`         # then launch the TUI");
    }
    Ok(())
}
