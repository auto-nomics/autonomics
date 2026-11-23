//! Bare `autonomics` — the getting-started guide.
//!
//! No subcommand given: probe the daemon, run the offline readiness
//! checks, and print suggested next commands. Never starts anything,
//! never prompts, always exits 0 — the readiness exit contract belongs
//! to `prepare --check`.

use color_eyre::Result;
use i18n::t;

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
            println!("{}", t!("cli.guide.gatewayNotRunning"));
            false
        }
        Err(error) => {
            println!(
                "{}",
                t!("cli.guide.gatewayProbeFailed", error = error)
            );
            false
        }
    };

    // ── Local readiness (offline checks) ─────────────────────────
    println!("\n{}", t!("cli.guide.localReadiness"));
    let report = diagnostics::check_readiness(&config).await;
    report.print();

    // ── Suggested next steps ─────────────────────────────────────
    println!("\n{}", t!("cli.guide.nextSteps"));
    if report.has_fatal_gaps() {
        println!("  1. {}", t!("cli.guide.stepPrepare"));
        if !daemon_running {
            println!("  2. {}", t!("cli.guide.stepServe"));
            println!("  3. {}", t!("cli.guide.stepTui"));
        } else {
            println!("  2. {}", t!("cli.guide.stepServeStopRestart"));
            println!("  3. {}", t!("cli.guide.stepServeRestart"));
        }
    } else if daemon_running {
        println!("  {}", t!("cli.guide.stepTuiReady"));
        println!("  {}", t!("cli.guide.stepServeStopDone"));
    } else {
        println!("  1. {}", t!("cli.guide.stepServeClean"));
        println!("  2. {}", t!("cli.guide.stepTui"));
    }
    Ok(())
}
