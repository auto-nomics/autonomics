//! dag_history_cli —— flow 运行审计与工作区持久化命令行工具。
//!
//! 子命令：
//!   dag_history_cli runs [--ref NAME] [--limit N]
//!       列出最近的运行记录（默认 ref 不过滤, limit=20）
//!   dag_history_cli run <run_id>
//!       查看单次运行的详情（含 run report 摘要）
//!   dag_history_cli flow <agent_id>
//!       打印某 agent 持久化的 authored flow JSON
//!   dag_history_cli flow-cache <agent_id>
//!       打印某 agent 的实例 resume 缓存 JSON
//!
//! 全局选项：
//!   --db <path>    指定数据库路径（默认 .autonomics/dag-history.db）
//!
//! 运行示例：
//!   cargo run -p data-engine --bin dag_history_cli -- runs --limit 10
//!   cargo run -p data-engine --bin dag_history_cli -- run 9a5c…
//!   cargo run -p data-engine --bin dag_history_cli -- flow researcher

use data_engine::dag::DagError;
use data_engine::dag::history::DagHistory;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, BoxErr>;

const DEFAULT_DB: &str = ".autonomics/dag-history.db";
const SHORT_ID_LEN: usize = 12;

fn usage() -> ! {
    eprintln!(
        "用法: dag_history_cli [--db <path>] <subcommand> [args]\n\
         子命令: runs, run, flow, flow-cache\n\
         详见源码顶部文档。"
    );
    std::process::exit(2);
}

/// 取 id 的前 12 个字符（类似 git short hash）。
fn short(id: &str) -> &str {
    if id.len() <= SHORT_ID_LEN {
        id
    } else {
        &id[..SHORT_ID_LEN]
    }
}

// ════════════════════════════════════════════════════════════════
// 全局参数解析
// ════════════════════════════════════════════════════════════════

/// 全局 + 子命令参数。
struct Args {
    db_path: String,
    cmd: String,
    rest: Vec<String>,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_path = DEFAULT_DB.to_string();
    let mut positional = Vec::new();

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db" => {
                i += 1;
                db_path = raw.get(i).cloned().unwrap_or_else(|| usage());
            }
            "-h" | "--help" => usage(),
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    let cmd = positional.first().cloned().unwrap_or_else(|| usage());
    let rest = positional[1..].to_vec();
    Args { db_path, cmd, rest }
}

/// 从 rest 中提取 `--flag value` 并返回剩余的位置参数。
fn extract_flag<'a>(rest: &'a [String], flag: &str, default: &'a str) -> (&'a str, Vec<&'a str>) {
    let mut value = default;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == flag {
            i += 1;
            if let Some(v) = rest.get(i) {
                value = v.as_str();
            }
        } else {
            positional.push(rest[i].as_str());
        }
        i += 1;
    }
    (value, positional)
}

fn err_to_box(e: DagError) -> BoxErr {
    e.into()
}

// ════════════════════════════════════════════════════════════════
// 子命令实现
// ════════════════════════════════════════════════════════════════

/// `runs [--ref NAME] [--limit N]` — 列出最近的运行记录。
async fn cmd_runs(history: &DagHistory, rest: &[String]) -> Result<()> {
    let (ref_name, positional) = extract_flag(rest, "--ref", "");
    let limit_str = positional
        .iter()
        .find(|s| s.starts_with("--limit"))
        .and_then(|s| s.split('=').nth(1))
        .unwrap_or("20");
    let limit: usize = limit_str.parse().unwrap_or(20);

    let ref_filter = (!ref_name.is_empty()).then_some(ref_name);
    let runs = history
        .list_runs(limit, ref_filter)
        .await
        .map_err(err_to_box)?;

    if runs.is_empty() {
        println!("（没有运行记录）");
        return Ok(());
    }

    println!(
        "{:<14} {:<8} {:<26} {:<14} {:<22} TRIGGER",
        "RUN", "OK", "FINISHED", "REF", "MANIFEST"
    );
    println!("{}", "─".repeat(100));

    for run in &runs {
        println!(
            "{:<14} {:<8} {:<26} {:<14} {:<22} {}",
            short(&run.id),
            if run.ok {
                "yes"
            } else if run.cancelled {
                "cancel"
            } else {
                "FAIL"
            },
            &run.finished_at[..26.min(run.finished_at.len())],
            run.ref_name,
            short(&run.manifest_hash),
            run.trigger.as_deref().unwrap_or("(internal)"),
        );
    }

    println!("\n{} 条运行记录（limit={limit}）", runs.len());
    Ok(())
}

