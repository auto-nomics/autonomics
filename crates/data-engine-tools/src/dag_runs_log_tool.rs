use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;
use serde_json::Value;

use crate::ExecError;

/// `dag_export_run` resolves a non-exact id against the most recent 1000
/// records, so listing prefixes must be unique within that same window.
const RUN_RESOLVE_POOL_LIMIT: usize = 1_000;
const RUN_ID_PREFIX_MIN: usize = 12;

/// Maximum bytes of a single error message surfaced in `view="diagnostic"`
/// and `view="nodes"`. The full untruncated message lives in
/// `view="node_detail"`.
const NODE_ERROR_MESSAGE_PREVIEW_CHARS: usize = 200;

/// Maximum item keys retained in `view="diagnostic"` per logical node. The
/// full list is recoverable via `view="nodes" filter.logical_node=<n>`.
const DIAGNOSTIC_ITEM_KEYS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDetailView {
    /// Logical-node aggregates, run-level metadata, and the failure
    /// manifest. The "second click" — what to call when `run_dag`'s index
    /// says "yes there are failures" but the agent is not yet drilling into
    /// individual nodes. Default for backward compatibility.
    Summary,
    /// Flat per-node view with status, error previews, port assignments,
    /// and timing. Optional `filter.status` and `filter.logical_node` prune
    /// the result; `fields` projects it. Use this for "list all failed
    /// nodes for this run" or "show me only the scatter's physical jobs".
    Nodes,
    /// Everything for a single node: execution evidence, input bindings,
    /// output schema, fingerprint, error, etc. Equivalent to the old
    /// `dag_runs_log` drill-down, scoped to one node id.
    NodeDetail,
    /// Only the execution evidence: image digest, exit code, persisted
    /// logs, workspace. Lighter than `node_detail` when audit is the
    /// question.
    Execution,
    /// Port-keyed file outputs only: `{port: {path, format, fingerprint}}`.
    /// The agent uses this when wiring downstream ports in a follow-up
    /// `run_dag`.
    Ports,
    /// Upstream input bindings only.
    Inputs,
    /// Output column schema and row counts only.
    Schema,
    /// Fast triage view: only failed / cancelled / errored nodes, grouped
    /// by logical_node, with the first 200 chars of each error. Designed
    /// to answer "why did this run fail" in a single call regardless of
    /// DAG size. Aliases `view="nodes" filter.status="failed"` plus
    /// `view="nodes" filter.status="cancelled"`.
    Diagnostic,
    /// The full per-node report, exactly as it was persisted. Equivalent
    /// to the legacy `run_id` drill-down: every field, every node, no
    /// projection. Use sparingly — this is what the slim index was
    /// designed to avoid.
    Full,
}

