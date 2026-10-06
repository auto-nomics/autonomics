//! Run-report assembly: per-node [`NodeReport`]s, logical-node aggregation
//! ([`LogicalRunSummary`]), and the opt-in eager row counts.

use std::collections::BTreeMap;

use datafusion::common::HashMap;

use super::super::logical::{LogicalExecutionStrategy, LogicalNodeDefinition};
use super::DAG;
use crate::dag::NodeId;
use crate::dag::runtime::{
    LogicalJobError, LogicalRunSummary, NodeReport, RuntimeStatus, SchemaReport,
};
use crate::value::NodeValue;

/// Accumulates the physical reports under one logical node into a
/// [`LogicalRunSummary`].
#[derive(Default)]
struct LogicalSummaryBuilder {
    execution_strategy: Option<&'static str>,
    logical_node_type: Option<String>,
    scatter_axis: Option<String>,
    status_counts: BTreeMap<String, usize>,
    item_keys: Vec<String>,
    failed_item_keys: Vec<String>,
    skipped_item_keys: Vec<String>,
    physical_job_ids: Vec<String>,
    summed_elapsed_ms: u64,
    max_elapsed_ms: Option<u64>,
    errors: Vec<LogicalJobError>,
}

impl LogicalSummaryBuilder {
    fn record(&mut self, report: &NodeReport) {
        *self
            .status_counts
            .entry(runtime_status_name(report.status).to_string())
            .or_insert(0) += 1;
        self.scatter_axis
            .get_or_insert_with(|| report.scatter_axis.clone().unwrap_or_default());
        if self.scatter_axis.as_deref() == Some("") {
            self.scatter_axis = None;
        }
        if let Some(item_key) = &report.item_key {
            self.item_keys.push(item_key.clone());
        }
        self.physical_job_ids.push(
            report
                .physical_job_id
                .clone()
                .unwrap_or_else(|| report.id.clone()),
        );
        if let Some(elapsed_ms) = report.elapsed_ms {
            self.summed_elapsed_ms += elapsed_ms;
            self.max_elapsed_ms = Some(
                self.max_elapsed_ms
                    .map_or(elapsed_ms, |current| current.max(elapsed_ms)),
            );
        }
        if report.status == RuntimeStatus::Failed {
            if let Some(item_key) = &report.item_key {
                self.failed_item_keys.push(item_key.clone());
            }
            if let Some(error) = &report.error {
                self.errors.push(LogicalJobError {
                    physical_job_id: report
                        .physical_job_id
                        .clone()
                        .unwrap_or_else(|| report.id.clone()),
                    item_key: report.item_key.clone(),
                    error: error.clone(),
                });
            }
        }
        if report.status == RuntimeStatus::Skipped {
            if let Some(item_key) = &report.item_key {
                self.skipped_item_keys.push(item_key.clone());
            }
        }
    }

    fn finish(mut self, logical_node: String) -> LogicalRunSummary {
        self.item_keys.sort();
        self.failed_item_keys.sort();
        self.skipped_item_keys.sort();
        self.physical_job_ids.sort();
        let status = aggregate_logical_status(&self.status_counts, self.physical_job_ids.len());
        LogicalRunSummary {
            logical_node,
            execution_strategy: self.execution_strategy,
            logical_node_type: self.logical_node_type,
            status,
            physical_job_count: self.physical_job_ids.len(),
            status_counts: self.status_counts,
            scatter_axis: self.scatter_axis,
            item_keys: self.item_keys,
            failed_item_keys: self.failed_item_keys,
            skipped_item_keys: self.skipped_item_keys,
            physical_job_ids: self.physical_job_ids,
            summed_elapsed_ms: self.summed_elapsed_ms,
            max_elapsed_ms: self.max_elapsed_ms,
            errors: self.errors,
        }
    }
}

fn logical_execution_strategy_name(strategy: &LogicalExecutionStrategy) -> &'static str {
    match strategy {
        LogicalExecutionStrategy::Once => "once",
        LogicalExecutionStrategy::ForEach { .. } => "for_each",
        LogicalExecutionStrategy::DynamicForEach { .. } => "dynamic_for_each",
        LogicalExecutionStrategy::Gather => "gather",
    }
}

fn logical_node_type(definition: &LogicalNodeDefinition) -> String {
    match definition {
        LogicalNodeDefinition::Registry { kind, .. } => kind.clone(),
        LogicalNodeDefinition::Gather => "logical_gather".to_string(),
        LogicalNodeDefinition::Channel(_) => "channel".to_string(),
    }
}

fn logical_scatter_axis(strategy: &LogicalExecutionStrategy) -> Option<String> {
    match strategy {
        LogicalExecutionStrategy::ForEach { axis, .. } => Some(axis.clone()),
        LogicalExecutionStrategy::DynamicForEach { axis, .. } => Some(axis.clone()),
        LogicalExecutionStrategy::Once | LogicalExecutionStrategy::Gather => None,
    }
}

