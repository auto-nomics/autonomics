//! Standard provenance exports of recorded runs.
//!
//! Turns a [`RunRecord`] (+ its run report and snapshot manifest) into
//! deliverable evidence packages:
//!
//! - **W3C PROV-JSON** ([`build_prov_document`]): a machine-queryable
//!   entities/activities/agents graph — inputs `used`, outputs
//!   `wasGeneratedBy`, derivations `wasDerivedFrom`, and the triggering
//!   agent. Content-addressed entity ids (`urn:autonomics:sha256:…`) make
//!   identical bytes the same entity across runs.
//! - **RO-Crate 1.1** ([`export_ro_crate`]): a self-contained directory —
//!   manifest + the run's result files pulled from the object store
//!   (content-verified against the recorded sha256) + the snapshot manifest.
//!   Input data is referenced by uri + hash, not copied.
//!
//! Both are pure generators over already-recorded audit state: no engine, no
//! registry — just `serde_json` and the file store. Relations are emitted in
//! the widely-consumed array form (`"used": [{"activity": …, "entity": …}]`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use super::history::{DagHistory, RunRecord};

/// RO-Crate 1.1 context (pinned by the spec).
pub const RO_CRATE_CONTEXT: &str = "https://w3id.org/ro/crate/1.1/context";

/// Requested export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// Single W3C PROV-JSON document.
    Prov,
    /// RO-Crate 1.1 directory (manifest + result files).
    Crate,
}

impl ExportFormat {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "prov" | "prov-json" | "provjson" => Ok(Self::Prov),
            "crate" | "ro-crate" | "rocrate" => Ok(Self::Crate),
            other => Err(format!(
                "unknown export format `{other}` (expected `prov` or `crate`)"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prov => "prov",
            Self::Crate => "crate",
        }
    }
}

/// One file inside an export.
#[derive(Debug, Clone, Serialize)]
pub struct ExportFile {
    /// Original recorded path / uri (usually `vfs://…`).
    pub source: String,
    /// Path inside the export (crate-relative, or the written file for PROV).
    pub exported_path: String,
    pub bytes: u64,
    /// Whether the pulled bytes were verified against a recorded
    /// `sha256:` hash. `false` means no recomputable hash was recorded —
    /// the bytes are copied as-is.
    pub sha256_verified: bool,
}

/// A file that could not be exported, and why.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

/// Outcome of one export.
#[derive(Debug, Clone, Serialize)]
pub struct ExportSummary {
    pub run_id: String,
    pub format: &'static str,
    /// Export destination (directory for crate, file for prov).
    pub out: String,
    pub files: Vec<ExportFile>,
    pub skipped: Vec<SkippedFile>,
}

// ── run resolution ────────────────────────────────────────────────────────────

