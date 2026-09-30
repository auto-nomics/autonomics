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
use sha2::Digest;

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
    /// After a VFS upload this becomes the `vfs://` address instead.
    pub exported_path: String,
    pub bytes: u64,
    /// sha256 of the exported bytes: the recorded hash for verified pulls,
    /// or the digest of the produced file itself (PROV document, crate
    /// manifest) so every export is self-certifying.
    pub sha256: Option<String>,
    /// Whether the pulled bytes were verified against a recorded
    /// `sha256:` hash. `false` for produced files (nothing to verify
    /// against) and pulls without a recomputable recorded hash.
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

/// Resolve `id_or_prefix` to a run: the literal `latest` selects the most
/// recent run; otherwise exact id first, then a unique prefix match. Errors
/// name the recent runs so the caller can retry.
pub async fn resolve_run(history: &DagHistory, id_or_prefix: &str) -> Result<RunRecord, String> {
    let id_or_prefix = id_or_prefix.trim();
    if id_or_prefix.eq_ignore_ascii_case("latest") {
        let mut recent = history
            .list_runs(1, None)
            .await
            .map_err(|e| format!("run listing failed: {e}"))?;
        return recent
            .pop()
            .ok_or_else(|| "no runs recorded yet".to_string());
    }
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
    let matches: Vec<&RunRecord> = all
        .iter()
        .filter(|run| run.id.starts_with(id_or_prefix))
        .collect();
    match matches.as_slice() {
        [run] => Ok((*run).clone()),
        [] => {
            let recent: Vec<&str> = all.iter().take(5).map(|run| run.id.as_str()).collect();
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
            many.iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
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
            format: value
                .get("format")
                .and_then(Value::as_str)
                .map(str::to_string),
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

/// One upstream value reference from a node's input bindings.
#[derive(Debug, Clone)]
enum NodeInputRef {
    /// A file-like value (carries path + fingerprint; FileSet bindings
    /// record only the first file — an existing audit degradation).
    File(FileEntry),
    /// A DataFrame value: no address, so the reference is the *producing
    /// node* — resolved to its df entity downstream. Without this the
    /// lineage chain breaks at every DataFrame edge.
    Dataframe { from: String },
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
    inputs: Vec<NodeInputRef>,
    /// File outputs, deduplicated by path — `output_files` and
    /// `port_assignments` carry the same artifacts under two spellings.
    outputs: Vec<FileEntry>,
    logs: Vec<FileEntry>, // stdout/stderr captures
    /// Output column schema (`NodeReport.output_schema`), when reported.
    output_schema: Option<Value>,
    output_rows: Option<u64>,
}

impl NodeEntry {
    /// Source nodes (no inputs) cannot generate file bytes — their file
    /// outputs are external references ingested into the run.
    fn is_source(&self) -> bool {
        self.inputs.is_empty()
    }
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
            // `output_files` and `port_assignments` are two spellings of the
            // same artifacts — collect by path so relations are emitted once.
            let mut outputs: BTreeMap<String, FileEntry> = BTreeMap::new();
            for value in node
                .get("output_files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .chain(
                    node.get("port_assignments")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|map| map.values()),
                )
            {
                if let Some(entry) = FileEntry::from_json(value) {
                    outputs.insert(entry.path.clone(), entry);
                }
            }
            let outputs = outputs.into_values().collect();
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
                id: node
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
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
                output_schema: node.get("output_schema").cloned(),
                output_rows: node.get("output_rows").and_then(Value::as_u64),
                inputs: node
                    .get("inputs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|binding| {
                        let from = binding.get("from").and_then(Value::as_str)?.to_string();
                        let file = FileEntry::from_json(&json!({
                            "path": binding.get("path"),
                            "fingerprint": binding.get("fingerprint"),
                        }));
                        if let Some(entry) = file {
                            return Some(NodeInputRef::File(entry));
                        }
                        // DataFrame bindings carry no path: reference the
                        // producing node so lineage stays connected.
                        (binding.get("kind").and_then(Value::as_str) == Some("DataFrame"))
                            .then_some(NodeInputRef::Dataframe { from })
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

/// Attributes of a file entity. PROV-JSON attribute names must be qualified
/// names and attribute values are never null — absent fields are omitted.
fn file_entity_attrs(entry: &FileEntry) -> Value {
    let mut attrs = json!({ "autonomics:path": entry.path });
    if let Some(format) = &entry.format {
        attrs["autonomics:format"] = json!(format);
    }
    if let Some(hash) = &entry.content_hash {
        attrs["autonomics:sha256"] = json!(hash);
    }
    attrs
}

/// PROV entity id for the DataFrame value produced by `from` within `run`.
fn df_entity_id(run_id: &str, from: &str) -> String {
    format!("urn:autonomics:run:{run_id}:df:{from}")
}

// ── W3C PROV-JSON ─────────────────────────────────────────────────────────────

/// Build a PROV-JSON document for one recorded run.
///
/// `manifest` is the raw snapshot manifest JSON (the executed definition),
/// embedded as a referenced entity.
///
/// Modeling notes:
/// - Attribute names are PROV-JSON qualified names (`prov:*` reserved,
///   `autonomics:*` extensions) and attribute values are never `null` —
///   unrecorded fields are omitted (spec: W3C PROV-JSON Submission).
/// - A **run-level activity** carries the wall-clock window
///   (`prov:startTime`/`prov:endTime`) and run attributes; node activities
///   reference it via an `autonomics:run` attribute (node-level wall-clock
///   timestamps are not recorded — `autonomics:elapsed_ms` is the honest
///   per-node duration).
/// - **Source nodes** (no inputs) cannot have generated file bytes: their
///   file outputs are external inputs *ingested* into the run — emitted as
///   `used` + entities tagged `autonomics:kind = external-input`, never
///   `wasGeneratedBy`. This keeps original data files from appearing as
///   run products.
/// - **DataFrame values** have no address; the entity is the producing
///   node's fingerprint, enriched with the reported schema summary and row
///   count, and participates in `wasDerivedFrom` so multi-hop lineage stays
///   connected through DataFrame edges.
/// - The **snapshot manifest** entity is wired in as an input the run-level
///   activity `used`, so the executed definition joins the graph instead of
///   dangling.
pub fn build_prov_document(run: &RunRecord, report: &Value, manifest: Option<&str>) -> Value {
    let nodes = parse_nodes(report);
    let run_activity = format!("urn:autonomics:run:{}", run.id);
    let activity = |node: &NodeEntry| format!("urn:autonomics:run:{}:node:{}", run.id, node.id);
    let df_entity = |node: &NodeEntry| df_entity_id(&run.id, &node.id);

    let mut entities: BTreeMap<String, Value> = BTreeMap::new();
    let mut activities: BTreeMap<String, Value> = BTreeMap::new();
    // Relation sets: structural dedup so two spellings of the same artifact
    // (output_files / port_assignments) can never double-emit.
    let mut used: BTreeMap<(String, String), ()> = BTreeMap::new();
    let mut generated: BTreeMap<(String, String), ()> = BTreeMap::new();
    let mut derived: BTreeMap<(String, String), ()> = BTreeMap::new();

    // Run-level activity: the one place real wall-clock timestamps exist.
    activities.insert(
        run_activity.clone(),
        json!({
            "prov:type": "autonomics:run",
            "prov:startTime": run.started_at,
            "prov:endTime": run.finished_at,
            "autonomics:ok": run.ok,
            "autonomics:cancelled": run.cancelled,
            "autonomics:ref": run.ref_name,
            "autonomics:message": run.message,
        }),
    );

    // Entities + per-node classification.
    for node in &nodes {
        let external = node.is_source();
        for input in &node.inputs {
            if let NodeInputRef::File(entry) = input {
                entities
                    .entry(entity_id_for_file(entry))
                    .or_insert_with(|| file_entity_attrs(entry));
            }
            // DataFrame inputs are declared by their producing node below.
        }
        for entry in node.outputs.iter().chain(node.logs.iter()) {
            let attrs = entities
                .entry(entity_id_for_file(entry))
                .or_insert_with(|| file_entity_attrs(entry));
            if external {
                attrs["autonomics:kind"] = json!("external-input");
            }
        }
        // DataFrame outputs have no address: represent the value by the
        // node's execution fingerprint, with the reported shape attached.
        if node.outputs.is_empty() && node.fingerprint.is_some() {
            let mut attrs = json!({
                "prov:type": "autonomics:DataFrame",
                "autonomics:fingerprint": node.fingerprint,
            });
            if let Some(schema) = &node.output_schema {
                // PROV attribute values are literals: encode the reported
                // schema as compact JSON text instead of a nested object
                // (which could itself contain nulls / unqualified keys).
                if let Ok(text) = serde_json::to_string(schema) {
                    attrs["autonomics:schema"] = json!(text);
                }
            }
            if let Some(rows) = node.output_rows {
                attrs["autonomics:rows"] = json!(rows);
            }
            entities.insert(df_entity(node), attrs);
        }
    }

    // Node activities.
    for node in &nodes {
        let mut attrs = json!({
            "prov:type": "autonomics:node",
            "autonomics:node_type": node.node_type,
            "autonomics:status": node.status,
            "autonomics:run": run_activity,
        });
        if let Some(fp) = &node.fingerprint {
            attrs["autonomics:fingerprint"] = json!(fp);
        }
        if let Some(ms) = node.elapsed_ms {
            attrs["autonomics:elapsed_ms"] = json!(ms);
        }
        if let Some(image) = &node.image {
            attrs["autonomics:image"] = json!(image);
        }
        if let Some(digest) = &node.image_digest {
            attrs["autonomics:image_digest"] = json!(digest);
        }
        if let Some(code) = node.exit_code {
            attrs["autonomics:exit_code"] = json!(code);
        }
        activities.insert(activity(node), attrs);
    }

    // Relations.
    for node in &nodes {
        let act = activity(node);
        let external = node.is_source();
        let mut input_ids = Vec::new();
        for input in &node.inputs {
            let id = match input {
                NodeInputRef::File(entry) => entity_id_for_file(entry),
                NodeInputRef::Dataframe { from } => df_entity_id(&run.id, from),
            };
            used.insert((act.clone(), id.clone()), ());
            input_ids.push(id);
        }
        let mut output_ids = Vec::new();
        for entry in node.outputs.iter().chain(node.logs.iter()) {
            let id = entity_id_for_file(entry);
            if external {
                // A source node ingests external files, it does not
                // generate them.
                used.insert((act.clone(), id.clone()), ());
            } else {
                generated.insert((id.clone(), act.clone()), ());
            }
            output_ids.push(id);
        }
        // DataFrame values join the derivation chain so lineage stays
        // connected across DataFrame edges.
        if node.outputs.is_empty() && node.fingerprint.is_some() {
            let id = df_entity(node);
            generated.insert((id.clone(), act.clone()), ());
            output_ids.push(id);
        }
        for input_id in input_ids {
            for output_id in &output_ids {
                derived.insert((output_id.clone(), input_id.clone()), ());
            }
        }
    }

    // Agents: the triggering agent (if attributed) and the engine build.
    let mut agents: BTreeMap<String, Value> = BTreeMap::new();
    let trigger_agent = run.trigger.as_deref().and_then(|trigger| {
        let path = trigger.strip_prefix("agent:")?;
        let id = format!("urn:autonomics:agent:{path}");
        agents.insert(
            id.clone(),
            json!({
                "prov:type": "prov:SoftwareAgent",
                "autonomics:kind": "trigger",
                "autonomics:name": path,
            }),
        );
        Some(id)
    });
    let engine_id = format!("urn:autonomics:engine:{}", run.engine_version);
    agents.insert(
        engine_id.clone(),
        json!({
            "prov:type": "prov:SoftwareAgent",
            "autonomics:kind": "engine",
            "autonomics:engine_version": run.engine_version,
            "autonomics:source_revision": run.source_revision,
        }),
    );

    // The executed definition (snapshot manifest) is an entity too — an
    // input the run consumed, so it joins the graph via `used`.
    if let Some(snapshot_id) = &run.snapshot_id {
        let snapshot_entity = format!("urn:autonomics:snapshot:{snapshot_id}");
        entities.insert(
            snapshot_entity.clone(),
            json!({
                "prov:type": "autonomics:dag-manifest",
                "autonomics:manifest_hash": run.manifest_hash,
                "autonomics:ref": run.ref_name,
                "autonomics:manifest_present": manifest.is_some(),
            }),
        );
        used.insert((run_activity.clone(), snapshot_entity), ());
    }

    let mut associated = Vec::new();
    for node_id in activities.keys().cloned().collect::<Vec<_>>() {
        if let Some(agent) = &trigger_agent {
            associated.push(json!({ "activity": node_id, "agent": agent }));
        }
        associated.push(json!({ "activity": node_id, "agent": engine_id }));
    }

    let relation_array = |set: &BTreeMap<(String, String), ()>, first: &str, second: &str| {
        set.keys()
            .map(|(a, b)| json!({ first: a, second: b }))
            .collect::<Vec<_>>()
    };

    json!({
        "prefix": { "autonomics": "urn:autonomics:" },
        "entity": entities,
        "activity": activities,
        "agent": agents,
        "used": relation_array(&used, "activity", "entity"),
        "wasGeneratedBy": relation_array(&generated, "entity", "activity"),
        "wasDerivedFrom": relation_array(&derived, "generatedEntity", "usedEntity"),
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

    std::fs::create_dir_all(out_dir.join("data")).map_err(|e| format!("create crate dir: {e}"))?;
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
        graph.push(file_entity(entry, &crate_path));
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
            sha256: Some(format!(
                "sha256:{}",
                hex_lower(&sha2::Sha256::digest(manifest_json.as_bytes()))
            )),
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
        // File inputs are referenced where they live (uri + hash), not
        // copied; DataFrame inputs have no address and are represented by
        // the action chain instead.
        let objects: Vec<Value> = node
            .inputs
            .iter()
            .filter_map(|input| match input {
                NodeInputRef::File(entry) => Some(referenced_input_entity(entry)),
                NodeInputRef::Dataframe { .. } => None,
            })
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
    let manifest_path = "ro-crate-metadata.json";
    std::fs::write(
        out_dir.join(manifest_path),
        serde_json::to_string_pretty(&document)
            .map_err(|e| format!("serialize crate manifest: {e}"))?,
    )
    .map_err(|e| format!("write crate manifest: {e}"))?;

    let preview_path = "ro-crate-preview.html";
    let preview = preview_html(run, &nodes);
    std::fs::write(out_dir.join(preview_path), &preview)
        .map_err(|e| format!("write preview: {e}"))?;

    // The crate's own documents are export artifacts too (with their
    // self-certifying digests) — so a VFS upload ships the complete crate.
    for path in [manifest_path, preview_path] {
        let bytes = std::fs::read(out_dir.join(path)).map_err(|e| format!("read {path}: {e}"))?;
        files.push(ExportFile {
            source: path.to_string(),
            exported_path: path.to_string(),
            bytes: bytes.len() as u64,
            sha256: Some(format!(
                "sha256:{}",
                hex_lower(&sha2::Sha256::digest(&bytes))
            )),
            sha256_verified: false,
        });
    }

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
        std::fs::create_dir_all(parent)
            .map_err(|e| fatal(format!("create {}: {e}", target_rel)))?;
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
        sha256: entry.content_hash.clone(),
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
    let bytes = serde_json::to_string_pretty(&document)
        .map_err(|e| format!("serialize prov document: {e}"))?;
    std::fs::write(&path, &bytes).map_err(|e| format!("write prov document: {e}"))?;
    Ok(ExportSummary {
        run_id: run.id.clone(),
        format: "prov",
        out: path.to_string_lossy().into_owned(),
        files: vec![ExportFile {
            source: format!("run:{}", run.id),
            exported_path: file_name,
            bytes: bytes.len() as u64,
            // Self-certifying: the document's own digest, so the export can
            // be checked without re-deriving it.
            sha256: Some(format!(
                "sha256:{}",
                hex_lower(&sha2::Sha256::digest(bytes.as_bytes()))
            )),
            sha256_verified: false,
        }],
        skipped: Vec::new(),
    })
}

/// Where an export should be delivered.
#[derive(Debug, Clone)]
pub enum ExportTarget {
    /// Write directly to this host directory (CLI path).
    Host(PathBuf),
    /// Materialize into a staging directory, then upload under this virtual
    /// prefix so the artifacts are visible through the VFS (agent path).
    Vfs(String),
}

/// Resolve an output path against the object store: `vfs://` uris and paths
/// covered by a mount map to [`ExportTarget::Vfs`]; other absolute paths
/// stay on the host filesystem.
pub fn resolve_export_target(
    out: &str,
    storage: Option<&vfs::OpendalFileStorage>,
) -> Result<ExportTarget, String> {
    if let Some(vpath) = out.strip_prefix("vfs://") {
        return Ok(ExportTarget::Vfs(vfs::OpendalFileStorage::normalize_path(
            vpath,
        )));
    }
    if out.starts_with('/') {
        let normalized = vfs::OpendalFileStorage::normalize_path(out);
        if storage.is_some_and(|storage| storage.is_mounted(&normalized)) {
            return Ok(ExportTarget::Vfs(normalized));
        }
        return Ok(ExportTarget::Host(PathBuf::from(out)));
    }
    Err(format!(
        "out path `{out}` is neither a vfs uri nor an absolute path"
    ))
}

/// Upload a materialized export from `staging` into the object store under
/// `vfs_prefix`, rewriting the summary's paths to their `vfs://` addresses.
/// `summary.out` follows what it pointed at before the upload: the file's
/// `vfs://` address for single-file exports (PROV), the prefix for
/// directory exports (RO-Crate).
pub async fn upload_export_to_vfs(
    summary: &mut ExportSummary,
    staging: &Path,
    vfs_prefix: &str,
    storage: &vfs::OpendalFileStorage,
) -> Result<(), String> {
    let prefix = vfs_prefix.trim_end_matches('/');
    let out_file_index = summary
        .files
        .iter()
        .position(|file| staging.join(&file.exported_path) == Path::new(&summary.out));
    for file in &mut summary.files {
        let local = staging.join(&file.exported_path);
        let bytes = std::fs::read(&local)
            .map_err(|e| format!("read staged {}: {e}", file.exported_path))?;
        let vpath = format!("{prefix}/{}", file.exported_path);
        storage
            .write_bytes(&vpath, bytes)
            .await
            .map_err(|e| format!("upload {vpath}: {e}"))?;
        file.exported_path = format!("vfs://{vpath}");
    }
    summary.out = match out_file_index {
        Some(index) => summary.files[index].exported_path.clone(),
        None => format!("vfs://{prefix}"),
    };
    Ok(())
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
        assert!(
            doc["activity"][tool_activity]["autonomics:image_digest"]
                .as_str()
                .is_some(),
            "container node carries its image digest"
        );
        // Content-addressed output entity + generation link.
        assert!(doc["entity"][out_entity].is_object());
        assert!(
            doc["wasGeneratedBy"]
                .as_array()
                .unwrap()
                .iter()
                .any(|link| link["entity"] == out_entity && link["activity"] == tool_activity)
        );
        // Trigger agent association.
        assert!(doc["agent"]["urn:autonomics:agent:/root/researcher"].is_object());
        assert!(
            doc["wasAssociatedWith"]
                .as_array()
                .unwrap()
                .iter()
                .any(|link| link["activity"] == tool_activity
                    && link["agent"] == "urn:autonomics:agent:/root/researcher")
        );
        // The engine agent carries the source revision.
        assert_eq!(
            doc["agent"]["urn:autonomics:engine:0.1.0"]["autonomics:source_revision"],
            json!("abc1234")
        );
        // DataFrame-only source is represented by its execution fingerprint.
        assert_eq!(
            doc["entity"]["urn:autonomics:run:0123456789abcdef:df:src"]["autonomics:fingerprint"],
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
        storage
            .write_bytes("/artifacts/run-1/out.csv", contents.to_vec())
            .await
            .unwrap();

        // Correct recorded hash.
        let digest = format!("sha256:{}", hex_lower(&sha2_computed(contents)));
        let mut report = sample_report_json();
        report["nodes"][1]["output_files"][0]["fingerprint"]["content_hash"] = json!(digest);

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
        let summary = export_ro_crate(&run, &report, None, out.path(), None)
            .await
            .unwrap();
        let entry = summary
            .files
            .iter()
            .find(|file| file.exported_path.ends_with("local-out.csv"))
            .expect("local output copied");
        assert!(
            !entry.sha256_verified,
            "no recorded hash → unverified, not an error"
        );
        let copied = std::fs::read(out.path().join(&entry.exported_path)).unwrap();
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
        assert_eq!(
            resolve_run(&history, "bbbb2222").await.unwrap().id,
            "bbbb2222"
        );
        assert!(
            resolve_run(&history, "zzzz")
                .await
                .unwrap_err()
                .contains("no run matches")
        );
    }

    fn sha2_computed(bytes: &[u8]) -> Vec<u8> {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).to_vec()
    }
}