fn runtime_status_name(status: RuntimeStatus) -> &'static str {
    match status {
        RuntimeStatus::Pending => "pending",
        RuntimeStatus::Ready => "ready",
        RuntimeStatus::Running => "running",
        RuntimeStatus::Success => "success",
        RuntimeStatus::Failed => "failed",
        RuntimeStatus::Skipped => "skipped",
        RuntimeStatus::Cancelled => "cancelled",
    }
}

fn aggregate_logical_status(
    status_counts: &BTreeMap<String, usize>,
    physical_job_count: usize,
) -> RuntimeStatus {
    let count = |name: &str| status_counts.get(name).copied().unwrap_or(0);
    if physical_job_count == 0 {
        return RuntimeStatus::Pending;
    }
    if count("failed") > 0 {
        RuntimeStatus::Failed
    } else if count("cancelled") > 0 {
        RuntimeStatus::Cancelled
    } else if count("running") > 0 {
        RuntimeStatus::Running
    } else if count("ready") > 0 {
        RuntimeStatus::Ready
    } else if count("pending") > 0 {
        RuntimeStatus::Pending
    } else if count("success") > 0 {
        RuntimeStatus::Success
    } else {
        RuntimeStatus::Skipped
    }
}

impl DAG {
    /// Aggregate physical execution reports under their logical source nodes.
    ///
    /// Every logical node is represented even when it currently has no
    /// physical jobs. That makes a deleted scatter sibling visible as a
    /// zero-job logical summary instead of silently disappearing from the
    /// logical execution view.
    pub(super) fn build_logical_run_summaries(
        &self,
        reports: &[NodeReport],
    ) -> Vec<LogicalRunSummary> {
        let mut builders = BTreeMap::new();
        for graph in &self.logical_graphs {
            for node in graph.nodes() {
                let builder = LogicalSummaryBuilder {
                    execution_strategy: Some(logical_execution_strategy_name(&node.strategy)),
                    logical_node_type: Some(logical_node_type(&node.definition)),
                    scatter_axis: logical_scatter_axis(&node.strategy),
                    ..LogicalSummaryBuilder::default()
                };
                builders.entry(node.id.clone()).or_insert(builder);
            }
        }

        for report in reports
            .iter()
            .filter(|report| report.logical_node.is_some())
            .filter(|report| {
                self.specs
                    .get(&report.id)
                    .map(|(kind, _)| kind != "dynamic_fanout")
                    .unwrap_or(true)
            })
        {
            let logical_node = report.logical_node.clone().unwrap();
            builders.entry(logical_node).or_default().record(report);
        }

        builders
            .into_iter()
            .map(|(logical_node, builder)| builder.finish(logical_node))
            .collect()
    }

