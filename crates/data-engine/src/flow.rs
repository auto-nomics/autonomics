//! Stage-model flow execution hosted by the [`DataEngine`].
//!
//! [`FlowRunner`] assembles the new [`dag_engine`] process-model scheduler
//! from the same ingredients the legacy DAG path uses — the node registry
//! (every registered kind, including containerized ones, works unchanged),
//! the engine-wide [`NodeCtx`], the VFS storage view, and the executor stack
//! behind [`DataEngine::task_executor`] (local / compute-router / Slurm).
//!
//! Unlike a DAG run, a flow run does **not** hold the engine mutex while
//! executing: the authored graph arrives with the request, instances never
//! touch engine state, so the runner is built under the lock and released
//! immediately.

use std::sync::Arc;

use dag_engine::{
    FlowGraph, FlowRunConfig, FlowRunReport, FlowScheduler, InstanceCache, NodeBuilder,
    stream_sources_with_functions,
};
use tokio_util::sync::CancellationToken;
use vfs::OpendalFileStorage;

use crate::data_engine::DataEngine;

/// Options for one flow run.
#[derive(Debug, Clone, Default)]
pub struct FlowOptions {
    /// Maximum instances executing concurrently. `None` uses the engine
    /// default (CPU-derived), matching [`FlowRunConfig::default`].
    pub max_concurrency: Option<usize>,
    /// Reuse instances whose recomputed fingerprint matches a cached result
    /// with still-valid output files (the flow engine's `-resume`).
    pub resume: Option<bool>,
}

/// Combined view payload for surfaces (TUI / GUI / gateway): the authored
/// flow plus the most recent run's grouped report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowView {
    pub flow: FlowGraph,
    /// The engine's node-registry kind for each process is carried in the
    /// flow itself; this summary adds only run outcome.
    pub last_run: Option<FlowRunReportSummary>,
}

/// The observation-safe subset of [`FlowRunReport`] for views: per-process
/// counts and the ok flag. Instances stay drill-down-only.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowRunReportSummary {
    pub ok: bool,
    #[serde(default)]
    pub cancelled: bool,
    pub processes: std::collections::BTreeMap<String, dag_engine::ProcessRunSummary>,
}

impl From<&FlowRunReport> for FlowRunReportSummary {
    fn from(report: &FlowRunReport) -> Self {
        Self {
            ok: report.ok,
            cancelled: report.cancelled,
            processes: report
                .processes
                .iter()
                .map(|(name, summary)| (name.clone(), summary.clone()))
                .collect(),
        }
    }
}

/// A ready-to-run flow scheduler bound to one engine's ingredients. Cheap to
/// build; holds only `Arc` clones and one closure.
pub struct FlowRunner {
    scheduler: FlowScheduler,
    storage: Option<Arc<OpendalFileStorage>>,
    source_function: dag_engine::ChannelFunction,
    /// Cross-run instance results, shared with every other runner this
    /// session assembles. Each isolated agent has its own cache.
    cache: Arc<tokio::sync::Mutex<InstanceCache>>,
}

impl DataEngine {
    /// Assemble a flow runner from this engine's registry, execution context
    /// and executor stack. The authored flow graph is supplied per run, so
    /// the engine's own (legacy) DAG state is never consulted or mutated.
    pub fn flow_runner(&self) -> FlowRunner {
        let function_registry = Arc::clone(&self.node_registry);
        let function_snapshot = crate::dag_shell::DagShellSnapshot::from_dag(&self.dag);
        let binary_registry = Arc::clone(&function_registry);
        let binary_snapshot = function_snapshot.clone();
        let channel_binary_function: dag_engine::ChannelBinaryFunction =
            Arc::new(move |function, first, second| {
                crate::dag_shell::run_channel_function_with_inputs(
                    function,
                    Some(first),
                    Some(second),
                    &binary_snapshot,
                    &binary_registry,
                )
            });
        let channel_function: dag_engine::ChannelFunction = Arc::new(move |function, input| {
            crate::dag_shell::run_channel_function(
                function,
                input,
                &function_snapshot,
                &function_registry,
            )
        });
        let registry = Arc::clone(&self.node_registry);
        let node_builder: NodeBuilder = Arc::new(move |kind, spec| {
            if kind == "channel" {
                return dag_core::dag::ChannelNode::from_spec(&spec)
                    .map(|node| Box::new(node) as Box<dyn dag_core::node::DagNode>)
                    .map_err(|error| dag_engine::Error::Execution(error.to_string()));
            }
            registry.build_node(kind, spec).map_err(|error| {
                dag_engine::Error::Execution(format!("cannot build node `{kind}`: {error}"))
            })
        });
        FlowRunner {
            scheduler: FlowScheduler::new(
                self.dag.task_executor(),
                node_builder,
                Arc::new(self.engine_ctx.clone()),
            )
            .with_channel_function(channel_function.clone())
            .with_channel_binary_function(channel_binary_function),
            storage: self.engine_ctx.opendal.clone(),
            source_function: channel_function,
            cache: Arc::clone(&self.flow_cache),
        }
    }
}

