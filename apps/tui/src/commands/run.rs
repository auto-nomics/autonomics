//! Headless run subcommand — thin CLI shell over `headless::run_task`.
//!
//! Responsibilities are deliberately narrow: prompt assembly (args +
//! stdin), model resolution via `gateway::model_bootstrap`, processor
//! selection (`--json` vs human), the `-o` last-message file, and the
//! exit-code contract. Everything else — spawn, translation,
//! termination, shutdown — belongs to the headless library, so the CLI
//! and any future adapter share one execution path.
//!
//! Exit codes (see docs/headless-run-design.md §3):
//! 0 turn completed · 1 turn failed · 2 run cancelled · 3 startup error.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;

use headless::processor::{HumanProcessor, JsonlProcessor, OutputProcessor};
use headless::{RunTaskConfig, run_task};
use rusqlite::Connection;

use crate::cli::RunArgs;

const EXIT_COMPLETED: i32 = 0;
const EXIT_TURN_FAILED: i32 = 1;
const EXIT_CANCELLED: i32 = 2;
const EXIT_STARTUP: i32 = 3;

pub fn run_headless(args: RunArgs) -> color_eyre::Result<()> {
    let prompt = match resolve_prompt(args.prompt.clone()) {
        Ok(prompt) => prompt,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(EXIT_STARTUP);
        }
    };

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async move {
        let runtime_config = gateway::RuntimeConfig::default();
        if let Some(parent) = runtime_config.app_db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&runtime_config.app_db_path)?;
        gateway::model_bootstrap::ensure_app_schema(&conn)?;

        let (model, model_name) = match resolve_model(&conn, args.model.as_deref()) {
            Ok(resolved) => resolved,
            Err(message) => {
                eprintln!("error: {message}");
                std::process::exit(EXIT_STARTUP);
            }
        };

        // Ephemeral runs redirect data/state to a throwaway dir that must
        // outlive the block_on below; the model stays resolved from the
        // real app DB either way.
        let (_ephemeral_guard, mut config) = if args.ephemeral {
            let (config, guard) = RunTaskConfig::ephemeral(prompt);
            (Some(guard), config)
        } else {
            (None, RunTaskConfig::new(prompt, runtime_config))
        };
        config.profile = args.profile.clone();
        config.model = Some(model);
        config.model_name = Some(model_name);
        config.timeout = args.timeout.map(std::time::Duration::from_secs);
        config.session = args.session;
        let meta = headless::manifest::ManifestMeta {
            run_id: uuid::Uuid::new_v4(),
            prompt_hash: headless::manifest::prompt_hash(&config.prompt),
            profile: config.profile.clone().unwrap_or_default(),
            model: config.model_name.clone().unwrap_or_default(),
        };

        let summary = if args.json {
            let mut processor = JsonlProcessor::new(std::io::stdout());
            let result = run_task(config, &mut processor).await;
            write_last_message(
                args.output_last_message.as_deref(),
                processor.last_message(),
            );
            result
        } else {
            let mut processor = HumanProcessor::new(std::io::stderr(), std::io::stdout());
            let result = run_task(config, &mut processor).await;
            write_last_message(
                args.output_last_message.as_deref(),
                processor.last_message(),
            );
            result
        };

        match summary {
            Ok(summary) => {
                if let Some(path) = args.manifest.as_deref() {
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
                // Gateway-era interim: the resident daemon owns the state
                // dir, so an in-process default run cannot open the host.
                // The gateway-backed `run` (follow-up PR) removes this
                // conflict entirely.
                if let headless::RunError::HostOpen(gateway::RuntimeError::InstanceLockHeld {
                    path,
                }) = &error
                {
                    eprintln!(
                        "note: the gateway daemon currently owns the state dir ({}); \
                         stop it (`tui serve stop`) or run with --ephemeral",
                        path.display()
                    );
                }
                std::process::exit(EXIT_STARTUP);
            }
        }
    })
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

/// Resolve the run's model: the `--model` spec when given, else the
/// installation's active model. Returns the model and its display name.
fn resolve_model(
    conn: &Connection,
    model_flag: Option<&str>,
) -> Result<(agentik_sdk::model::Model, String), String> {
    let spec = match model_flag {
        Some(spec) => spec.to_string(),
        None => gateway::model_bootstrap::active_model_spec(conn).ok_or_else(|| {
            "no active model configured — set one in the TUI Config tab or pass \
             --model provider_name:model_name"
                .to_string()
        })?,
    };
    let model = gateway::model_bootstrap::resolve_model_spec(conn, &spec).ok_or_else(|| {
        format!("model `{spec}` unavailable (unknown provider/model or missing API key)")
    })?;
    let name = spec
        .split_once(':')
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| spec.clone());
    Ok((model, name))
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