/// Resolve `id_or_prefix` to a run: exact id first, then a unique prefix
/// match. Errors name the recent runs so the caller can retry.
pub async fn resolve_run(
    history: &DagHistory,
    id_or_prefix: &str,
) -> Result<RunRecord, String> {
    if let Some(run) = history
        .get_run(id_or_prefix)
        .await
        .map_err(|e| format!("run lookup failed: {e}"))?
    {
        return Ok(run);
    }
    let all = history
        .list_runs(1000, None)
        .await
        .map_err(|e| format!("run listing failed: {e}"))?;
    let matches: Vec<&RunRecord> =
        all.iter().filter(|run| run.id.starts_with(id_or_prefix)).collect();
    match matches.as_slice() {
        [run] => Ok((*run).clone()),
        [] => {
            let recent: Vec<&str> =
                all.iter().take(5).map(|run| run.id.as_str()).collect();
            Err(format!(
                "no run matches `{id_or_prefix}`. Recent runs: {}",
                if recent.is_empty() {
                    "(none recorded)".to_string()
                } else {
                    recent.join(", ")
                }
            ))
        }
        many => Err(format!(
            "run prefix `{id_or_prefix}` is ambiguous ({} matches: {})",
            many.len(),
            many.iter().map(|run| run.id.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

// ── shared schemaless helpers over run_report_json ───────────────────────────

/// A file-like value pulled from the report: {path, format, fingerprint}.
#[derive(Debug, Clone)]
struct FileEntry {
    path: String,
    format: Option<String>,
    content_hash: Option<String>,
    size: Option<u64>,
}

impl FileEntry {
    fn from_json(value: &Value) -> Option<Self> {
        let path = value.get("path")?.as_str()?.to_string();
        Some(Self {
            path,
            format: value.get("format").and_then(Value::as_str).map(str::to_string),
            content_hash: value
                .get("fingerprint")
                .and_then(|fp| fp.get("content_hash"))
                .and_then(Value::as_str)
                .map(str::to_string),
            size: value
                .get("fingerprint")
                .and_then(|fp| fp.get("size"))
                .and_then(Value::as_u64),
        })
    }

    /// Bare lowercase hex when the recorded hash is in the recomputable
    /// `sha256:` dialect.
    fn sha256_hex(&self) -> Option<&str> {
        self.content_hash
            .as_deref()
            .and_then(|hash| hash.strip_prefix("sha256:"))
    }
}

/// Everything the exporters need from one node's report entry.
struct NodeEntry {
    id: String,
    node_type: String,
    status: String,
    elapsed_ms: Option<u64>,
    fingerprint: Option<String>,
    image: Option<String>,
    image_digest: Option<String>,
    exit_code: Option<i64>,
    inputs: Vec<(String, Option<FileEntry>)>, // (from node id, file-like value)
    outputs: Vec<FileEntry>,                  // output_files + port_assignments
    logs: Vec<FileEntry>,                     // stdout/stderr captures
}

fn parse_nodes(report: &Value) -> Vec<NodeEntry> {
    let empty = Vec::new();
    report
        .get("nodes")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .map(|node| {
            let execution = node.get("execution");
            let mut outputs = Vec::new();
            for value in node
                .get("output_files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(entry) = FileEntry::from_json(value) {
                    outputs.push(entry);
                }
            }
            for value in node
                .get("port_assignments")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|map| map.values())
            {
                if let Some(entry) = FileEntry::from_json(value) {
                    outputs.push(entry);
                }
            }
            let mut logs = Vec::new();
            for key in ["stdout_log", "stderr_log"] {
                if let Some(entry) = execution
                    .and_then(|exec| exec.get(key))
                    .and_then(FileEntry::from_json)
                {
                    logs.push(entry);
                }
            }
            NodeEntry {
                id: node.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                node_type: node
                    .get("node_type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                status: node
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                elapsed_ms: node.get("elapsed_ms").and_then(Value::as_u64),
                fingerprint: node
                    .get("fingerprint")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                image: execution
                    .and_then(|exec| exec.get("image"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                image_digest: execution
                    .and_then(|exec| exec.get("image_digest"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                exit_code: execution
                    .and_then(|exec| exec.get("exit_code"))
                    .and_then(Value::as_i64),
                inputs: node
                    .get("inputs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|binding| {
                        let from = binding
                            .get("from")
                            .and_then(Value::as_str)?
                            .to_string();
                        // A binding carries the first file of file-like
                        // values; DataFrame bindings have no path → None.
                        let entry = FileEntry::from_json(
                            &json!({
                                "path": binding.get("path"),
                                "fingerprint": binding.get("fingerprint"),
                            }),
                        );
                        Some((from, entry))
                    })
                    .collect(),
                outputs,
                logs,
            }
        })
        .collect()
}

/// PROV entity id for a file-like value: content-addressed when a sha256 is
/// recorded, path-addressed otherwise.
fn entity_id_for_file(entry: &FileEntry) -> String {
    match entry.sha256_hex() {
        Some(hex) => format!("urn:autonomics:sha256:{hex}"),
        None => format!("urn:autonomics:path:{}", entry.path),
    }
}

// ── W3C PROV-JSON ─────────────────────────────────────────────────────────────

/// Build a PROV-JSON document for one recorded run.
///
/// `manifest` is the raw snapshot manifest JSON (the executed definition),
/// embedded as a referenced entity.
pub fn build_prov_document(
    run: &RunRecord,
    report: &Value,
    manifest: Option<&str>,
) -> Value {
    let nodes = parse_nodes(report);
    let activity = |node: &NodeEntry| format!("urn:autonomics:run:{}:node:{}", run.id, node.id);

    let mut entities: BTreeMap<String, Value> = BTreeMap::new();
    let mut activities: BTreeMap<String, Value> = BTreeMap::new();
    let mut used: Vec<Value> = Vec::new();
    let mut generated: Vec<Value> = Vec::new();
    let mut derived: Vec<Value> = Vec::new();

    // Input / output entities (content-addressed ids dedupe identical bytes).
    for node in &nodes {
        for (_, entry) in &node.inputs {
            if let Some(entry) = entry {
                entities.entry(entity_id_for_file(entry)).or_insert_with(|| {
                    json!({ "path": entry.path, "format": entry.format,
                            "sha256": entry.content_hash })
                });
            }
        }
        for entry in node.outputs.iter().chain(node.logs.iter()) {
            entities.entry(entity_id_for_file(entry)).or_insert_with(|| {
                json!({ "path": entry.path, "format": entry.format,
                        "sha256": entry.content_hash })
            });
        }
        // DataFrame outputs have no address: represent the value by the
        // node's execution fingerprint.
        if node.outputs.is_empty() && node.fingerprint.is_some() {
            let id = format!("urn:autonomics:run:{}:df:{}", run.id, node.id);
            entities.insert(
                id.clone(),
                json!({ "kind": "DataFrame", "fingerprint": node.fingerprint }),
            );
            generated.push(json!({ "entity": id, "activity": activity(node) }));
        }
    }

    // Activities.
    for node in &nodes {
        let mut attrs = json!({
            "type": "autonomics:node",
            "node_type": node.node_type,
            "status": node.status,
        });
        if let Some(fp) = &node.fingerprint {
            attrs["fingerprint"] = json!(fp);
        }
        if let Some(ms) = node.elapsed_ms {
            attrs["elapsed_ms"] = json!(ms);
        }
        if let Some(image) = &node.image {
            attrs["image"] = json!(image);
        }
        if let Some(digest) = &node.image_digest {
            attrs["image_digest"] = json!(digest);
        }
        if let Some(code) = node.exit_code {
            attrs["exit_code"] = json!(code);
        }
        activities.insert(activity(node), attrs);
    }

    // Relations.
    for node in &nodes {
        let act = activity(node);
        let mut input_ids = Vec::new();
        for (_, entry) in &node.inputs {
            if let Some(entry) = entry {
                let id = entity_id_for_file(entry);
                used.push(json!({ "activity": act, "entity": id }));
                input_ids.push(id);
            }
        }
        let mut output_ids = Vec::new();
        for entry in node.outputs.iter().chain(node.logs.iter()) {
            let id = entity_id_for_file(entry);
            generated.push(json!({ "entity": id, "activity": act }));
            output_ids.push(id);
        }
        for input_id in input_ids {
            for output_id in &output_ids {
                derived.push(json!({
                    "generatedEntity": output_id,
                    "usedEntity": input_id,
                }));
            }
        }
    }

    // Agents: the triggering agent (if attributed) and the engine build.
    let mut agents: BTreeMap<String, Value> = BTreeMap::new();
    let trigger_agent = run.trigger.as_deref().and_then(|trigger| {
        let path = trigger.strip_prefix("agent:")?;
        let id = format!("urn:autonomics:agent:{path}");
        agents.insert(id.clone(), json!({ "kind": "trigger", "name": path }));
        Some(id)
    });
    let engine_id = format!("urn:autonomics:engine:{}", run.engine_version);
    agents.insert(
        engine_id.clone(),
        json!({
            "kind": "software",
            "engine_version": run.engine_version,
            "source_revision": run.source_revision,
        }),
    );

    // The executed definition (snapshot manifest) is an entity too.
    if let Some(snapshot_id) = &run.snapshot_id {
        entities.insert(
            format!("urn:autonomics:snapshot:{snapshot_id}"),
            json!({
                "kind": "dag-manifest",
                "manifest_hash": run.manifest_hash,
                "manifest_present": manifest.is_some(),
            }),
        );
    }

    let mut associated = Vec::new();
    for node_id in activities.keys().cloned().collect::<Vec<_>>() {
        if let Some(agent) = &trigger_agent {
            associated.push(json!({ "activity": node_id, "agent": agent }));
        }
        associated.push(json!({ "activity": node_id, "agent": engine_id }));
    }

    json!({
        "prefix": { "autonomics": "urn:autonomics:" },
        "entity": entities,
        "activity": activities,
        "agent": agents,
        "used": used,
        "wasGeneratedBy": generated,
        "wasDerivedFrom": derived,
        "wasAssociatedWith": associated,
    })
}

// ── RO-Crate ──────────────────────────────────────────────────────────────────

/// Export the run as an RO-Crate 1.1 directory.
///
/// Pulls the run's result files (outputs + captured logs) from the object
/// store — verifying each against its recorded `sha256:` hash; a mismatch is
/// an error, not a warning — and writes the crate manifest plus the snapshot
/// manifest. Input data is referenced by uri + hash, never copied.
pub async fn export_ro_crate(
    run: &RunRecord,
    report: &Value,
    manifest: Option<&str>,
    out_dir: &Path,
    storage: Option<&vfs::OpendalFileStorage>,
) -> Result<ExportSummary, String> {
    let nodes = parse_nodes(report);

    // Dedupe pull list by source path (outputs + port assignments + logs).
    let mut pull: BTreeMap<String, FileEntry> = BTreeMap::new();
    for node in &nodes {
        for entry in node.outputs.iter().chain(node.logs.iter()) {
            pull.insert(entry.path.clone(), entry.clone());
        }
    }

    std::fs::create_dir_all(out_dir.join("data"))
        .map_err(|e| format!("create crate dir: {e}"))?;
    std::fs::create_dir_all(out_dir.join("workflow"))
        .map_err(|e| format!("create workflow dir: {e}"))?;

    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut graph: Vec<Value> = Vec::new();

    for entry in pull.values() {
        let exported = match pull_file(entry, storage, out_dir).await {
            Ok(exported) => exported,
            Err(PullFailure::Skip(reason)) => {
                skipped.push(SkippedFile {
                    path: entry.path.clone(),
                    reason,
                });
                continue;
            }
            Err(PullFailure::Fatal(reason)) => return Err(reason),
        };
        let crate_path = exported.exported_path.clone();
        files.push(exported);
        graph.push(file_entity(&entry, &crate_path));
    }

    // Snapshot manifest: the executed definition ships in the crate.
    if let Some(manifest_json) = manifest {
        let path = "workflow/manifest.json";
        std::fs::write(out_dir.join(path), manifest_json)
            .map_err(|e| format!("write manifest: {e}"))?;
        files.push(ExportFile {
            source: format!("snapshot:{}", run.snapshot_id.clone().unwrap_or_default()),
            exported_path: path.to_string(),
            bytes: manifest_json.len() as u64,
            sha256_verified: false,
        });
        graph.push(json!({
            "@id": path, "@type": "File",
            "name": "dag-manifest.json",
            "description": "DAG definition (snapshot manifest) executed by this run",
            "snapshotId": run.snapshot_id,
            "manifestHash": run.manifest_hash,
        }));
    }

    // Actions: one CreateAction per executed node.
    let agent_id = trigger_agent_id(run);
    for node in &nodes {
        let mut action = json!({
            "@id": format!("#action-{}", node.id),
            "@type": "CreateAction",
            "name": node.node_type,
            "actionStatus": node.status,
        });
        if let Some(fp) = &node.fingerprint {
            action["fingerprint"] = json!(fp);
        }
        if let (Some(image), Some(digest)) = (&node.image, &node.image_digest) {
            action["instrument"] = json!({
                "@id": image, "@type": "SoftwareApplication", "digest": digest,
            });
        }
        if let Some(code) = node.exit_code {
            action["exitCode"] = json!(code);
        }
        if let Some(agent) = &agent_id {
            action["agent"] = json!({ "@id": agent });
        }
        // Inputs are referenced where they live (uri + hash), not copied.
        let objects: Vec<Value> = node
            .inputs
            .iter()
            .filter_map(|(_, entry)| entry.as_ref())
            .map(|entry| referenced_input_entity(entry))
            .collect();
        if !objects.is_empty() {
            action["object"] = json!(objects);
        }
        let results: Vec<Value> = node
            .outputs
            .iter()
            .chain(node.logs.iter())
            .filter_map(|entry| {
                files
                    .iter()
                    .find(|file| file.source == entry.path)
                    .map(|file| json!({ "@id": file.exported_path }))
            })
            .collect();
        if !results.is_empty() {
            action["result"] = json!(results);
        }
        graph.push(action);
    }

    // Trigger agent + engine entities.
    if let Some(agent) = &agent_id {
        let name = run
            .trigger
            .as_deref()
            .and_then(|trigger| trigger.strip_prefix("agent:"))
            .unwrap_or_default();
        graph.push(json!({
            "@id": agent, "@type": "SoftwareAgent", "name": name,
        }));
    }
    graph.push(json!({
        "@id": "urn:autonomics:engine", "@type": "SoftwareApplication",
        "name": "autonomics", "softwareVersion": run.engine_version,
        "sourceRevision": run.source_revision,
    }));

    // Root + descriptor entities.
    graph.push(json!({
        "@id": "./", "@type": "Dataset",
        "name": format!("autonomics run {}", run.id),
        "description": run.message.as_deref().unwrap_or("DAG execution evidence"),
        "datePublished": run.finished_at,
        "runId": run.id,
        "snapshotId": run.snapshot_id,
        "ok": run.ok,
        "hasPart": files.iter().map(|f| json!({"@id": f.exported_path})).collect::<Vec<_>>(),
    }));
    graph.push(json!({
        "@id": "ro-crate-metadata.json", "@type": "CreativeWork",
        "about": {"@id": "./"}, "conformsTo": {"@id": RO_CRATE_CONTEXT},
    }));

    let document = json!({
        "@context": RO_CRATE_CONTEXT,
        "@graph": graph,
    });
    std::fs::write(
        out_dir.join("ro-crate-metadata.json"),
        serde_json::to_string_pretty(&document)
            .map_err(|e| format!("serialize crate manifest: {e}"))?,
    )
    .map_err(|e| format!("write crate manifest: {e}"))?;

    std::fs::write(
        out_dir.join("ro-crate-preview.html"),
        preview_html(run, &nodes),
    )
    .map_err(|e| format!("write preview: {e}"))?;

    Ok(ExportSummary {
        run_id: run.id.clone(),
        format: "crate",
        out: out_dir.to_string_lossy().into_owned(),
        files,
        skipped,
    })
}

fn trigger_agent_id(run: &RunRecord) -> Option<String> {
    run.trigger
        .as_deref()
        .and_then(|trigger| trigger.strip_prefix("agent:"))
        .map(|path| format!("urn:autonomics:agent:{path}"))
}

/// Crate File entity for a packaged file (relative crate path + original uri).
fn file_entity(entry: &FileEntry, crate_path: &str) -> Value {
    let mut entity = json!({
        "@id": crate_path, "@type": "File",
        "name": Path::new(&entry.path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.path.clone()),
        "sourcePath": entry.path,
    });
    if let Some(hex) = entry.sha256_hex() {
        entity["sha256"] = json!(hex);
    } else if let Some(hash) = &entry.content_hash {
        entity["checksum"] = json!(hash);
    }
    if let Some(format) = &entry.format {
        entity["encodingFormat"] = json!(format);
    }
    if let Some(size) = entry.size {
        entity["contentSize"] = json!(size);
    }
    entity
}

/// Crate File entity for an input: referenced in place (uri + hash), not copied.
fn referenced_input_entity(entry: &FileEntry) -> Value {
    let mut entity = json!({
        "@id": entry.path, "@type": "File",
        "description": "referenced input (not packaged)",
    });
    if let Some(hex) = entry.sha256_hex() {
        entity["sha256"] = json!(hex);
    }
    entity
}

/// Why a file could not be pulled into the crate.
enum PullFailure {
    /// The pulled bytes do not match the recorded hash — the export must
    /// abort: an evidence package that cannot vouch for its contents is
    /// worse than none.
    Fatal(String),
    /// The source file is unavailable (missing / unreachable / no storage).
    /// The crate documents the gap honestly as a skip.
    Skip(String),
}

/// Pull one file into the crate under `data/`, verifying its content hash.
async fn pull_file(
    entry: &FileEntry,
    storage: Option<&vfs::OpendalFileStorage>,
    out_dir: &Path,
) -> Result<ExportFile, PullFailure> {
    use sha2::Digest;

    let fatal = |message: String| PullFailure::Fatal(message);
    let skip = |message: String| PullFailure::Skip(message);

    // Resolve: vfs:// → store; mounted bare path → store; else host file.
    let (virtual_path, target_rel) = if let Some(vpath) = entry.path.strip_prefix("vfs://") {
        let normalized = vfs::OpendalFileStorage::normalize_path(vpath);
        (
            Some(normalized.clone()),
            format!("data/{}", normalized.trim_start_matches('/')),
        )
    } else if entry.path.starts_with('/') {
        let normalized = vfs::OpendalFileStorage::normalize_path(&entry.path);
        let mounted = storage.is_some_and(|storage| storage.is_mounted(&normalized));
        if mounted {
            (
                Some(normalized.clone()),
                format!("data/{}", normalized.trim_start_matches('/')),
            )
        } else {
            // Host-absolute path: copy from the filesystem, mirrored under
            // data/local/…
            let stripped = normalized.trim_start_matches('/');
            (None, format!("data/local/{stripped}"))
        }
    } else {
        return Err(skip(format!(
            "path `{}` is neither a vfs uri nor an absolute path",
            entry.path
        )));
    };

    let target = out_dir.join(&target_rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| fatal(format!("create {}: {e}", target_rel)))?;
    }

    let mut hasher = sha2::Sha256::new();
    match (virtual_path, storage) {
        (Some(vpath), Some(storage)) => {
            let length = match storage.content_length(&vpath).await {
                Ok(length) => length,
                // Source unavailable (e.g. logs pruned by GC): skip honestly.
                Err(e) => return Err(skip(format!("stat `{}`: {e}", entry.path))),
            };
            const CHUNK: u64 = 1024 * 1024;
            let mut offset = 0u64;
            let mut file = tokio::fs::File::create(&target)
                .await
                .map_err(|e| fatal(format!("create {target_rel}: {e}")))?;
            use tokio::io::AsyncWriteExt;
            while offset < length {
                let end = (offset + CHUNK).min(length);
                let buffer = storage
                    .read_range(&vpath, offset..end)
                    .await
                    .map_err(|e| fatal(format!("read `{}` at {offset}: {e}", entry.path)))?;
                let bytes = buffer.to_bytes();
                hasher.update(&bytes);
                file.write_all(&bytes)
                    .await
                    .map_err(|e| fatal(format!("write {target_rel}: {e}")))?;
                offset += bytes.len() as u64;
            }
            file.flush()
                .await
                .map_err(|e| fatal(format!("flush {target_rel}: {e}")))?;
        }
        (Some(_), None) => {
            return Err(skip(format!(
                "path `{}` lives on the object store but no storage was configured",
                entry.path
            )));
        }
        (None, _) => {
            // Host file: stream copy + hash in one pass.
            use tokio::io::AsyncReadExt;
            let mut source = match tokio::fs::File::open(&entry.path).await {
                Ok(source) => source,
                Err(e) => return Err(skip(format!("open host file `{}`: {e}", entry.path))),
            };
            let mut file = tokio::fs::File::create(&target)
                .await
                .map_err(|e| fatal(format!("create {target_rel}: {e}")))?;
            use tokio::io::AsyncWriteExt;
            let mut buffer = vec![0u8; 1024 * 1024];
            loop {
                let read = source
                    .read(&mut buffer)
                    .await
                    .map_err(|e| fatal(format!("read `{}`: {e}", entry.path)))?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                file.write_all(&buffer[..read])
                    .await
                    .map_err(|e| fatal(format!("write {target_rel}: {e}")))?;
            }
            file.flush()
                .await
                .map_err(|e| fatal(format!("flush {target_rel}: {e}")))?;
        }
    }

    let computed = hex_lower(&hasher.finalize());
    let verified = match entry.sha256_hex() {
        Some(expected) => {
            if !computed.eq_ignore_ascii_case(expected) {
                return Err(PullFailure::Fatal(format!(
                    "content hash mismatch for `{}`: recorded sha256:{expected}, pulled bytes hash to {computed} — the evidence package would be untrustworthy",
                    entry.path
                )));
            }
            true
        }
        None => false,
    };
    let bytes = std::fs::metadata(&target)
        .map_err(|e| fatal(format!("stat {target_rel}: {e}")))?
        .len();

    Ok(ExportFile {
        source: entry.path.clone(),
        exported_path: target_rel,
        bytes,
        sha256_verified: verified,
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Minimal human-readable preview.
fn preview_html(run: &RunRecord, nodes: &[NodeEntry]) -> String {
    let rows = nodes
        .iter()
        .map(|node| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                node.id,
                node.node_type,
                node.status,
                node.fingerprint.as_deref().unwrap_or("—"),
                node.outputs
                    .iter()
                    .map(|entry| entry.path.as_str())
                    .collect::<Vec<_>>()
                    .join("<br/>"),
            )
        })
        .collect::<String>();
    format!(
        "<!doctype html><html><head><meta charset='utf-8'><title>autonomics run {}</title>\
         <style>body{{font-family:sans-serif;margin:2rem}}td,th{{border:1px solid #ccc;padding:.3rem .6rem;text-align:left}}</style></head>\
         <body><h1>autonomics run {}</h1>\
         <p>ok: {} · snapshot: {} · engine {} ({})</p>\
         <table><tr><th>node</th><th>type</th><th>status</th><th>fingerprint</th><th>outputs</th></tr>{rows}</table>\
         <p>See <code>ro-crate-metadata.json</code> for the machine-readable graph.</p></body></html>",
        run.id,
        run.id,
        run.ok,
        run.snapshot_id.as_deref().unwrap_or("—"),
        run.engine_version,
        run.source_revision,
    )
}

/// Convenience for callers that only need the PROV document written to disk:
/// writes `{out_dir}/prov-{run_id}.json`.
pub fn write_prov_document(
    run: &RunRecord,
    report: &Value,
    manifest: Option<&str>,
    out_dir: &Path,
) -> Result<ExportSummary, String> {
    let document = build_prov_document(run, report, manifest);
    std::fs::create_dir_all(out_dir).map_err(|e| format!("create out dir: {e}"))?;
    let file_name = format!("prov-{}.json", run.id);
    let path = out_dir.join(&file_name);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&document)
            .map_err(|e| format!("serialize prov document: {e}"))?,
    )
    .map_err(|e| format!("write prov document: {e}"))?;
    Ok(ExportSummary {
        run_id: run.id.clone(),
        format: "prov",
        out: path.to_string_lossy().into_owned(),
        files: vec![ExportFile {
            source: format!("run:{}", run.id),
            exported_path: file_name,
            bytes: std::fs::metadata(&path)
                .map(|meta| meta.len())
                .unwrap_or_default(),
            sha256_verified: false,
        }],
        skipped: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_run() -> RunRecord {
        RunRecord {
            id: "0123456789abcdef".to_string(),
            ref_name: "main".to_string(),
            snapshot_id: Some("snap123".to_string()),
            manifest_hash: "hashabc".to_string(),
            trigger: Some("agent:/root/researcher".to_string()),
            started_at: "2026-09-29T10:00:00Z".to_string(),
            finished_at: "2026-09-29T10:01:00Z".to_string(),
            ok: true,
            cancelled: false,
            error: None,
            message: Some("audit e2e".to_string()),
            engine_version: "0.1.0".to_string(),
            source_revision: "abc1234".to_string(),
            run_report_json: Some(sample_report_json().to_string()),
        }
    }

    fn sample_report_json() -> Value {
        json!({
            "ok": true,
            "nodes": [
                {
                    "id": "src", "status": "success", "node_type": "file_to_dataframe",
                    "elapsed_ms": 12, "fingerprint": "fingerprint-src",
                    "output_type": "DataFrame",
                },
                {
                    "id": "tool", "status": "success", "node_type": "container",
                    "elapsed_ms": 3400, "fingerprint": "fingerprint-tool",
                    "output_files": [{
                        "path": "vfs:///artifacts/run-1/out.csv", "format": "csv",
                        "fingerprint": {
                            "size": 10, "mtime_ns": 0,
                            "content_hash": "sha256:deadbeefdeadbeef",
                            "immutable_remote": true,
                        },
                    }],
                    "inputs": [{
                        "from": "src", "from_port": 0, "to_port": 0,
                        "kind": "DataFrame",
                    }],
                    "execution": {
                        "image": "ghcr.io/x/y@sha256:ee95",
                        "image_digest": "sha256:ee95",
                        "exit_code": 0, "run_name": "run-1",
                        "stdout_log": {
                            "path": "vfs:///artifacts/run-1/.autonomics-logs/stdout.log",
                            "format": "text/plain",
                            "fingerprint": {
                                "size": 6, "mtime_ns": 0,
                                "content_hash": "sha256:0123456789abcdef",
                                "immutable_remote": true,
                            },
                        },
                    },
                },
            ],
        })
    }

    #[test]
    fn prov_document_carries_the_core_triplets() {
        let run = sample_run();
        let doc = build_prov_document(&run, &sample_report_json(), Some("{}"));

        let tool_activity = "urn:autonomics:run:0123456789abcdef:node:tool";
        let out_entity = "urn:autonomics:sha256:deadbeefdeadbeef";
        assert!(doc["activity"][tool_activity]["image_digest"]
            .as_str()
            .is_some(), "container node carries its image digest");
        // Content-addressed output entity + generation link.
        assert!(doc["entity"][out_entity].is_object());
        assert!(doc["wasGeneratedBy"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["entity"] == out_entity && link["activity"] == tool_activity));
        // Trigger agent association.
        assert!(doc["agent"]["urn:autonomics:agent:/root/researcher"].is_object());
        assert!(doc["wasAssociatedWith"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["activity"] == tool_activity
                && link["agent"] == "urn:autonomics:agent:/root/researcher"));
        // The engine agent carries the source revision.
        assert_eq!(
            doc["agent"]["urn:autonomics:engine:0.1.0"]["source_revision"],
            json!("abc1234")
        );
        // DataFrame-only source is represented by its execution fingerprint.
        assert_eq!(
            doc["entity"]["urn:autonomics:run:0123456789abcdef:df:src"]["fingerprint"],
            json!("fingerprint-src")
        );
    }

    #[test]
    fn format_parsing() {
        assert_eq!(ExportFormat::parse("crate").unwrap(), ExportFormat::Crate);
        assert_eq!(ExportFormat::parse("PROV").unwrap(), ExportFormat::Prov);
        assert!(ExportFormat::parse("pdf").is_err());
    }

    #[tokio::test]
    async fn crate_export_pulls_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let storage = vfs::OpendalFileStorage::new(dir.path());
        let contents = b"csv,bytes\n1,2\n";
        storage.write_bytes("/artifacts/run-1/out.csv", contents.to_vec()).await.unwrap();

        // Correct recorded hash.
        let digest = format!("sha256:{}", hex_lower(&sha2_computed(contents)));
        let mut report = sample_report_json();
        report["nodes"][1]["output_files"][0]["fingerprint"]["content_hash"] =
            json!(digest);

        let run = sample_run();
        let out = tempfile::tempdir().unwrap();
        let summary = export_ro_crate(
            &run,
            &report,
            Some(r#"{"nodes":[],"edges":[]}"#),
            out.path(),
            Some(&storage),
        )
        .await
        .unwrap();

        assert!(summary.files.iter().any(|file| {
            file.exported_path == "data/artifacts/run-1/out.csv" && file.sha256_verified
        }));
        let packaged = std::fs::read(out.path().join("data/artifacts/run-1/out.csv")).unwrap();
        assert_eq!(packaged, contents);
        // The log object does not exist in the store → skipped with a reason.
        assert!(
            summary
                .skipped
                .iter()
                .any(|skip| skip.path.contains(".autonomics-logs/stdout.log"))
        );
        // Manifest + crate metadata are in place.
        assert!(out.path().join("workflow/manifest.json").exists());
        let crate_doc: Value = serde_json::from_str(
            &std::fs::read_to_string(out.path().join("ro-crate-metadata.json")).unwrap(),
        )
        .unwrap();
        let actions: Vec<&Value> = crate_doc["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["@type"] == json!("CreateAction"))
            .collect();
        assert_eq!(actions.len(), 2, "one action per reported node");
    }

    #[tokio::test]
    async fn crate_export_rejects_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let storage = vfs::OpendalFileStorage::new(dir.path());
        storage
            .write_bytes("/artifacts/run-1/out.csv", b"actual bytes".to_vec())
            .await
            .unwrap();

        let mut report = sample_report_json();
        // Recorded hash does NOT match the stored bytes.
        report["nodes"][1]["output_files"][0]["fingerprint"]["content_hash"] =
            json!("sha256:0000000000000000");

        let run = sample_run();
        let out = tempfile::tempdir().unwrap();
        let error = export_ro_crate(&run, &report, None, out.path(), Some(&storage))
            .await
            .unwrap_err();
        assert!(error.contains("mismatch"), "{error}");
    }

    #[tokio::test]
    async fn crate_export_copies_local_host_outputs() {
        let source_dir = tempfile::tempdir().unwrap();
        let csv = source_dir.path().join("local-out.csv");
        std::fs::write(&csv, b"local,data\n1,2\n").unwrap();

        let mut report = sample_report_json();
        report["nodes"][1]["output_files"][0] = json!({
            "path": csv.to_string_lossy(), "format": "csv",
            "fingerprint": { "size": 14, "mtime_ns": 1, "content_hash": null },
        });

        let run = sample_run();
        let out = tempfile::tempdir().unwrap();
        // No storage configured: the local path must still be copyable.
        let summary =
            export_ro_crate(&run, &report, None, out.path(), None).await.unwrap();
        let entry = summary
            .files
            .iter()
            .find(|file| file.exported_path.ends_with("local-out.csv"))
            .expect("local output copied");
        assert!(!entry.sha256_verified, "no recorded hash → unverified, not an error");
        let copied =
            std::fs::read(out.path().join(&entry.exported_path)).unwrap();
        assert_eq!(copied, b"local,data\n1,2\n");
    }

    #[tokio::test]
    async fn resolve_run_accepts_unique_prefix() {
        let history = DagHistory::open_in_memory().await.unwrap();
        let mut run = sample_run();
        run.id = "aaaa1111".to_string();
        history.record_run(&run).await.unwrap();
        let mut other = sample_run();
        other.id = "bbbb2222".to_string();
        history.record_run(&other).await.unwrap();

        assert_eq!(resolve_run(&history, "aaaa").await.unwrap().id, "aaaa1111");
        assert_eq!(resolve_run(&history, "bbbb2222").await.unwrap().id, "bbbb2222");
        assert!(resolve_run(&history, "zzzz").await.unwrap_err().contains("no run matches"));
    }

    fn sha2_computed(bytes: &[u8]) -> Vec<u8> {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).to_vec()
    }
}