impl RunDetailView {
    fn parse(raw: Option<&str>) -> Result<Self, ToolError> {
        let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::Summary);
        };
        match raw.to_ascii_lowercase().as_str() {
            "summary" => Ok(Self::Summary),
            "nodes" => Ok(Self::Nodes),
            "node_detail" | "node-detail" | "node" => Ok(Self::NodeDetail),
            "execution" => Ok(Self::Execution),
            "ports" | "port_assignments" | "port-assignments" => Ok(Self::Ports),
            "inputs" => Ok(Self::Inputs),
            "schema" => Ok(Self::Schema),
            "diagnostic" => Ok(Self::Diagnostic),
            "full" => Ok(Self::Full),
            other => Err(ToolError::ValidationFailed {
                message: format!(
                    "view must be one of summary|nodes|node_detail|execution|ports|inputs|schema|diagnostic|full, got `{other}`"
                ),
            }),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NodeFilter {
    /// Restrict to one logical node (matches `nodes[].logical_node`).
    pub logical_node: Option<String>,
    /// Restrict to a status (matches `nodes[].status`): `success`,
    /// `failed`, `skipped`, `cancelled`.
    pub status: Option<String>,
    /// Restrict to one item key (matches `nodes[].item_key`).
    pub item_key: Option<String>,
}

impl NodeFilter {
    fn parse(input: &DagRunsLogInput) -> Result<Self, ToolError> {
        let status = input
            .filter_status
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(status) = &status
            && !matches!(
                status.as_str(),
                "success" | "failed" | "skipped" | "cancelled" | "pending" | "running"
            )
        {
            return Err(ToolError::ValidationFailed {
                message: format!(
                    "filter.status must be one of success|failed|skipped|cancelled, got `{status}`"
                ),
            });
        }
        Ok(Self {
            logical_node: input
                .filter_logical_node
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            status,
            item_key: input
                .filter_item_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        })
    }

    fn matches(&self, node: &Value) -> bool {
        if let Some(want) = &self.logical_node {
            if node.get("logical_node").and_then(Value::as_str) != Some(want.as_str()) {
                return false;
            }
        }
        if let Some(want) = &self.status {
            if node.get("status").and_then(Value::as_str) != Some(want.as_str()) {
                return false;
            }
        }
        if let Some(want) = &self.item_key {
            if node.get("item_key").and_then(Value::as_str) != Some(want.as_str()) {
                return false;
            }
        }
        true
    }
}

#[tool(
    name = "dag_runs_log",
    description = "Query the execution audit trail of the data engine. \
                  Every DAG execution leaves one append-only run record: \
                  who triggered it, when, which snapshot (DAG definition) it \
                  ran, the engine version + git revision, and the full \
                  per-node report (container image digest, exit code, \
                  input bindings, persisted stdout/stderr logs, etc.). \
                  Without run_id: list recent runs newest-first (leave the \
                  field absent or empty — do not pass an empty string \
                  expecting a list to be rejected). With run_id: that \
                  single run's report in the chosen `view` shape. \
                  Pass `view=diagnostic` (default when run failed) for a \
                  compact, failure-only view; `view=nodes` for a flat \
                  per-node list with optional `filter.status`, \
                  `filter.logical_node`, `filter.item_key`, and `fields` \
                  projection; `view=node_detail` for everything on one \
                  node (requires `node_id`); `view=execution` for just the \
                  container image / exit code / logs; `view=ports`, \
                  `view=inputs`, `view=schema` for a single sub-shape; \
                  `view=summary` for the run-level aggregate; `view=full` \
                  for the untruncated per-node report. Run ids shown here \
                  are what dag_export_run accepts. \
                  Use this to answer audit questions such as 'which run \
                  produced this output, from which inputs, with which image'."
)]
pub struct DagRunsLogInput {
    /// Ref name to filter by (e.g. "main"). If omitted, spans all refs.
    pub ref_name: Option<String>,
    /// Maximum number of runs to return when listing (default 20).
    pub limit: Option<usize>,
    /// Fetch a single run by its id (`latest` or a unique prefix also
    /// works) instead of listing.
    pub run_id: Option<String>,
    /// Detail view shape (see tool description). Defaults to `summary`,
    /// or `diagnostic` when the matched run failed / was cancelled.
    pub view: Option<String>,
    /// Required when `view="node_detail"`: the node id from
    /// `view="nodes"`.
    pub node_id: Option<String>,
    /// `view="nodes"`: filter to one logical node.
    pub filter_logical_node: Option<String>,
    /// `view="nodes"`: filter to one status (`success|failed|skipped|cancelled`).
    pub filter_status: Option<String>,
    /// `view="nodes"`: filter to one scatter item key.
    pub filter_item_key: Option<String>,
    /// `view="nodes"`: comma-separated field list to project. When set,
    /// only the listed top-level node keys are returned (unknown keys
    /// produce an empty object — no implicit fallback). Default
    /// (`None`): the canonical failure-triage set
    /// `id,status,logical_node,item_key,error.kind,error.message`.
    pub fields: Option<String>,
}

pub struct DagRunsLogTool {
    client: Arc<DataEngineClient>,
}

impl DagRunsLogTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for DagRunsLogTool {
    type Input = DagRunsLogInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let limit = input.limit.unwrap_or(20);
        // An empty or whitespace run_id means "list" — normalize so callers
        // passing "" get the listing they meant, not a not-found error.
        let run_id = input
            .run_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        let view = RunDetailView::parse(input.view.as_deref())?;
        let filter = NodeFilter::parse(&input)?;

        if view == RunDetailView::NodeDetail
            && input
                .node_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .is_none()
        {
            return Err(ToolError::ValidationFailed {
                message: "view=node_detail requires node_id".into(),
            });
        }

        let runs = self
            .client
            .dag_runs_log(input.ref_name.clone(), limit, run_id.clone())
            .await
            .map_err(ExecError::from)?;