/// `run <run_id>` — 查看单次运行详情。
async fn cmd_run(history: &DagHistory, rest: &[String]) -> Result<()> {
    let id = rest.first().ok_or_else(|| BoxErr::from("run 需要 run id"))?;

    let run = history
        .get_run(id.trim())
        .await
        .map_err(err_to_box)?
        .ok_or_else(|| BoxErr::from(format!("运行 '{id}' 不存在")))?;

    println!("Run id:         {}", run.id);
    println!("Ref:            {}", run.ref_name);
    println!("Manifest hash:  {}", run.manifest_hash);
    println!("Trigger:        {}", run.trigger.as_deref().unwrap_or("(internal)"));
    println!("Started:        {}", run.started_at);
    println!("Finished:       {}", run.finished_at);
    println!("OK:             {}（cancelled: {}）", run.ok, run.cancelled);
    if let Some(error) = &run.error {
        println!("Error:          {error}");
    }
    if let Some(message) = &run.message {
        println!("Message:        {message}");
    }
    println!("Engine version: {}（rev {}）", run.engine_version, run.source_revision);

    match &run.run_report_json {
        Some(json) => {
            let value: serde_json::Value = serde_json::from_str(json)
                .map_err(|e| BoxErr::from(format!("run report 解析失败: {e}")))?;
            println!("\nRun report 摘要:");
            if let Some(ok) = value.get("ok") {
                println!("  ok: {ok}");
            }
            if let Some(processes) = value.get("processes").and_then(|p| p.as_object()) {
                for (name, summary) in processes {
                    println!(
                        "  process '{name}': total={}, ok={}, failed={}, skipped={}, reused={}",
                        summary.get("total").and_then(|v| v.as_u64()).unwrap_or(0),
                        summary.get("succeeded").and_then(|v| v.as_u64()).unwrap_or(0),
                        summary.get("failed").and_then(|v| v.as_u64()).unwrap_or(0),
                        summary.get("skipped").and_then(|v| v.as_u64()).unwrap_or(0),
                        summary.get("reused").and_then(|v| v.as_u64()).unwrap_or(0),
                    );
                }
            }
            println!("\n完整 run report JSON:");
            println!("{json}");
        }
        None => println!("\n（本次运行没有 run report）"),
    }
    Ok(())
}

/// `flow <agent_id>` — 打印持久化的 authored flow。
async fn cmd_flow(history: &DagHistory, rest: &[String]) -> Result<()> {
    let agent = rest.first().ok_or_else(|| BoxErr::from("flow 需要 agent id"))?;
    let flow = history
        .load_flow_workspace(agent)
        .await
        .map_err(err_to_box)?;
    match flow {
        Some(json) => {
            let value: serde_json::Value = serde_json::from_str(&json)
                .map_err(|e| BoxErr::from(format!("flow JSON 解析失败: {e}")))?;
            println!("{value:#}");
        }
        None => println!("（agent '{agent}' 没有持久化的 flow）"),
    }
    Ok(())
}

/// `flow-cache <agent_id>` — 打印实例 resume 缓存。
async fn cmd_flow_cache(history: &DagHistory, rest: &[String]) -> Result<()> {
    let agent = rest
        .first()
        .ok_or_else(|| BoxErr::from("flow-cache 需要 agent id"))?;
    let cache = history
        .load_flow_cache(agent)
        .await
        .map_err(err_to_box)?;
    match cache {
        Some(json) => {
            let value: serde_json::Value = serde_json::from_str(&json)
                .map_err(|e| BoxErr::from(format!("cache JSON 解析失败: {e}")))?;
            println!("{value:#}");
        }
        None => println!("（agent '{agent}' 没有 resume 缓存）"),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args();

    let history = DagHistory::open(&args.db_path)
        .await
        .map_err(|e| BoxErr::from(format!("打开数据库失败 ({}): {e}", args.db_path)))?;

    match args.cmd.as_str() {
        "runs" => cmd_runs(&history, &args.rest).await,
        "run" => cmd_run(&history, &args.rest).await,
        "flow" => cmd_flow(&history, &args.rest).await,
        "flow-cache" => cmd_flow_cache(&history, &args.rest).await,
        "" => usage(),
        other => {
            eprintln!("未知子命令: {other}");
            usage();
        }
    }
}