/// Build the process-level projection for the currently executing flow.
pub(crate) fn flow_snapshot(flow: &FlowGraph) -> dag_core::dag::view::DagTuiSnapshot {
    use dag_core::dag::{runtime::RuntimeStatus, view::{DagEdgeView, DagNodeView, DagPortView, DagTuiSnapshot}};
    let mut nodes = Vec::new();
    for (id, kind) in flow.sources().iter().map(|s| (s.id.clone(), "source".to_string()))
        .chain(flow.processes().iter().map(|p| (p.id.clone(), p.kind.clone()))) {
        let ports = |input: bool| {
            let mut indices: Vec<u8> = flow.wires().iter().filter_map(|w| {
                if input && w.to == id { Some(w.to_port) }
                else if !input && w.from == id { Some(w.from_port) }
                else { None }
            }).collect();
            indices.sort_unstable(); indices.dedup();
            indices.into_iter().map(|index| DagPortView { index, label: None, data_type: "channel".into() }).collect()
        };
        nodes.push(DagNodeView { inputs: ports(true), outputs: ports(false), id, kind, status: RuntimeStatus::Pending, dirty: false, execution: None });
    }
    let edges = flow.wires().iter().map(|w| DagEdgeView { from: w.from.clone(), from_port: w.from_port, to: w.to.clone(), to_port: w.to_port }).collect();
    DagTuiSnapshot { nodes, edges, running: false, run_id: Some(uuid::Uuid::new_v4().to_string()), observed_at_ms: 0 }
}

impl FlowRunner {
    pub fn with_live_view(mut self, view: dag_core::dag::live::LiveDagView) -> Self {
        self.scheduler = self.scheduler.with_live_view(view);
        self
    }

    /// Execute a reactive flow, enumerating file sources through the VFS.
    /// Source errors encountered during enumeration fail the run; tasks may
    /// already have started when a later source item or error arrives.
    pub async fn run(
        &self,
        flow: &FlowGraph,
        options: &FlowOptions,
        cancel: Option<CancellationToken>,
    ) -> Result<FlowRunReport, crate::error::Error> {
        let report = self
            .run_internal(flow, options, cancel)
            .await
            .map_err(|error| crate::error::Error::Custom(error.to_string()))?;
        Ok(report)
    }