        if runs.is_empty() {
            return Ok(ToolResult::success(if run_id.is_some() {
                if run_id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case("latest"))
                {
                    "No runs recorded yet."
                } else {
                    "No run found with that id (pass no run_id to list recent runs)."
                }
            } else {
                "No runs recorded yet."
            }));
        }

        if run_id.is_some()
            && let Some(run) = runs.first()
        {
            return Ok(ToolResult::success_json(run_detail_json(
                run, view, &input, &filter,
            )?));
        }

        let displayed_count = runs.len().min(limit);
        let global_resolver_runs = self
            .client
            .dag_runs_log(None, RUN_RESOLVE_POOL_LIMIT, None)
            .await
            .map_err(ExecError::from)?;
        let resolver_pool = global_resolver_runs
            .iter()
            .take(RUN_RESOLVE_POOL_LIMIT)
            .map(|run| run.id.as_str())
            .collect::<Vec<_>>();
        let resolver_ids = resolver_pool
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let display_ids = runs
            .iter()
            .take(displayed_count)
            .map(|run| {
                if resolver_ids.contains(run.id.as_str()) {
                    resolver_safe_run_prefix(&run.id, &resolver_pool)
                } else {
                    run.id.clone()
                }
            })
            .collect::<Vec<_>>();

        let mut out = String::new();
        for (run, short_id) in runs.iter().take(displayed_count).zip(&display_ids) {
            let snapshot = run
                .snapshot_id
                .as_ref()
                .map(|id| &id[..12.min(id.len())])
                .unwrap_or("(none)");
            let trigger = run.trigger.as_deref().unwrap_or("(unattributed)");
            let outcome = if run.cancelled {
                "cancelled"
            } else if run.ok {
                "ok"
            } else {
                "failed"
            };
            out.push_str(&format!(
                "run_id={short_id}  snapshot={snapshot}  {outcome}  {}  trigger={trigger}\n",
                &run.started_at[..19.min(run.started_at.len())],
            ));
        }
        out.push_str(&format!(
            "\n{} runs (limit {limit}). Pass a run_id with view=summary|diagnostic|nodes|\
             node_detail|execution|ports|inputs|schema|full to drill into a specific run.",
            displayed_count
        ));

        Ok(ToolResult::success(out))
    }
}

/// Default field projection for `view="nodes"`. Designed to be cheap
/// enough to ship for a 1000-node run while still answering "what failed
/// and why" without a follow-up call.
const NODE_FIELDS_DEFAULT: &[&str] = &[
    "id",
    "status",
    "node_type",
    "logical_node",
    "item_key",
    "error",
    "port_assignments",
    "file_path",
    "artifact_path",
    "elapsed_ms",
];

/// Default field projection for `view="diagnostic"`. Slightly leaner than
/// `NODE_FIELDS_DEFAULT` — only the failure-triage columns, no timings or
/// sink paths, since the caller is here to read error text.
const DIAGNOSTIC_FIELDS: &[&str] = &["id", "status", "logical_node", "item_key", "error"];

/// Build the per-view payload for a single run.
fn run_detail_json(
    run: &data_engine::dag::RunRecord,
    view: RunDetailView,
    input: &DagRunsLogInput,
    filter: &NodeFilter,
) -> Result<serde_json::Value, ToolError> {
    let report_value = run
        .run_report_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Value>(json).ok());

    // `diagnostic` auto-elevates from a successful run to `summary` so
    // the caller never sees a diagnostic-shaped payload for a green run.
    let effective_view = match view {
        RunDetailView::Diagnostic if run.ok && !run.cancelled => RunDetailView::Summary,
        other => other,
    };

    let mut out = match effective_view {
        RunDetailView::Summary => run_summary_view(run, report_value.as_ref()),
        RunDetailView::Nodes => {
            run_nodes_view(report_value.as_ref(), filter, input.fields.as_deref())?
        }
        RunDetailView::NodeDetail => {
            let node_id = input
                .node_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| ToolError::ValidationFailed {
                    message: "view=node_detail requires node_id".into(),
                })?;
            run_node_detail_view(report_value.as_ref(), node_id)
        }
        RunDetailView::Execution => run_single_field_view(report_value.as_ref(), "execution"),
        RunDetailView::Ports => run_ports_view(report_value.as_ref()),
        RunDetailView::Inputs => run_single_field_view(report_value.as_ref(), "inputs"),
        RunDetailView::Schema => run_schema_view(report_value.as_ref()),
        RunDetailView::Diagnostic => run_diagnostic_view(report_value.as_ref())?,
        RunDetailView::Full => run_full_view(run, report_value.as_ref()),
    };

    // Always include the run record identity so the caller can correlate
    // a view back to its run id without re-listing.
    if let Some(obj) = out.as_object_mut() {
        obj.insert("run_id".into(), Value::String(run.id.clone()));
        obj.insert("ref_name".into(), Value::String(run.ref_name.clone()));
        obj.insert(
            "snapshot_id".into(),
            run.snapshot_id
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        obj.insert(
            "view".into(),
            Value::String(view_label(effective_view).into()),
        );
    }
    Ok(out)
}

