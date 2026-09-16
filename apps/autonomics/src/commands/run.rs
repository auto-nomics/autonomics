//! Headless run subcommand — thin CLI shell over the gateway runner.
//!
//! The prompt is submitted to the resident gateway daemon
//! (`headless::gateway_runner::run_via_gateway`, auto-started when
//! absent).
//!
//! Responsibilities are deliberately narrow: prompt assembly (args +
//! stdin), processor selection (`--json` vs human), the `-o`
//! last-message file, and the exit-code contract. Everything else
//! belongs to the headless library, so the CLI and any future adapter
//! share one execution path.
//!
//! Exit codes (see docs/headless-run-design.md §3):
//! 0 turn completed · 1 turn failed · 2 run cancelled · 3 startup error.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::time::Duration;

use headless::gateway_runner::list_sessions_via_gateway;
use headless::gateway_runner::{GatewayRunConfig, run_via_gateway};
use headless::processor::{HumanProcessor, JsonlProcessor, OutputProcessor};
use headless::{RunError, RunSummary};
use tokio_util::sync::CancellationToken;

use crate::cli::RunArgs;

const EXIT_COMPLETED: i32 = 0;
const EXIT_TURN_FAILED: i32 = 1;
const EXIT_CANCELLED: i32 = 2;
const EXIT_STARTUP: i32 = 3;

pub fn run_headless(args: RunArgs) -> color_eyre::Result<()> {
    if let Err(message) = validate_run_args(&args) {
        eprintln!("error: {message}");
        std::process::exit(EXIT_STARTUP);
    }
    if args.list_sessions {
        return print_headless_sessions(&args);
    }
    let prompt = match resolve_prompt(args.prompt.clone()) {
        Ok(prompt) => prompt,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(EXIT_STARTUP);
        }
    };
    let prompt_hash = headless::manifest::prompt_hash(&prompt);
    let run_id = uuid::Uuid::new_v4();
    let agent_runtime = match parse_agent_runtime(&args) {
        Ok(runtime) => runtime,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(EXIT_STARTUP);
        }
    };

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    let cancel = CancellationToken::new();
    let ctrl_cancel = cancel.clone();
    let _ctrl_task = runtime.spawn(async move {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                break;
            }
            ctrl_cancel.cancel();
        }
    });
    runtime.block_on(async {
        let (summary, model_used) =
            run_on_gateway(&args, &agent_runtime, prompt, run_id, cancel).await;

        match summary {
            Ok(summary) => {
                if let Some(path) = args.manifest.as_deref() {
                    let meta = headless::manifest::ManifestMeta {
                        run_id,
                        prompt_hash,
                        profile: summary.profile.clone(),
                        model: model_used.unwrap_or_default(),
                    };
                    let manifest = headless::manifest::RunManifest::new(meta, &summary);
                    if let Err(e) = manifest.write_to(path) {
                        eprintln!("Failed to write manifest {}: {e}", path.display());
                    }
                }
                std::process::exit(match summary.outcome {
                    headless::processor::Outcome::Completed => EXIT_COMPLETED,
                    headless::processor::Outcome::Failed => EXIT_TURN_FAILED,
                    headless::processor::Outcome::Cancelled => EXIT_CANCELLED,
                    headless::processor::Outcome::Unknown => EXIT_TURN_FAILED,
                })
            }
            Err(error) => {
                eprintln!("error: {error}");
                if matches!(error, RunError::Cancelled) {
                    std::process::exit(EXIT_CANCELLED);
                }
                std::process::exit(EXIT_STARTUP);
            }
        }
    })
}

/// The default path: submit the prompt to the resident gateway daemon
/// (auto-started when absent). Returns the run result plus the model
/// name used (for the manifest).
async fn run_on_gateway(
    args: &RunArgs,
    agent_runtime: &agentik_core::AgentRuntimeOverrides,
    prompt: String,
    run_id: uuid::Uuid,
    cancel: CancellationToken,
) -> (Result<RunSummary, RunError>, Option<String>) {
    let config = GatewayRunConfig {
        run_id,
        prompt,
        profile: args.profile.clone(),
        agent_runtime: agent_runtime.clone(),
        model: args.model.clone(),
        session: args.session,
        timeout: args.timeout.map(Duration::from_secs),
        cancel,
    };

    let result = if args.json {
        let mut processor = JsonlProcessor::new(std::io::stdout());
        let result = run_via_gateway(config, &mut processor).await;
        write_last_message(
            args.output_last_message.as_deref(),
            processor.last_message(),
        );
        result
    } else {
        let mut processor = HumanProcessor::new(std::io::stderr(), std::io::stdout());
        let result = run_via_gateway(config, &mut processor).await;
        write_last_message(
            args.output_last_message.as_deref(),
            processor.last_message(),
        );
        result
    };

    // Manifest hint: --model's name part, else the daemon's active model
    // (the daemon is guaranteed up after a run attempt, even a failed
    // one — `run_via_gateway` starts it before anything else).
    let model_used = match args.model.as_deref() {
        Some(spec) => Some(
            spec.split_once(':')
                .map(|(_, name)| name.to_string())
                .unwrap_or_else(|| spec.to_string()),
        ),
        None => daemon_active_model_name().await,
    };
    (result, model_used)
}

