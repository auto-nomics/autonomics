use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Parser;
use headless::processor::{JsonlProcessor, OutputProcessor};
use headless::{run_task, RunSummary, RunTaskConfig};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

/// Wall-clock budget for the agent run unless overridden.
const DEFAULT_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Parser)]
struct Args {
    /// Task package containing task.json and visible data.
    #[arg(long)]
    task_dir: PathBuf,

    /// Empty or reusable ReproBioBench run directory.
    #[arg(long)]
    run_dir: PathBuf,

    /// Deterministic seed retained in the public audit manifest.
    #[arg(long)]
    seed: i64,

    /// Model as `provider_name:model_name`. Defaults to
    /// `$AUTONOMICS_MODEL`, then the active model in the app database.
    #[arg(long)]
    model: Option<String>,

    /// Wall-clock budget for the agent run, in seconds.
    #[arg(long)]
    timeout: Option<u64>,
}

#[derive(Debug)]
struct TaskMetadata {
    id: String,
}

#[derive(Debug, Deserialize)]
struct VisibleTask {
    task: String,
    data_files: Vec<PathBuf>,
    #[serde(default)]
    input_manifest: BTreeMap<String, String>,
}

#[derive(Debug)]
struct StagedInput {
    virtual_path: String,
    sha256: String,
}

fn read_task(task_dir: &Path) -> Result<(TaskMetadata, VisibleTask), Box<dyn Error>> {
    let raw = fs::read_to_string(task_dir.join("task.json"))?;
    // Deliberately extract only public task fields; grader and hidden fields
    // are never deserialized by this adapter.
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("task.json is missing id")?
        .to_string();
    let metadata = TaskMetadata { id };
    let visible: VisibleTask = serde_json::from_value(
        value
            .get("visible_to_agent")
            .cloned()
            .ok_or("task.json is missing visible_to_agent")?,
    )?;
    Ok((metadata, visible))
}