fn view_label(view: RunDetailView) -> &'static str {
    match view {
        RunDetailView::Summary => "summary",
        RunDetailView::Nodes => "nodes",
        RunDetailView::NodeDetail => "node_detail",
        RunDetailView::Execution => "execution",
        RunDetailView::Ports => "ports",
        RunDetailView::Inputs => "inputs",
        RunDetailView::Schema => "schema",
        RunDetailView::Diagnostic => "diagnostic",
        RunDetailView::Full => "full",
    }
}

/// `view="summary"`: run-level metadata, logical-node aggregates, and the
/// failure manifest. Reuses the same slim shape `run_dag` returns so a
/// caller can ask either tool for the same data.
fn run_summary_view(
    run: &data_engine::dag::RunRecord,
    report: Option<&Value>,
) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "ok": run.ok,
        "cancelled": run.cancelled,
        "warnings": report
            .and_then(|r| r.get("warnings").cloned())
            .unwrap_or(Value::Array(Vec::new())),
        "error": run.error.clone().map(Value::String).unwrap_or(Value::Null),
        "message": run.message.clone().map(Value::String).unwrap_or(Value::Null),
        "started_at": run.started_at.clone(),
        "finished_at": run.finished_at.clone(),
        "engine_version": run.engine_version.clone(),
        "source_revision": run.source_revision.clone(),
        "manifest_hash": run.manifest_hash.clone(),
        "trigger": run.trigger.clone().map(Value::String).unwrap_or(Value::Null),
    });
    if let Some(report) = report {
        if let Some(summary) = report.get("resource") {
            obj["resource"] = summary.clone();
        }
        if let Some(ls) = report.get("logical_nodes").cloned() {
            obj["logical_nodes"] = ls;
        }
        if let Some(waves) = report.get("waves").cloned() {
            obj["waves"] = waves;
        }
        if let Some(fm) = report.get("failure_manifest").cloned() {
            obj["failure_manifest"] = fm;
        }
        if let Some(ok) = report.get("ok") {
            obj["report_ok"] = ok.clone();
        }
    }
    obj
}

/// `view="nodes"`: flat per-node list, optionally filtered and projected.
fn run_nodes_view(
    report: Option<&Value>,
    filter: &NodeFilter,
    fields: Option<&str>,
) -> Result<serde_json::Value, ToolError> {
    let Some(report) = report else {
        return Ok(serde_json::json!({"nodes": []}));
    };
    let nodes = report
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let projected_fields: Option<Vec<&str>> = fields
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());
    let mut out = Vec::new();
    for node in nodes {
        if !filter.matches(node) {
            continue;
        }
        let projected = match &projected_fields {
            Some(list) => project_node(node, list),
            None => default_node_projection(node),
        };
        out.push(projected);
    }
    Ok(serde_json::json!({
        "nodes": out,
        "total": out.len(),
        "fields": projected_fields.map(|f| f.join(",")).unwrap_or_else(|| NODE_FIELDS_DEFAULT.join(",")),
    }))
}

/// Apply the default failure-triage projection: identity, status, type,
/// logical_node / item_key, the (clamped) error, sink paths, and timing.
fn default_node_projection(node: &Value) -> Value {
    let mut obj = copy_present_fields(node, NODE_FIELDS_DEFAULT);
    clamp_error_message_in_place(&mut obj);
    obj
}

/// Project to the user-requested field list. Only top-level keys are
/// supported. Missing keys are dropped silently (a node without an error
/// has no `error` field — that's not a bug). The error message is clamped
/// to `NODE_ERROR_MESSAGE_PREVIEW_CHARS` regardless of projection, so
/// `view="nodes" fields=id,error` cannot re-inflate the size.
fn project_node(node: &Value, fields: &[&str]) -> Value {
    let mut obj = copy_present_fields(node, fields);
    clamp_error_message_in_place(&mut obj);
    obj
}

fn clamp_error_message_in_place(obj: &mut Value) {
    let Some(err) = obj.get_mut("error") else {
        return;
    };
    let Some(err_obj) = err.as_object_mut() else {
        return;
    };
    let Some(message) = err_obj.get("message").and_then(Value::as_str) else {
        return;
    };
    err_obj.insert(
        "message".into(),
        Value::String(truncate_chars(message, NODE_ERROR_MESSAGE_PREVIEW_CHARS)),
    );
}