/// Best-effort read of the daemon's active model name (manifest hint).
async fn daemon_active_model_name() -> Option<String> {
    let token = gateway::manager::read_token();
    let client =
        gateway::GatewayClient::new(&gateway::daemon::env_addr(), token.as_deref()).ok()?;
    let spec = client.state().await.ok()?.active_model_spec?;
    Some(
        spec.split_once(':')
            .map(|(_, name)| name.to_string())
            .unwrap_or(spec),
    )
}

fn validate_run_args(args: &RunArgs) -> Result<(), String> {
    if args.list_sessions
        && (args.prompt.is_some()
            || args.session.is_some()
            || args.timeout.is_some()
            || args.output_last_message.is_some()
            || args.manifest.is_some()
            || args.agent_config.is_some()
            || args.no_memory)
    {
        return Err("--list-sessions cannot be combined with run-specific options".to_string());
    }
    Ok(())
}

fn parse_agent_runtime(args: &RunArgs) -> Result<agentik_core::AgentRuntimeOverrides, String> {
    let mut runtime = match args.agent_config.as_deref() {
        Some(raw) => {
            serde_json::from_str(raw).map_err(|e| format!("invalid --agent-config JSON: {e}"))?
        }
        None => agentik_core::AgentRuntimeOverrides::default(),
    };
    if args.no_memory {
        runtime.use_memory = Some(false);
        runtime.generate_memory = Some(false);
    }
    Ok(runtime)
}

fn print_headless_sessions(args: &RunArgs) -> color_eyre::Result<()> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    let result = runtime.block_on(list_sessions_via_gateway(args.profile.clone()));
    let mut sessions = match result {
        Ok(sessions) => sessions,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(EXIT_STARTUP);
        }
    };
    sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active).then(a.id.cmp(&b.id)));

    if args.json {
        let output = serde_json::to_vec(&sessions)
            .map_err(|e| color_eyre::eyre::eyre!("serialize sessions: {e}"))?;
        std::io::stdout().write_all(&output)?;
        std::io::stdout().write_all(b"\n")?;
        return Ok(());
    }

    for session in sessions {
        let active = if session.active { "active" } else { "paused" };
        let timestamp = chrono::DateTime::from_timestamp_millis(session.last_active)
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| session.last_active.to_string());
        let title = session.title.as_deref().unwrap_or("<untitled>");
        println!("{active}\t{timestamp}\t{}\t{title}", session.id);
    }
    Ok(())
}

/// Assemble the prompt, following codex exec's stdin semantics:
/// - positional prompt (or `-` sentinel / none) → read stdin;
/// - piped stdin alongside a positional prompt → appended as a
///   `<stdin>` block.
fn resolve_prompt(arg_prompt: Option<String>) -> Result<String, String> {
    let stdin_is_tty = std::io::stdin().is_terminal();
    match arg_prompt {
        Some(prompt) if prompt != "-" => {
            if stdin_is_tty {
                return Ok(prompt);
            }
            let mut piped = String::new();
            let _ = std::io::stdin().read_to_string(&mut piped);
            if piped.trim().is_empty() {
                return Ok(prompt);
            }
            let mut combined = format!("{prompt}\n\n<stdin>\n{piped}");
            if !piped.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str("</stdin>");
            Ok(combined)
        }
        from_stdin => {
            if from_stdin.is_none() && stdin_is_tty {
                return Err(
                    "no prompt provided — pass one as an argument or pipe it into stdin"
                        .to_string(),
                );
            }
            let mut prompt = String::new();
            let _ = std::io::stdin().read_to_string(&mut prompt);
            if prompt.trim().is_empty() {
                return Err("prompt (read from stdin) is empty".to_string());
            }
            Ok(prompt)
        }
    }
}

/// Write the final agent message to the `-o` file. Codex behavior: the
/// file is written even when no message exists (empty content + stderr
/// warning), so scripts always have an artifact.
fn write_last_message(path: Option<&Path>, message: Option<&str>) {
    let Some(path) = path else { return };
    let content = message.unwrap_or("");
    if message.is_none() {
        eprintln!(
            "Warning: no last agent message; wrote empty content to {}",
            path.display()
        );
    }
    if let Err(e) = std::fs::write(path, content) {
        eprintln!("Failed to write last message file {}: {e}", path.display());
    }
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod run_tests;
