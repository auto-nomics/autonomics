//! dag_history_cli —— DAG 快照历史命令行工具。
//!
//! 子命令：
//!   dag_history_cli log [--ref NAME] [--limit N]
//!       查看快照历史（默认 ref=main, limit=20）
//!   dag_history_cli show <snapshot_id>
//!       查看某次快照的详情
//!   dag_history_cli head [--ref NAME]
//!       查看某 ref 当前指向的快照
//!   dag_history_cli refs
//!       列出所有 ref（branch / tag）
//!   dag_history_cli manifest <snapshot_id>
//!       打印快照的 manifest（nodes + edges），pretty JSON
//!   dag_history_cli diff <old_id> <new_id>
//!       对比两次快照的 manifest 差异
//!   dag_history_cli branch <new_name> [--from REF]
//!       从 ref 创建新分支（默认 from=main）
//!   dag_history_cli restore <snapshot_id> [--ref NAME]
//!       将 ref 回退到历史快照（默认 ref=main）
//!
//! 全局选项：
//!   --db <path>    指定数据库路径（默认 .autonomics/dag-history.db）
//!
//! 运行示例：
//!   cargo run -p data-engine --bin dag_history_cli -- log --limit 10
//!   cargo run -p data-engine --bin dag_history_cli -- refs
//!   cargo run -p data-engine --bin dag_history_cli -- diff abc123 def456
//!   cargo run -p data-engine --bin dag_history_cli -- log --db /path/to/custom.db

use std::collections::HashMap as StdHashMap;

use data_engine::dag::history::{DagHistory, DagManifest, Snapshot};
use data_engine::dag::DagError;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, BoxErr>;

const DEFAULT_DB: &str = ".autonomics/dag-history.db";
const DEFAULT_REF: &str = "main";
const SHORT_HASH_LEN: usize = 12;

fn usage() -> ! {
    eprintln!(
        "用法: dag_history_cli [--db <path>] <subcommand> [args]\n\
         子命令: log, show, head, refs, manifest, diff, branch, restore\n\
         详见源码顶部文档。"
    );
    std::process::exit(2);
}