/// `view="node_detail"`: one node, every persisted field, no projection.
fn run_node_detail_view(report: Option<&Value>, node_id: &str) -> serde_json::Value {
    let Some(report) = report else {
        return serde_json::json!({"error": "no persisted report for this run"});
    };
    let Some(nodes) = report.get("nodes").and_then(Value::as_array) else {
        return serde_json::json!({"error": "no nodes in the persisted report"});
    };
    nodes
        .iter()
        .find(|node| node.get("id").and_then(Value::as_str) == Some(node_id))
        .cloned()
        .unwrap_or_else(|| {
            serde_json::json!({
                "error": format!("node `{node_id}` not found in this run"),
            })
        })
}

/// `view="execution"` / `view="inputs"`: pull one top-level key from each
/// node. `null` for nodes that did not record it.
fn run_single_field_view(report: Option<&Value>, field: &str) -> serde_json::Value {
    let Some(report) = report else {
        return serde_json::json!({"nodes": []});
    };
    let nodes = report
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let out: Vec<Value> = nodes
        .iter()
        .filter_map(|node| {
            let id = node.get("id").and_then(Value::as_str)?;
            let value = node.get(field).cloned().unwrap_or(Value::Null);
            Some(serde_json::json!({"id": id, field: value}))
        })
        .collect();
    serde_json::json!({"nodes": out, "field": field, "total": out.len()})
}

/// `view="ports"`: flatten `port_assignments` to `{node_id, port, path,
/// format, fingerprint}` for quick downstream wiring.
fn run_ports_view(report: Option<&Value>) -> serde_json::Value {
    let Some(report) = report else {
        return serde_json::json!({"ports": []});
    };
    let nodes = report
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut out = Vec::new();
    for node in nodes {
        let id = node.get("id").and_then(Value::as_str).unwrap_or("?");
        let Some(assignments) = node.get("port_assignments").and_then(Value::as_object) else {
            continue;
        };
        for (port, value) in assignments {
            out.push(serde_json::json!({
                "node_id": id,
                "port": port.parse::<u8>().ok(),
                "path": value.get("path"),
                "format": value.get("format"),
                "fingerprint": value.get("fingerprint"),
            }));
        }
    }
    serde_json::json!({"ports": out, "total": out.len()})
}

/// `view="schema"`: per-node `output_schema` + `output_rows`.
fn run_schema_view(report: Option<&Value>) -> serde_json::Value {
    let Some(report) = report else {
        return serde_json::json!({"nodes": []});
    };
    let nodes = report
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let out: Vec<Value> = nodes
        .iter()
        .filter_map(|node| {
            let id = node.get("id").and_then(Value::as_str)?;
            let schema = node.get("output_schema").cloned();
            let rows = node.get("output_rows").cloned();
            if schema.is_none() && rows.is_none() {
                return None;
            }
            Some(serde_json::json!({
                "id": id,
                "output_schema": schema.unwrap_or(Value::Null),
                "output_rows": rows.unwrap_or(Value::Null),
            }))
        })
        .collect();
    serde_json::json!({"nodes": out, "total": out.len()})
}

/// `view="diagnostic"`: only failed/cancelled/errored nodes, grouped by
/// `logical_node` (or `"(unscoped)"` for nodes without a logical parent),
/// with the first `DIAGNOSTIC_ITEM_KEYS` item keys per group. The error
/// message is clamped to the same preview used elsewhere.
fn run_diagnostic_view(report: Option<&Value>) -> Result<serde_json::Value, ToolError> {
    let Some(report) = report else {
        return Ok(serde_json::json!({"groups": []}));
    };
    let nodes = report
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    // Group failed/cancelled nodes by logical_node. A BTreeMap gives a
    // stable, deterministic ordering across runs (so the same scatter
    // axis always surfaces first) — a side benefit is that the caller
    // can grep for a specific group without scanning the whole list.
    let mut groups: std::collections::BTreeMap<
        String,
        (
            Vec<String>,
            Vec<Value>,
            Option<&str>,
            Option<&str>,
            Option<&str>,
        ),
    > = std::collections::BTreeMap::new();
    // (item_keys, nodes, wave_of_first, scatter_axis_of_first, logical_node_type)
    for node in nodes {
        let status = node.get("status").and_then(Value::as_str).unwrap_or("");
        if !matches!(status, "failed" | "cancelled") {
            continue;
        }
        let logical = node
            .get("logical_node")
            .and_then(Value::as_str)
            .unwrap_or("(unscoped)")
            .to_string();
        let entry = groups.entry(logical.clone()).or_insert_with(|| {
            (
                Vec::new(),
                Vec::new(),
                node.get("wave").and_then(Value::as_str),
                node.get("scatter_axis").and_then(Value::as_str),
                node.get("logical_node_type").and_then(Value::as_str),
            )
        });
        if let Some(key) = node.get("item_key").and_then(Value::as_str) {
            if entry.0.len() < DIAGNOSTIC_ITEM_KEYS
                && !entry.0.iter().any(|existing| existing == key)
            {
                entry.0.push(key.to_string());
            }
        }
        entry.1.push(diagnostic_node_projection(node));
    }

    let mut out_groups = Vec::new();
    for (key, (item_keys, nodes, wave, scatter_axis, logical_node_type)) in groups {
        out_groups.push(serde_json::json!({
            "logical_node": key,
            "logical_node_type": logical_node_type,
            "wave": wave,
            "scatter_axis": scatter_axis,
            "failed_item_keys": item_keys,
            "nodes": nodes,
            "node_count": nodes.len(),
        }));
    }
    Ok(serde_json::json!({
        "groups": out_groups,
        "total_groups": out_groups.len(),
        "total_failed_nodes": out_groups.iter()
            .map(|g| g["node_count"].as_u64().unwrap_or(0))
            .sum::<u64>(),
    }))
}