    /// Build per-node [`NodeReport`] summaries from the execution state
    /// available after the scheduler loop.
    ///
    /// Row counts (`output_rows`) require running `SELECT COUNT(*)` over the
    /// LogicalPlan of every successful node's primary output DataFrame. That is
    /// an **eager** operation — for a source over a multi-GB VCF.gz it forces
    /// the file to be decompressed, parsed, and counted end-to-end. We never
    /// compute row counts here unless the caller explicitly opted in via
    /// [`crate::dag::runtime::SchedulerConfig::compute_row_counts`]. When
    /// opted in, the per-node count futures are joined concurrently via
    /// `futures::future::join_all` so the count phase runs in parallel rather
    /// than sequentially.
    pub(super) async fn build_node_reports(
        &self,
        all_ids: &[NodeId],
        durations: &HashMap<NodeId, std::time::Duration>,
        skipped_because: &HashMap<NodeId, NodeId>,
        dispatch_order: &HashMap<NodeId, u64>,
        compute_row_counts: bool,
    ) -> Vec<NodeReport> {
        // Only run `count()` when the caller opted in. Default is off, so a
        // VCF source node does NOT trigger full decompression/parse just to
        // surface `output_rows` in the report.
        let counts: HashMap<NodeId, usize> = if compute_row_counts {
            self.collect_row_counts(all_ids).await
        } else {
            HashMap::new()
        };

        all_ids
            .iter()
            .map(|id| {
                let status = self
                    .statuses
                    .get(id)
                    .copied()
                    .unwrap_or(RuntimeStatus::Pending);
                let node_type = self
                    .nodes
                    .get(id)
                    .map(|n| n.kind())
                    .unwrap_or("unknown")
                    .to_string();
                let output_type = self
                    .outputs
                    .get(id)
                    .and_then(|outputs| outputs.values().next())
                    .map(|value| value.data_type().to_string());
                let port_assignments = self
                    .outputs
                    .get(id)
                    .and_then(|outputs| {
                        self.nodes.get(id).map(|node| {
                            node.ports()
                                .output_ports()
                                .iter()
                                .filter_map(|port| {
                                    outputs
                                        .get(&port.index)
                                        .and_then(|value| value.as_file().ok())
                                        .cloned()
                                        .map(|file| (port.index, file))
                                })
                                .collect::<std::collections::BTreeMap<_, _>>()
                        })
                    })
                    .unwrap_or_default();
                let output_files = self
                    .outputs
                    .get(id)
                    .map(|outputs| {
                        outputs
                            .values()
                            .flat_map(|value| match value {
                                NodeValue::File(file) => vec![file.clone()],
                                NodeValue::FileSet(files) => files.clone(),
                                NodeValue::DataFrame(_) => Vec::new(),
                                NodeValue::Channel(_) => Vec::new(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                // Extract output schema from the first output port's DataFrame.
                // `schema()` only inspects the LogicalPlan — it does NOT trigger
                // execution, so it's safe (and free) to query unconditionally.
                // Wide schemas are folded to a leading-column sample + type
                // distribution (see `SchemaReport`).
                let output_schema = self
                    .outputs
                    .get(id)
                    .and_then(|outputs| {
                        outputs.values().find_map(|value| value.as_dataframe().ok())
                    })
                    .map(|df| SchemaReport::from_fields(df.schema().fields()));

                let output_rows = counts.get(id).copied();
                let elapsed_ms = durations.get(id).map(|d| d.as_millis() as u64);
                let dispatch_seq = dispatch_order.get(id).copied();
                let executor = dispatch_seq
                    .is_some()
                    .then(|| self.task_executor.executor.name());

                // Extract file sink path / artifact path via DagNode trait hooks.
                let file_path = self
                    .nodes
                    .get(id)
                    .and_then(|n| n.sink_path().map(|s| s.to_string()));

                let artifact_path = self
                    .nodes
                    .get(id)
                    .and_then(|n| n.artifact_path().map(|s| s.to_string()));

                let error = self.errors.get(id).map(|e| e.to_report());
                let skipped_because = skipped_because.get(id).cloned();

                // Audit trail: what the node reported about its own
                // execution, and which upstream values were injected into it.
                let execution = self.node_run_details.get(id).cloned();
                let inputs = self.input_bindings.get(id).cloned().unwrap_or_default();
                let fingerprint = self.fingerprints.get(id).cloned();
                let physical_job = self.physical_jobs.get(id);
                let task_resources = self
                    .task_resources
                    .get(id)
                    .or_else(|| {
                        physical_job
                            .and_then(|job| self.logical_task_resources.get(&job.logical_node))
                    })
                    .cloned()
                    .unwrap_or_else(|| self.default_task_resources.clone());

                NodeReport {
                    id: id.clone(),
                    status,
                    node_type,
                    executor,
                    resources: task_resources,
                    logical_node: physical_job.map(|job| job.logical_node.clone()),
                    physical_job_id: physical_job.map(|_| id.clone()),
                    scatter_axis: physical_job.and_then(|job| job.axis.clone()),
                    item_key: physical_job.and_then(|job| job.item_key.clone()),
                    output_type,
                    output_files,
                    port_assignments,
                    output_schema,
                    output_rows,
                    elapsed_ms,
                    dispatch_seq,
                    artifact_path,
                    file_path,
                    error,
                    skipped_because,
                    execution,
                    inputs,
                    fingerprint,
                }
            })
            .collect()
    }

    /// Drive `df.count()` for every successful node's primary output DataFrame
    /// concurrently. Returns an empty map if no node produced output.
    ///
    /// **This is the only place in the runtime that voluntarily does eager I/O
    /// on output DataFrames.** Callers must gate it behind
    /// `SchedulerConfig::compute_row_counts` — invoking it unconditionally
    /// would force every source (e.g. `.vcf.gz`) to be fully scanned.
    ///
    /// Execution model: each count future is spawned on the runtime and we
    /// `join_all` them so multiple nodes' counts run in parallel instead of
    /// blocking the report on the slowest single scan.
    async fn collect_row_counts(&self, all_ids: &[NodeId]) -> HashMap<NodeId, usize> {
        // Build a list of (id, owned future). Cloning `DataFrame` is cheap (it
        // wraps an `Arc<LogicalPlan>`), so we can move each clone into its own
        // future without contention.
        let mut pairs: Vec<(
            NodeId,
            futures::future::BoxFuture<'static, datafusion::error::Result<usize>>,
        )> = Vec::new();
        for id in all_ids {
            let Some(dfs) = self.outputs.get(id) else {
                continue;
            };
            let Some(df) = dfs.values().find_map(|value| value.as_dataframe().ok()) else {
                continue;
            };
            let owned = df.clone();
            let id_owned = id.clone();
            pairs.push((id_owned, Box::pin(async move { owned.count().await })));
        }

        // Join all counts concurrently. If any future panics or returns an
        // error, fall back to 0 for that node — the row count is purely
        // diagnostic and shouldn't poison the report.
        let futs: Vec<_> = pairs.into_iter().map(|(_, f)| f).collect();
        let results = futures::future::join_all(futs).await;

        // We need the ids in the order that matches `results`. Rebuild the
        // ordered list by walking `all_ids` and skipping nodes that had no
        // output DataFrame — keeping the result-index aligned with the source
        // ordering.
        let mut counts: HashMap<NodeId, usize> = HashMap::new();
        let mut result_iter = results.into_iter();
        for id in all_ids {
            if self
                .outputs
                .get(id)
                .and_then(|d| d.values().next())
                .is_some()
                && let Some(res) = result_iter.next()
            {
                counts.insert(id.clone(), res.unwrap_or(0));
            }
        }
        counts
    }
}