/// 取 id 的前 12 个字符（类似 git short hash）。
fn short(id: &str) -> &str {
    if id.len() <= SHORT_HASH_LEN {
        id
    } else {
        &id[..SHORT_HASH_LEN]
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
                db_path = raw
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| usage());
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

// ════════════════════════════════════════════════════════════════
// 子命令实现
// ════════════════════════════════════════════════════════════════

/// `log [--ref NAME] [--limit N]` — 打印快照历史。
async fn cmd_log(history: &DagHistory, rest: &[String]) -> Result<()> {
    let (ref_name, positional) = extract_flag(rest, "--ref", DEFAULT_REF);
    let limit_str = positional
        .iter()
        .find(|s| s.starts_with("--limit"))
        .and_then(|s| s.split('=').nth(1))
        .unwrap_or("20");
    let limit: usize = limit_str.parse().unwrap_or(20);

    let snapshots = history
        .log(ref_name, limit)
        .await
        .map_err(err_to_box)?;

    if snapshots.is_empty() {
        println!("（ref '{ref_name}' 没有快照）");
        return Ok(());
    }

    println!("ref: {ref_name}\n");
    println!(
        "{:<14} {:<26} {:<14} {}",
        "SNAPSHOT", "TIMESTAMP", "MANIFEST", "MESSAGE"
    );
    println!("{}", "─".repeat(90));

    for snap in &snapshots {
        println!(
            "{:<14} {:<26} {:<14} {}",
            short(&snap.id),
            &snap.timestamp[..26.min(snap.timestamp.len())],
            short(&snap.manifest_hash),
            snap.message,
        );
    }

    // 额外统计
    println!(
        "\n{} 个快照（limit={}, 距离根节点 {} 代）",
        snapshots.len(),
        limit,
        snapshots.len().saturating_sub(1)
    );
    Ok(())
}

/// `show <snapshot_id>` — 查看单个快照详情。
async fn cmd_show(history: &DagHistory, rest: &[String]) -> Result<()> {
    let id = rest
        .first()
        .ok_or_else(|| BoxErr::from("show 需要快照 id"))?;

    let snap = resolve_snapshot(history, id).await?;
    let snap = snap.ok_or_else(|| BoxErr::from(format!("快照 '{id}' 不存在")))?;

    println!("Snapshot:       {}", snap.id);
    println!("Parent:         {}", snap.parent_id.as_deref().unwrap_or("(root)"));
    println!("Manifest hash:  {}", snap.manifest_hash);
    println!("Timestamp:      {}", snap.timestamp);
    println!("Message:        {}", snap.message);
    println!("Engine version: {}", snap.engine_version);
    println!("Run report:     {}", if snap.run_report_json.is_some() { "有" } else { "无" });

    // Manifest 概要
    if let Ok(manifest) = snap.manifest() {
        println!("\nManifest 概要:");
        println!("  节点: {} 个", manifest.nodes.len());
        for n in &manifest.nodes {
            println!("    • {} ({})", n.id, n.kind);
        }
        println!("  边: {} 条", manifest.edges.len());
        for e in &manifest.edges {
            println!("    • {}.[{}] → {}.[{}]", e.from, e.from_port, e.to, e.to_port);
        }
    }
    Ok(())
}

/// `head [--ref NAME]` — 查看 ref 当前指向的快照。
async fn cmd_head(history: &DagHistory, rest: &[String]) -> Result<()> {
    let (ref_name, _) = extract_flag(rest, "--ref", DEFAULT_REF);

    let snap = history
        .ref_head(ref_name)
        .await
        .map_err(err_to_box)?;

    match snap {
        Some(s) => {
            println!("ref '{ref_name}' → {}", s.id);
            println!("  消息:     {}", s.message);
            println!("  时间:     {}", s.timestamp);
            println!("  manifest: {} (短: {})", s.manifest_hash, short(&s.manifest_hash));
            println!("  parent:   {}", s.parent_id.as_deref().unwrap_or("(root)"));
        }
        None => {
            println!("ref '{ref_name}' 不存在或无快照");
        }
    }
    Ok(())
}

/// `refs` — 列出所有 ref。
async fn cmd_refs(history: &DagHistory) -> Result<()> {
    let refs = history.list_refs().await.map_err(err_to_box)?;

    if refs.is_empty() {
        println!("（没有任何 ref）");
        return Ok(());
    }

    println!("{:<20} {:<14} {}", "NAME", "SNAPSHOT", "TYPE");
    println!("{}", "─".repeat(50));
    for (name, snap_id, pinned) in &refs {
        let ty = if *pinned { "tag" } else { "branch" };
        println!("{:<20} {:<14} {}", name, short(snap_id), ty);
    }
    Ok(())
}

/// `manifest <snapshot_id>` — 打印快照的 manifest（pretty JSON）。
async fn cmd_manifest(history: &DagHistory, rest: &[String]) -> Result<()> {
    let id = rest
        .first()
        .ok_or_else(|| BoxErr::from("manifest 需要快照 id"))?;

    let snap = resolve_snapshot(history, id).await?;
    let snap = snap.ok_or_else(|| BoxErr::from(format!("快照 '{id}' 不存在")))?;

    let manifest = snap
        .manifest()
        .map_err(|e| BoxErr::from(format!("manifest 反序列化失败: {e}")))?;

    let pretty = serde_json::to_string_pretty(&manifest)
        .map_err(|e| BoxErr::from(format!("JSON 序列化失败: {e}")))?;

    println!("{pretty}");
    Ok(())
}

/// `diff <old_id> <new_id>` — 对比两次快照的 manifest。
async fn cmd_diff(history: &DagHistory, rest: &[String]) -> Result<()> {
    let old_id = rest
        .first()
        .ok_or_else(|| BoxErr::from("diff 需要 <old_id> <new_id>"))?;
    let new_id = rest
        .get(1)
        .ok_or_else(|| BoxErr::from("diff 需要 <old_id> <new_id>"))?;

    let old_snap = resolve_snapshot(history, old_id)
        .await?
        .ok_or_else(|| BoxErr::from(format!("快照 '{old_id}' 不存在")))?;
    let new_snap = resolve_snapshot(history, new_id)
        .await?
        .ok_or_else(|| BoxErr::from(format!("快照 '{new_id}' 不存在")))?;

    let old_m = old_snap.manifest().map_err(|e| BoxErr::from(e.to_string()))?;
    let new_m = new_snap.manifest().map_err(|e| BoxErr::from(e.to_string()))?;

    diff_manifests(&old_m, &new_m);
    Ok(())
}

/// `branch <name> [--from REF]` — 创建分支。
async fn cmd_branch(history: &DagHistory, rest: &[String]) -> Result<()> {
    let (from_ref, positional) = extract_flag(rest, "--from", DEFAULT_REF);
    let name = positional
        .first()
        .ok_or_else(|| BoxErr::from("branch 需要 <name>"))?;

    history
        .branch(name, from_ref)
        .await
        .map_err(err_to_box)?;

    println!("已创建分支 '{name}' ← {from_ref}");
    Ok(())
}

/// `restore <snapshot_id> [--ref NAME]` — 回退 ref 到历史快照。
async fn cmd_restore(history: &DagHistory, rest: &[String]) -> Result<()> {
    let (ref_name, positional) = extract_flag(rest, "--ref", DEFAULT_REF);
    let target = positional
        .first()
        .ok_or_else(|| BoxErr::from("restore 需要 <snapshot_id>"))?;

    // 先确认快照存在
    let snap = resolve_snapshot(history, target).await?;
    let snap = snap.ok_or_else(|| BoxErr::from(format!("快照 '{target}' 不存在")))?;

    history
        .reset(ref_name, &snap.id)
        .await
        .map_err(err_to_box)?;

    println!("ref '{ref_name}' 已回退到 {} ({})", short(&snap.id), snap.message);
    Ok(())
}

// ════════════════════════════════════════════════════════════════
// 辅助函数
// ════════════════════════════════════════════════════════════════

/// 将 DagError 转为 BoxErr。
fn err_to_box(e: DagError) -> BoxErr {
    BoxErr::from(e.to_string())
}

/// 解析快照 id（支持短 hash 前缀匹配）。
async fn resolve_snapshot(
    history: &DagHistory,
    id_or_prefix: &str,
) -> Result<Option<Snapshot>> {
    // 先尝试精确匹配
    if let Ok(Some(snap)) = history.get_snapshot(id_or_prefix).await {
        return Ok(Some(snap));
    }

    // 短 hash 前缀匹配：查最近的快照历史
    // 先从 main ref 的 lineage 里找
    let snapshots = history.log("main", 1000).await.map_err(err_to_box)?;
    for snap in snapshots {
        if snap.id.starts_with(id_or_prefix) {
            return Ok(Some(snap));
        }
    }

    // 也查其他 refs 的 heads
    let refs = history.list_refs().await.map_err(err_to_box)?;
    for (ref_name, _, _) in &refs {
        if ref_name == "main" {
            continue;
        }
        let snaps = history.log(ref_name, 200).await.map_err(err_to_box)?;
        for snap in snaps {
            if snap.id.starts_with(id_or_prefix) {
                return Ok(Some(snap));
            }
        }
    }

    Ok(None)
}

/// 对比两个 manifest 并打印差异。
fn diff_manifests(old: &DagManifest, new: &DagManifest) {
    let old_nodes: StdHashMap<&str, &data_engine::dag::history::NodeEntry> = old
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    let new_nodes: StdHashMap<&str, &data_engine::dag::history::NodeEntry> = new
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();

    let old_edges: std::collections::HashSet<String> = old
        .edges
        .iter()
        .map(|e| format!("{}.[{}] → {}.[{}]", e.from, e.from_port, e.to, e.to_port))
        .collect();
    let new_edges: std::collections::HashSet<String> = new
        .edges
        .iter()
        .map(|e| format!("{}.[{}] → {}.[{}]", e.from, e.from_port, e.to, e.to_port))
        .collect();

    let mut changes = 0;

    // 新增节点
    for n in &new.nodes {
        if !old_nodes.contains_key(n.id.as_str()) {
            println!("  + node {} ({})", n.id, n.kind);
            changes += 1;
        }
    }
    // 删除节点
    for n in &old.nodes {
        if !new_nodes.contains_key(n.id.as_str()) {
            println!("  - node {} ({})", n.id, n.kind);
            changes += 1;
        }
    }
    // 修改节点（kind 或 spec 变了）
    for n in &new.nodes {
        if let Some(old_n) = old_nodes.get(n.id.as_str()) {
            if old_n.kind != n.kind || old_n.spec != n.spec {
                println!("  ~ node {} ({})", n.id, n.kind);
                // 简要显示 spec 变化
                let old_keys: std::collections::HashSet<&str> =
                    old_n.spec.as_object().iter().flat_map(|m| m.keys()).map(|s| s.as_str()).collect();
                let new_keys: std::collections::HashSet<&str> =
                    n.spec.as_object().iter().flat_map(|m| m.keys()).map(|s| s.as_str()).collect();
                for k in new_keys.difference(&old_keys) {
                    println!("      + {k}");
                }
                for k in old_keys.difference(&new_keys) {
                    println!("      - {k}");
                }
                changes += 1;
            }
        }
    }

    // 新增边
    for e in new_edges.difference(&old_edges) {
        println!("  + edge {e}");
        changes += 1;
    }
    // 删除边
    for e in old_edges.difference(&new_edges) {
        println!("  - edge {e}");
        changes += 1;
    }

    if changes == 0 {
        println!("（无差异）");
    } else {
        println!("\n共 {changes} 处变更");
    }
}

// ════════════════════════════════════════════════════════════════
// main
// ════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args();

    let history = DagHistory::open(&args.db_path)
        .await
        .map_err(|e| BoxErr::from(format!("打开数据库失败 ({}): {e}", args.db_path)))?;

    match args.cmd.as_str() {
        "log" => cmd_log(&history, &args.rest).await,
        "show" => cmd_show(&history, &args.rest).await,
        "head" => cmd_head(&history, &args.rest).await,
        "refs" => cmd_refs(&history).await,
        "manifest" => cmd_manifest(&history, &args.rest).await,
        "diff" => cmd_diff(&history, &args.rest).await,
        "branch" => cmd_branch(&history, &args.rest).await,
        "restore" => cmd_restore(&history, &args.rest).await,
        "" => usage(),
        other => {
            eprintln!("未知子命令: {other}");
            usage();
        }
    }
}