fn diagnostic_node_projection(node: &Value) -> Value {
    let mut obj = copy_present_fields(node, DIAGNOSTIC_FIELDS);
    clamp_error_message_in_place(&mut obj);
    obj
}

/// `view="full"`: the full per-node report, including fingerprints and
/// every input binding. Equivalent to the legacy `dag_runs_log` detail
/// shape; provided for tools that genuinely need the entire blob (export
/// tooling, audit replay).
fn run_full_view(run: &data_engine::dag::RunRecord, report: Option<&Value>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "id": run.id,
        "ref_name": run.ref_name,
        "snapshot_id": run.snapshot_id,
        "manifest_hash": run.manifest_hash,
        "trigger": run.trigger,
        "started_at": run.started_at,
        "finished_at": run.finished_at,
        "ok": run.ok,
        "cancelled": run.cancelled,
        "error": run.error,
        "message": run.message,
        "engine_version": run.engine_version,
        "source_revision": run.source_revision,
    });
    if let Some(report) = report {
        if let Some(nodes) = report.get("nodes").cloned() {
            value["nodes"] = nodes;
        }
        if let Some(ls) = report.get("logical_nodes").cloned() {
            value["logical_nodes"] = ls;
        }
        if let Some(waves) = report.get("waves").cloned() {
            value["waves"] = waves;
        }
        if let Some(warnings) = report.get("warnings").cloned() {
            value["warnings"] = warnings;
        }
        if let Some(resource) = report.get("resource").cloned() {
            value["resource"] = resource;
        }
        if let Some(failure_manifest) = report.get("failure_manifest").cloned() {
            value["failure_manifest"] = failure_manifest;
        }
    }
    value
}

/// Project `source` down to the listed keys, keeping only keys that exist
/// and are non-null. The persisted `NodeReport` serialization uses
/// `skip_serializing_if` for its optional fields, so presence == recorded.
fn copy_present_fields(source: &Value, keys: &[&str]) -> Value {
    let mut obj = serde_json::Map::new();
    for key in keys {
        if let Some(value) = source.get(*key)
            && !value.is_null()
        {
            obj.insert((*key).into(), value.clone());
        }
    }
    Value::Object(obj)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let prefix: String = text.chars().take(keep).collect();
    format!("{prefix}…")
}