fn validate_relative_path(path: &Path) -> Result<(), Box<dyn Error>> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::CurDir | Component::RootDir
            )
        })
        || path.as_os_str().is_empty()
    {
        return Err(format!("unsafe visible data path: {}", path.display()).into());
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, Box<dyn Error>> {
    let mut hasher = Sha256::new();
    let bytes = fs::read(path)?;
    hasher.update(bytes);
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn stage_inputs(
    task_dir: &Path,
    run_dir: &Path,
    visible: &VisibleTask,
) -> Result<Vec<StagedInput>, Box<dyn Error>> {
    let mut staged = Vec::new();
    for relative_path in &visible.data_files {
        validate_relative_path(relative_path)?;
        let source = task_dir.join(relative_path);
        let target = run_dir.join("input").join(relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        if !target.exists() {
            fs::copy(&source, &target)?;
        }

        let digest = sha256_file(&target)?;
        match visible.input_manifest.get(
            relative_path
                .to_str()
                .ok_or("visible data path is not valid UTF-8")?,
        ) {
            Some(expected) if &digest != expected => {
                return Err(format!(
                    "input checksum mismatch for {}: expected {expected}, got {digest}",
                    relative_path.display()
                )
                .into());
            }
            _ => {}
        }

        staged.push(StagedInput {
            virtual_path: format!(
                "/input/{}",
                relative_path
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            ),
            sha256: digest,
        });
    }
    Ok(staged)
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

/// Build the agent's prompt: the visible task plus the workspace/output
/// contract the grader relies on.
fn build_prompt(visible: &VisibleTask, staged: &[StagedInput]) -> String {
    let files = staged
        .iter()
        .map(|input| format!("  - {}", input.virtual_path))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n\n---\nWorkspace contract:\n\
         - Input files (read-only reference):\n{files}\n\
         - Write your final answer as a single JSON object to `/answer.json`.\n\
         - Write any intermediate artifacts under `/artifacts/`.\n",
        visible.task
    )
}

/// Prepare an isolated runtime state dir under the run directory, with a
/// VFS that mounts the whole run dir at `/` so the agent's `/input/...`,
/// `/answer.json`, and `/artifacts/...` paths land in the audited tree.
fn prepare_state(run_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let state_dir = run_dir.join("state");
    let data_dir = run_dir.join("data");
    fs::create_dir_all(&state_dir)?;
    fs::create_dir_all(&data_dir)?;

    let manifest = vfs::VfsManifest {
        backend: vec![vfs::BackendDefinition {
            id: "work".into(),
            config: vfs::BackendConfig::local(run_dir.to_string_lossy()),
        }],
        mount: vec![vfs::MountDefinition {
            path: "/".into(),
            backend: "work".into(),
            source: "/".into(),
            read_only: false,
        }],
    };
    fs::write(
        state_dir.join("vfs.toml"),
        toml::to_string_pretty(&manifest)?,
    )?;
    Ok(state_dir)
}

/// Resolve the run's model from `--model`, `$AUTONOMICS_MODEL`, or the
/// app database's active model. Returns (model, display name, provider).
fn resolve_model(spec: Option<String>) -> Result<(agentik_sdk::model::Model, String, String), String> {
    let spec = match spec.or_else(|| std::env::var("AUTONOMICS_MODEL").ok()) {
        Some(spec) => spec,
        None => {
            let config = runtime::RuntimeConfig::default();
            if let Some(parent) = config.app_db_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match rusqlite::Connection::open(&config.app_db_path) {
                Ok(conn) => {
                    let _ = runtime::model_bootstrap::ensure_app_schema(&conn);
                    runtime::model_bootstrap::active_model_spec(&conn)
                }
                Err(_) => None,
            }
            .ok_or_else(|| {
                "no model: pass --model provider_name:model_name, set AUTONOMICS_MODEL, \
                 or configure an active model in the app database"
                    .to_string()
            })?
        }
    };
    let (provider, name) = spec
        .split_once(':')
        .ok_or_else(|| format!("model spec `{spec}` must be `provider_name:model_name`"))?;
    let conn_result = (|| -> Result<agentik_sdk::model::Model, String> {
        let config = runtime::RuntimeConfig::default();
        if let Some(parent) = config.app_db_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&config.app_db_path)
            .map_err(|e| format!("open app db: {e}"))?;
        runtime::model_bootstrap::ensure_app_schema(&conn)
            .map_err(|e| format!("init app db schema: {e}"))?;
        runtime::model_bootstrap::resolve_model_spec(&conn, &spec)
            .ok_or_else(|| format!("model `{spec}` unavailable (missing credentials?)"))
    })();
    let model = conn_result?;
    Ok((model, name.to_string(), provider.to_string()))
}

/// Outcome of the agent run as recorded in the audit manifest.
fn execution_status(summary: &RunSummary, answer_present: bool) -> &'static str {
    match summary.outcome {
        headless::processor::Outcome::Completed if answer_present => "succeeded",
        headless::processor::Outcome::Completed => "no_answer",
        headless::processor::Outcome::Failed => "failed",
        headless::processor::Outcome::Cancelled => "cancelled",
        headless::processor::Outcome::Unknown => "unknown",
    }
}

/// Run the task through the headless agent runtime. Split from `main` so
/// tests can inject a scripted mock model.
async fn execute(
    prompt: String,
    run_dir: &Path,
    model: agentik_sdk::model::Model,
    model_name: String,
    timeout: Option<u64>,
) -> Result<RunSummary, Box<dyn Error>> {
    let state_dir = prepare_state(run_dir)?;
    let mut runtime_config = runtime::RuntimeConfig::default();
    // Every persistent path moves under the run dir: benchmark runs must
    // not write agents/sessions into the host installation's databases
    // (`RuntimeConfig::default()` bakes absolute ~/.autonomics paths).
    runtime_config.state_dir = state_dir.clone();
    runtime_config.data_dir = run_dir.join("data");
    runtime_config.agent_db = state_dir.join("agents.db");
    runtime_config.dag_history_db = state_dir.join("dag_history.db");
    runtime_config.bib_db_path = state_dir.join("bib.db");
    runtime_config.writing_db_path = state_dir.join("writing.db");
    runtime_config.app_db_path = state_dir.join("app.db");

    let mut config = RunTaskConfig::new(prompt, runtime_config);
    config.model = Some(model);
    config.model_name = Some(model_name);
    config.timeout = Some(Duration::from_secs(timeout.unwrap_or(DEFAULT_TIMEOUT_SECS)));

    // Full event stream lands in the run dir as the workflow audit trail.
    let events_path = run_dir.join("events.jsonl");
    let writer = std::io::BufWriter::new(fs::File::create(&events_path)?);
    let mut processor = JsonlProcessor::new(writer);
    let summary = run_task(config, &mut processor)
        .await
        .map_err(|e| format!("headless run failed to start: {e}"))?;
    processor.finish();
    Ok(summary)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let (metadata, visible) = read_task(&args.task_dir)?;

    fs::create_dir_all(&args.run_dir)?;
    let staged = stage_inputs(&args.task_dir, &args.run_dir, &visible)?;

    let (model, model_name, provider) = match resolve_model(args.model.clone()) {
        Ok(resolved) => resolved,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(3);
        }
    };

    let prompt = build_prompt(&visible, &staged);
    let started = Instant::now();
    let summary = execute(prompt.clone(), &args.run_dir, model, model_name.clone(), args.timeout).await?;

    // Post-run audit: what did the agent actually produce?
    let answer_path = args.run_dir.join("answer.json");
    let answer_present = answer_path.exists();
    let prompt_hash = hex(&Sha256::digest(prompt.as_bytes()));
    let run_id = args
        .run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("run directory has no valid final path component")?
        .to_string();

    let usage = summary.usage;
    let token_count: u64 = usage
        .map(|u| {
            u.output_tokens
                + u.input_tokens.unwrap_or(0)
                + u.cache_read_input_tokens.unwrap_or(0)
                + u.cache_creation_input_tokens.unwrap_or(0)
        })
        .unwrap_or(0);

    let mut outputs = vec![json!({
        "path": "events.jsonl",
        "sha256": sha256_file(&args.run_dir.join("events.jsonl"))?,
        "produced_by": "headless-run"
    })];
    if answer_present {
        outputs.push(json!({
            "path": "answer.json",
            "sha256": sha256_file(&answer_path)?,
            "produced_by": "agent"
        }));
    }
    let artifacts_dir = args.run_dir.join("artifacts");
    if artifacts_dir.is_dir() {
        outputs.push(json!({
            "path": "artifacts/",
            "produced_by": "agent"
        }));
    }

    let audit_manifest = json!({
        "run_id": run_id,
        "task_id": metadata.id,
        "adapter": "autonomics",
        "seed": args.seed,
        "model": {
            "id": model_name,
            "provider": provider,
            "prompt_hash": prompt_hash
        },
        "inputs": staged
            .iter()
            .map(|input| json!({
                "path": input.virtual_path,
                "sha256": input.sha256,
                "kind": "visible_data",
                "produced_by": "benchmark_stager"
            }))
            .collect::<Vec<_>>(),
        "workflow": {
            "format": "agent-events-jsonl",
            "path": "events.jsonl"
        },
        "outputs": outputs,
        "execution": {
            "status": execution_status(&summary, answer_present),
            "wall_time_secs": started.elapsed().as_secs_f64(),
            "token_count": token_count,
            "tool_calls": summary.tool_calls,
            "turns": summary.turns,
            "container_policy": {
                "network": "profile-dependent",
                "read_only_rootfs": false
            }
        },
        "errors": if summary.outcome == headless::processor::Outcome::Completed {
            vec![]
        } else {
            vec![json!({ "kind": "run_outcome", "detail": execution_status(&summary, answer_present) })]
        }
    });
    write_json(&args.run_dir.join("audit_manifest.json"), &audit_manifest)?;

    if !answer_present {
        eprintln!(
            "run finished without /answer.json (status: {})",
            execution_status(&summary, answer_present)
        );
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::model::ModelInfo;
    use agentik_sdk::provider::client::MockApiClient;
    use agentik_sdk::streaming::MessageStream;
    use agentik_types::messages::{ContentBlock, Message, Role, StopReason};
    use agentik_types::streaming::{
        ContentBlockDelta, MessageDelta, MessageDeltaUsage, MessageStreamEvent,
    };
    use std::time::Duration;

    fn mock_model_info() -> ModelInfo {
        ModelInfo {
            model_name: "mock-model".into(),
            provider_id: uuid::Uuid::nil(),
            context_length: 8192,
            max_output_tokens: 1024,
            supports_function_calling: true,
            supports_streaming: true,
            ..Default::default()
        }
    }

    fn scripted_text_model(text: &str) -> agentik_sdk::model::Model {
        let mut mock = MockApiClient::new();
        let start = Message {
            id: "msg_mock".into(),
            type_: "message".into(),
            role: Role::Assistant,
            content: vec![],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        let events = vec![
            MessageStreamEvent::MessageStart {
                message: start.clone(),
            },
            MessageStreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::Text {
                    text: String::new(),
                },
            },
            MessageStreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentBlockDelta::TextDelta {
                    text: text.to_string(),
                },
            },
            MessageStreamEvent::ContentBlockStop { index: 0 },
            MessageStreamEvent::MessageDelta {
                delta: MessageDelta {
                    stop_reason: Some(StopReason::EndTurn),
                    stop_sequence: None,
                },
                usage: MessageDeltaUsage {
                    output_tokens: 5,
                    input_tokens: Some(7),
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    server_tool_use: None,
                },
            },
            MessageStreamEvent::MessageStop,
        ];
        let final_message = Message {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            stop_reason: Some(StopReason::EndTurn),
            ..start
        };
        mock.expect_request_stream_with_system()
            .times(1)
            .returning(move |_, _, _, _| {
                Ok(MessageStream::from_events(events.clone(), final_message.clone()))
            });
        agentik_sdk::model::Model::with_client(mock_model_info(), mock)
    }

    fn write_task(dir: &Path) {
        let input = dir.join("task/input/values.csv");
        fs::create_dir_all(input.parent().unwrap()).unwrap();
        fs::write(&input, "value\n1\n2\n3\n").unwrap();
        let digest = {
            let mut hasher = Sha256::new();
            hasher.update(fs::read(&input).unwrap());
            hex(&hasher.finalize())
        };
        fs::write(
            dir.join("task/task.json"),
            serde_json::to_vec_pretty(&json!({
                "id": "smoke_schema_001",
                "visible_to_agent": {
                    "task": "Compute the mean of the input values.",
                    "data_files": ["input/values.csv"],
                    "input_manifest": { "input/values.csv": digest }
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn run_records_audit_trail_even_without_answer() {
        let tmp = tempfile::tempdir().unwrap();
        write_task(tmp.path());
        let run_dir = tmp.path().join("run-001");
        let (metadata, visible) = read_task(&tmp.path().join("task")).unwrap();
        assert_eq!(metadata.id, "smoke_schema_001");
        let staged = stage_inputs(&tmp.path().join("task"), &run_dir, &visible).unwrap();
        assert_eq!(staged[0].virtual_path, "/input/input/values.csv");

        let summary = execute(
            build_prompt(&visible, &staged),
            &run_dir,
            scripted_text_model("I would compute the mean."),
            "mock-model".into(),
            Some(30),
        )
        .await
        .unwrap();
        assert_eq!(summary.outcome, headless::processor::Outcome::Completed);

        // The audit trail exists: events + state + vfs mount.
        assert!(run_dir.join("events.jsonl").exists());
        assert!(run_dir.join("state/vfs.toml").exists());
        let events = fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
        assert!(events.contains("\"run.started\""));
        // No answer.json from a text-only mock — status reflects it.
        assert!(!run_dir.join("answer.json").exists());
        assert_eq!(execution_status(&summary, false), "no_answer");
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        write_task(tmp.path());
        // Corrupt the input after the manifest was written.
        fs::write(tmp.path().join("task/input/values.csv"), "value\n9\n").unwrap();
        let (_, visible) = read_task(&tmp.path().join("task")).unwrap();
        let err = stage_inputs(&tmp.path().join("task"), &tmp.path().join("run"), &visible)
            .unwrap_err()
            .to_string();
        assert!(err.contains("checksum mismatch"), "{err}");
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut visible = VisibleTask {
            task: "t".into(),
            data_files: vec![PathBuf::from("../escape.csv")],
            input_manifest: BTreeMap::new(),
        };
        assert!(stage_inputs(tmp.path(), &tmp.path().join("run"), &visible).is_err());
        visible.data_files = vec![PathBuf::from("/etc/passwd")];
        assert!(stage_inputs(tmp.path(), &tmp.path().join("run"), &visible).is_err());
    }

    #[test]
    fn prompt_carries_workspace_contract() {
        let visible = VisibleTask {
            task: "Do the thing.".into(),
            data_files: vec![],
            input_manifest: BTreeMap::new(),
        };
        let staged = vec![StagedInput {
            virtual_path: "/input/data.csv".into(),
            sha256: "0".repeat(64),
        }];
        let prompt = build_prompt(&visible, &staged);
        assert!(prompt.contains("Do the thing."));
        assert!(prompt.contains("/input/data.csv"));
        assert!(prompt.contains("/answer.json"));
    }

}