    async fn run_internal(
        &self,
        flow: &FlowGraph,
        options: &FlowOptions,
        cancel: Option<CancellationToken>,
    ) -> Result<FlowRunReport, dag_engine::Error> {
        let streams = stream_sources_with_functions(
            flow,
            self.storage.clone(),
            Some(self.source_function.clone()),
        )?;
        let config = FlowRunConfig {
            max_concurrency: options
                .max_concurrency
                .unwrap_or_else(|| FlowRunConfig::default().max_concurrency),
            resume: options.resume.unwrap_or(false),
        };
        let mut cache = self.cache.lock().await;
        self.scheduler
            .run_dataflow_streamed_cached(flow, streams, &config, Some(&mut cache), cancel)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dag_engine::model::{Process, Source, Wire, WireOp};

    fn engine() -> DataEngine {
        DataEngine::builder().build()
    }

    #[tokio::test]
    async fn singleton_echo_flow_runs_through_the_real_registry() {
        let flow = FlowGraph::builder()
            .process(Process::new("hello", "echo", serde_json::json!({})))
            .build()
            .unwrap();
        let report = engine()
            .flow_runner()
            .run(&flow, &FlowOptions::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.processes);
        assert_eq!(report.processes["hello"].succeeded, 1);
    }

    #[tokio::test]
    async fn source_driven_flow_executes_one_instance_per_item() {
        // file_reference resolves each delivered path — one real node per item.
        let directory = tempfile::tempdir().unwrap();
        let items: Vec<serde_json::Value> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let path = directory.path().join(format!("{name}.txt"));
                std::fs::write(&path, name).unwrap();
                serde_json::json!(path.display().to_string())
            })
            .collect();
        let flow = FlowGraph::builder()
            .source(Source::items("reads", items))
            .process(Process::new(
                "batch",
                "file_reference",
                serde_json::json!({ "path": "{{item}}" }),
            ))
            .wire(Wire::direct("reads", 0, "batch", 0))
            .build()
            .unwrap();
        let report = engine()
            .flow_runner()
            .run(&flow, &FlowOptions::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.processes);
        assert_eq!(report.processes["batch"].total, 3);
        assert_eq!(report.processes["batch"].succeeded, 3);
        // Authored state stays process-granular regardless of item count.
        assert_eq!(flow.processes().len(), 1);
    }

    #[tokio::test]
    async fn unknown_kind_fails_before_dispatch() {
        let flow = FlowGraph::builder()
            .process(Process::new(
                "broken",
                "no_such_kind",
                serde_json::json!({}),
            ))
            .build()
            .unwrap();
        let error = engine()
            .flow_runner()
            .run(&flow, &FlowOptions::default(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no_such_kind"), "{error}");
    }

    #[tokio::test]
    async fn glob_without_registered_storage_reports_clearly() {
        // A default engine has no opendal storage registered; a glob source
        // must fail with the explicit no-storage error, not a panic.
        let flow = FlowGraph::builder()
            .source(Source::glob("reads", "data/*.fastq"))
            .process(Process::new("scan", "echo", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "scan", 0))
            .build()
            .unwrap();
        let error = engine()
            .flow_runner()
            .run(&flow, &FlowOptions::default(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no VFS storage"), "{error}");
    }

    #[tokio::test]
    async fn glob_stream_runs_real_nodes_through_vfs() {
        let storage = Arc::new(OpendalFileStorage::new_temp());
        for path in ["data/a.fastq", "data/b.fastq", "data/c.txt"] {
            storage
                .write_bytes(path, b"payload".to_vec())
                .await
                .unwrap();
        }
        let engine = DataEngine::builder()
            .register_opendal_fs(storage)
            .unwrap()
            .build();
        let graph = FlowGraph::builder()
            .source(Source::glob("reads", "data/*.fastq"))
            .process(Process::new(
                "read",
                "file_reference",
                serde_json::json!({"path":"{{item}}"}),
            ))
            .wire(Wire::direct("reads", 0, "read", 0))
            .build()
            .unwrap();
        let report = engine
            .flow_runner()
            .run(&graph, &FlowOptions::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(report.processes["read"].succeeded, 2);
    }
    #[tokio::test]
    async fn vfs_resume_reuses_without_history_and_invalidates_same_size_changes() {
        let storage = Arc::new(OpendalFileStorage::new_temp());
        storage
            .write_bytes("data/a.fastq", b"first".to_vec())
            .await
            .unwrap();
        let engine = DataEngine::builder()
            .register_opendal_fs(storage.clone())
            .unwrap()
            .build();
        let graph = FlowGraph::builder()
            .source(Source::glob("reads", "data/*.fastq"))
            .process(Process::new(
                "read",
                "file_reference",
                serde_json::json!({"path":"{{item}}"}),
            ))
            .wire(Wire::direct("reads", 0, "read", 0))
            .build()
            .unwrap();
        let runner = engine.flow_runner();
        let options = FlowOptions {
            resume: Some(true),
            ..Default::default()
        };
        let first = runner.run(&graph, &options, None).await.unwrap();
        assert_eq!(first.processes["read"].reused, 0);
        let json = serde_json::to_value(&first).unwrap();
        assert_eq!(
            json["nodes"][0]["output_files"][0]["path"],
            "vfs://data/a.fastq"
        );
        let second = runner.run(&graph, &options, None).await.unwrap();
        assert_eq!(second.processes["read"].reused, 1);
        storage
            .write_bytes("data/a.fastq", b"other".to_vec())
            .await
            .unwrap();
        let changed = runner.run(&graph, &options, None).await.unwrap();
        assert_eq!(changed.processes["read"].reused, 0);
        let reused = runner.run(&graph, &options, None).await.unwrap();
        assert_eq!(reused.processes["read"].reused, 1);
    }

    #[tokio::test]
    async fn reactive_resume_reuses_file_outputs_through_runner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.txt");
        std::fs::write(&path, "test").unwrap();
        let flow = FlowGraph::builder()
            .source(Source::items(
                "files",
                vec![serde_json::json!(path.to_string_lossy())],
            ))
            .process(Process::new(
                "file",
                "file_reference",
                serde_json::json!({"path": "{{item}}"}),
            ))
            .wire(Wire::direct("files", 0, "file", 0))
            .build()
            .unwrap();
        let engine = engine();
        let runner = engine.flow_runner();
        let opts = FlowOptions {
            resume: Some(true),
            ..Default::default()
        };
        let first = runner.run(&flow, &opts, None).await.unwrap();
        assert_eq!(first.processes["file"].reused, 0);
        let second = runner.run(&flow, &opts, None).await.unwrap();
        assert_eq!(second.processes["file"].reused, 1);
    }

    #[tokio::test]
    async fn isolated_agents_do_not_share_run_locks_or_cached_results() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.txt");
        std::fs::write(&path, "test").unwrap();
        let flow = FlowGraph::builder()
            .process(Process::new(
                "file",
                "file_reference",
                serde_json::json!({"path": path.to_string_lossy()}),
            ))
            .build()
            .unwrap();
        let template = engine();
        let first_agent = template.new_isolated_engine();
        let second_agent = template.new_isolated_engine();
        assert!(!Arc::ptr_eq(
            &first_agent.flow_cache,
            &second_agent.flow_cache
        ));
        assert!(!Arc::ptr_eq(&template.flow_cache, &first_agent.flow_cache));
        let first = first_agent.flow_runner();
        let second = second_agent.flow_runner();
        let options = FlowOptions {
            resume: Some(true),
            ..Default::default()
        };
        let report = first.run(&flow, &options, None).await.unwrap();
        assert_eq!(report.processes["file"].reused, 0);
        let report = first.run(&flow, &options, None).await.unwrap();
        assert_eq!(report.processes["file"].reused, 1);
        let lock = first_agent.flow_cache.lock().await;
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            second.run(&flow, &options, None),
        )
        .await
        .expect("another agent must not wait for this session's run lock")
        .unwrap();
        assert_eq!(
            report.processes["file"].reused, 0,
            "another agent must not reuse this session's result"
        );
        drop(lock);
    }

    struct TrackingExecutor {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        inner: Arc<dyn dag_core::TaskExecutor>,
    }
    #[async_trait::async_trait]
    impl dag_core::TaskExecutor for TrackingExecutor {
        fn name(&self) -> &'static str {
            "tracking-test"
        }
        fn execution_binding(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({"target":"configured"}))
        }
        async fn run(
            &self,
            execution: dag_core::TaskExecution,
        ) -> dag_core::dag::node_event::JobResult {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.run(execution).await
        }
    }

    #[tokio::test]
    async fn isolated_agent_runs_through_configured_executor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.txt");
        std::fs::write(&path, "test").unwrap();
        let flow = FlowGraph::builder()
            .process(Process::new(
                "file",
                "file_reference",
                serde_json::json!({"path":path.to_string_lossy()}),
            ))
            .build()
            .unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let executor: Arc<dyn dag_core::TaskExecutor> = Arc::new(TrackingExecutor {
            calls: Arc::clone(&calls),
            inner: dag_core::local_task_executor(),
        });
        let mut template = engine();
        template.set_task_executor(Arc::clone(&executor));
        let agent = template.new_isolated_engine();
        assert!(Arc::ptr_eq(&agent.dag.task_executor(), &executor));
        let report = agent
            .flow_runner()
            .run(&flow, &FlowOptions::default(), None)
            .await
            .unwrap();
        assert!(report.ok, "{:?}", report.instances);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            report.execution_binding,
            Some(serde_json::json!({"target":"configured"}))
        );
    }
}