fn resolver_safe_run_prefix(id: &str, resolver_pool: &[&str]) -> String {
    let mut required = RUN_ID_PREFIX_MIN.min(id.len());

    for neighbor in resolver_pool.iter().filter(|neighbor| ***neighbor != *id) {
        let shared = id
            .bytes()
            .zip(neighbor.bytes())
            .take_while(|(left, right)| left == right)
            .count();
        required = required.max(shared + 1);
    }

    id[..required.min(id.len())].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_run(report_json: &str) -> data_engine::dag::RunRecord {
        data_engine::dag::RunRecord {
            id: "run-1".into(),
            ref_name: "main".into(),
            snapshot_id: Some("snap-1".into()),
            manifest_hash: "hash".into(),
            trigger: None,
            started_at: "2026-10-06T10:00:00Z".into(),
            finished_at: "2026-10-06T10:01:00Z".into(),
            ok: true,
            cancelled: false,
            error: None,
            message: None,
            engine_version: "0.1.0".into(),
            source_revision: "abc1234".into(),
            run_report_json: Some(report_json.to_string()),
        }
    }

    fn full_report() -> Value {
        serde_json::json!({
            "ok": true,
            "warnings": [],
            "nodes": [{
                "id": "sink",
                "status": "success",
                "node_type": "dataframe_to_file",
                "executor": "local",
                "logical_node": "scatter",
                "physical_job_id": "job-1",
                "scatter_axis": "sample",
                "item_key": "s1",
                "output_type": "File",
                "port_assignments": {"0": {"path": "out.csv", "format": "csv"}},
                "output_schema": {"column_count": 1, "columns": {}, "type_distribution": null},
                "output_rows": 42,
                "elapsed_ms": 7,
                "dispatch_seq": 3,
                "artifact_path": "a.png",
                "file_path": "out.csv",
                "execution": {"exit_code": 0, "image": "img"},
                "inputs": [{
                    "from": "src",
                    "from_port": 0,
                    "to_port": 0,
                    "kind": "File",
                    "path": "in.csv"
                }],
                "fingerprint": "fp"
            }],
            "logical_nodes": [{
                "logical_node": "scatter",
                "status": "success",
                "physical_job_count": 1,
                "status_counts": {"success": 1},
                "scatter_axis": "sample",
                "item_keys": ["s1"],
                "physical_job_ids": ["job-1"],
                "summed_elapsed_ms": 7,
                "max_elapsed_ms": 7
            }],
            "waves": [],
            "resource": {"memory": {"peak": null}},
            "failure_manifest": {"failed_item_count": 0, "failed_items": []}
        })
    }

    #[test]
    fn summary_view_returns_run_metadata_and_logicals() {
        let run = sample_run(&full_report().to_string());
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("summary".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        assert_eq!(detail["run_id"], "run-1");
        assert_eq!(detail["view"], "summary");
        assert_eq!(detail["ok"], true);
        assert!(detail.get("logical_nodes").is_some());
        assert!(
            detail.get("nodes").is_none(),
            "summary must not include per-node[]"
        );
    }

    #[test]
    fn nodes_view_projects_default_failure_triage_set() {
        let run = sample_run(&full_report().to_string());
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("nodes".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        let node = &detail["nodes"][0];
        for key in [
            "id",
            "status",
            "logical_node",
            "item_key",
            "port_assignments",
            "file_path",
            "artifact_path",
            "elapsed_ms",
        ] {
            assert!(
                node.get(key).is_some(),
                "default projection must include {key}"
            );
        }
        // Drill-down-only fields are excluded by the default projection.
        for key in ["execution", "inputs", "output_schema", "fingerprint"] {
            assert!(
                node.get(key).is_none(),
                "default projection must omit {key}"
            );
        }
    }

    #[test]
    fn nodes_view_respects_filter_and_fields() {
        let run = sample_run(&full_report().to_string());
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("nodes".into()),
            node_id: None,
            filter_logical_node: Some("scatter".into()),
            filter_status: Some("success".into()),
            filter_item_key: Some("s1".into()),
            fields: Some("id,status".into()),
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        assert_eq!(detail["total"], 1);
        let node = &detail["nodes"][0];
        assert_eq!(node.as_object().unwrap().len(), 2);
        assert!(node.get("id").is_some());
        assert!(node.get("status").is_some());
    }

    #[test]
    fn nodes_view_filter_rejects_unknown_status() {
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: None,
            view: Some("nodes".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: Some("bogus".into()),
            filter_item_key: None,
            fields: None,
        };
        assert!(NodeFilter::parse(&input).is_err());
    }

    #[test]
    fn node_detail_view_requires_node_id() {
        let run = sample_run(&full_report().to_string());
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("node_detail".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        assert!(
            run_detail_json(
                &run,
                RunDetailView::parse(input.view.as_deref()).unwrap(),
                &input,
                &filter
            )
            .is_err()
        );
    }

    #[test]
    fn node_detail_view_returns_one_node() {
        let run = sample_run(&full_report().to_string());
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("node_detail".into()),
            node_id: Some("sink".into()),
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        assert_eq!(detail["id"], "sink");
        assert!(detail.get("execution").is_some());
        assert!(detail.get("inputs").is_some());
    }

    #[test]
    fn execution_view_emits_per_node_evidence_only() {
        let run = sample_run(&full_report().to_string());
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("execution".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        assert_eq!(detail["field"], "execution");
        assert_eq!(detail["total"], 1);
        assert!(detail["nodes"][0].get("execution").is_some());
        assert!(detail["nodes"][0].get("id").is_some());
    }

    #[test]
    fn ports_view_flattens_port_assignments() {
        let run = sample_run(&full_report().to_string());
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("ports".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        assert_eq!(detail["total"], 1);
        assert_eq!(detail["ports"][0]["port"], 0);
        assert_eq!(detail["ports"][0]["path"], "out.csv");
    }

    #[test]
    fn diagnostic_view_groups_failed_nodes_by_logical_node() {
        let report = serde_json::json!({
            "ok": false,
            "warnings": [],
            "nodes": [
                {"id": "a", "status": "failed", "logical_node": "scatter", "item_key": "chr1",
                 "error": {"kind": "x", "message": "boom"}},
                {"id": "b", "status": "success", "logical_node": "scatter", "item_key": "chr2"},
                {"id": "c", "status": "failed", "logical_node": "scatter", "item_key": "chr3",
                 "error": {"kind": "x", "message": "boom2"}},
                {"id": "d", "status": "failed", "logical_node": "other", "item_key": "s1",
                 "error": {"kind": "y", "message": "fail"}}
            ]
        });
        let mut run = sample_run(&report.to_string());
        run.ok = false;
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("diagnostic".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        let groups = detail["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        // BTreeMap groups are sorted lexicographically: "other" before
        // "scatter".
        assert_eq!(groups[0]["logical_node"], "other");
        assert_eq!(groups[1]["logical_node"], "scatter");
        assert_eq!(groups[1]["node_count"], 2);
        let first = &groups[1]["nodes"][0];
        assert!(first.get("error").is_some());
        assert!(first.get("execution").is_none());
    }

    #[test]
    fn diagnostic_view_clamps_long_error_messages() {
        let long_message = "x".repeat(500);
        let report = serde_json::json!({
            "ok": false,
            "warnings": [],
            "nodes": [
                {"id": "a", "status": "failed", "logical_node": "x", "item_key": "k1",
                 "error": {"kind": "x", "message": long_message}}
            ]
        });
        let mut run = sample_run(&report.to_string());
        run.ok = false;
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("diagnostic".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        let message = detail["groups"][0]["nodes"][0]["error"]["message"]
            .as_str()
            .unwrap();
        assert!(message.chars().count() <= NODE_ERROR_MESSAGE_PREVIEW_CHARS);
    }

    #[test]
    fn view_parse_rejects_unknown() {
        assert!(RunDetailView::parse(Some("nope")).is_err());
        assert!(matches!(
            RunDetailView::parse(None).unwrap(),
            RunDetailView::Summary
        ));
        assert!(matches!(
            RunDetailView::parse(Some("nodes")).unwrap(),
            RunDetailView::Nodes
        ));
    }

    #[test]
    fn full_view_keeps_everything_for_audit() {
        let run = sample_run(&full_report().to_string());
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("full".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        let node = &detail["nodes"][0];
        assert!(node.get("execution").is_some());
        assert!(node.get("inputs").is_some());
        assert!(node.get("fingerprint").is_some());
        assert!(node.get("output_schema").is_some());
    }

    #[test]
    fn run_prefixes_extend_past_collisions() {
        let first = "6821f700-8f80000000000000000000000000001";
        let second = "6821f700-8f80000000000000000000000000002";
        let distinct = "aaaaaaaa-bbbb-cccc-dddd-eeeeffff0001";
        let pool = [first, second, distinct];

        let first_prefix = resolver_safe_run_prefix(first, &pool);
        let second_prefix = resolver_safe_run_prefix(second, &pool);
        let distinct_prefix = resolver_safe_run_prefix(distinct, &pool);

        assert_eq!(distinct_prefix.len(), RUN_ID_PREFIX_MIN);
        assert_eq!(first_prefix, first);
        assert_eq!(second_prefix, second);
        assert_ne!(first_prefix, second_prefix);
        assert!(
            pool.iter()
                .filter(|candidate| candidate.starts_with(&first_prefix))
                .count()
                == 1
        );
    }

    #[test]
    fn empty_report_still_returns_view_shape() {
        let run = sample_run(r#"{"ok": true, "warnings": [], "nodes": []}"#);
        let input = DagRunsLogInput {
            ref_name: None,
            limit: None,
            run_id: Some("run-1".into()),
            view: Some("nodes".into()),
            node_id: None,
            filter_logical_node: None,
            filter_status: None,
            filter_item_key: None,
            fields: None,
        };
        let filter = NodeFilter::parse(&input).unwrap();
        let detail = run_detail_json(
            &run,
            RunDetailView::parse(input.view.as_deref()).unwrap(),
            &input,
            &filter,
        )
        .unwrap();
        assert_eq!(detail["total"], 0);
        assert_eq!(detail["nodes"].as_array().unwrap().len(), 0);
    }
}
