use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use clap::Parser;
use data_engine::data_engine::DataEngine;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

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
                "input/{}",
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

fn read_mean(path: &Path) -> Result<f64, Box<dyn Error>> {
    let mut reader = csv::Reader::from_path(path)?;
    let headers = reader.headers()?.clone();
    if headers.len() != 1 || headers.get(0) != Some("mean") {
        return Err(format!(
            "unexpected DAG sink schema: expected one mean column, got {}",
            headers.iter().collect::<Vec<_>>().join(",")
        )
        .into());
    }

    let mut records = reader.records();
    let Some(first) = records.next().transpose()? else {
        return Err("DAG sink contains no rows".into());
    };
    if records.next().is_some() {
        return Err("DAG sink contains more than one row".into());
    }
    Ok(first
        .get(0)
        .ok_or("DAG sink row is missing mean value")?
        .parse()?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let (metadata, visible) = read_task(&args.task_dir)?;
    if metadata.id != "smoke_schema_001" {
        return Err(format!(
            "the deterministic Autonomics adapter does not yet implement task {}",
            metadata.id
        )
        .into());
    }

    fs::create_dir_all(&args.run_dir)?;
    let staged = stage_inputs(&args.task_dir, &args.run_dir, &visible)?;
    let Some(input) = staged.first() else {
        return Err("smoke task has no visible input".into());
    };

    let workflow = json!({
        "format": "dag-json",
        "nodes": [
            {
                "id": "read",
                "kind": "file_to_dataframe",
                "spec": {
                    "path": input.virtual_path,
                    "format": "csv"
                }
            },
            {
                "id": "mean",
                "kind": "sql",
                "spec": {
                    "sql_query": "SELECT AVG(value) AS mean FROM port_0"
                }
            },
            {
                "id": "write",
                "kind": "dataframe_to_file",
                "spec": {
                    "path": "artifacts/mean.csv",
                    "format": "csv",
                    "mode": "overwrite"
                }
            }
        ],
        "edges": [
            {"from": "read", "from_port": 0, "to": "mean", "to_port": 0},
            {"from": "mean", "from_port": 0, "to": "write", "to_port": 0}
        ]
    });

    let nodes = workflow["nodes"].as_array().unwrap().clone();
    let mut execution_nodes = nodes.clone();
    execution_nodes[0]["spec"]["path"] = json!(args.run_dir.join(&input.virtual_path));
    execution_nodes[2]["spec"]["path"] = json!(args.run_dir.join("artifacts").join("mean.csv"));

    let mut engine = DataEngine::builder().build();
    for node in &execution_nodes {
        engine.add_node_from_registry(
            node["id"].as_str().unwrap(),
            node["kind"].as_str().unwrap(),
            node["spec"].clone(),
        )?;
    }
    for edge in workflow["edges"].as_array().unwrap() {
        engine.add_edge(
            edge["from"].as_str().unwrap(),
            edge["to"].as_str().unwrap(),
            edge["from_port"].as_u64().unwrap().try_into()?,
            edge["to_port"].as_u64().unwrap().try_into()?,
        )?;
    }

    let started = Instant::now();
    let report = engine.run().await?;
    let elapsed = started.elapsed().as_secs_f64();
    if !report.ok {
        return Err(format!("Autonomics DAG failed: {:?}", report.errors).into());
    }

    let artifact_path = args.run_dir.join("artifacts").join("mean.csv");
    let mean = read_mean(&artifact_path)?;
    let answer = json!({"mean": mean});
    write_json(&args.run_dir.join("answer.json"), &answer)?;
    write_json(&args.run_dir.join("workflow.json"), &workflow)?;

    let answer_path = args.run_dir.join("answer.json");
    let answer_digest = sha256_file(&answer_path)?;
    let artifact_digest = sha256_file(&artifact_path)?;
    let prompt_hash = hex(&Sha256::digest(visible.task.as_bytes()));
    let run_id = args
        .run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("run directory has no valid final path component")?
        .to_string();

    let audit_manifest = json!({
        "run_id": run_id,
        "task_id": metadata.id,
        "adapter": "autonomics",
        "seed": args.seed,
        "model": {
            "id": "autonomics-data-engine",
            "provider": "autonomics",
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
            "format": "dag-json",
            "path": "workflow.json",
            "nodes": workflow["nodes"],
            "edges": workflow["edges"]
        },
        "outputs": [
            {
                "path": "answer.json",
                "sha256": answer_digest,
                "produced_by": "mean"
            },
            {
                "path": "artifacts/mean.csv",
                "sha256": artifact_digest,
                "produced_by": "write"
            }
        ],
        "execution": {
            "status": "succeeded",
            "wall_time_secs": elapsed,
            "token_count": 0,
            "tool_calls": nodes.len(),
            "container_policy": {
                "network": "not_applicable",
                "read_only_rootfs": true
            },
            "dag_run_report": serde_json::to_value(&report)?
        },
        "errors": []
    });
    write_json(&args.run_dir.join("audit_manifest.json"), &audit_manifest)?;

    Ok(())
}
