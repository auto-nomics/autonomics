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
    FlowGraph, FlowRunConfig, FlowRunReport, FlowScheduler, NodeBuilder, SourceResolution,
    build_plan, resolve_sources,
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
}

/// A ready-to-run flow scheduler bound to one engine's ingredients. Cheap to
/// build; holds only `Arc` clones and one closure.
pub struct FlowRunner {
    scheduler: FlowScheduler,
    storage: Option<Arc<OpendalFileStorage>>,
}

impl DataEngine {
    /// Assemble a flow runner from this engine's registry, execution context
    /// and executor stack. The authored flow graph is supplied per run, so
    /// the engine's own (legacy) DAG state is never consulted or mutated.
    pub fn flow_runner(&self) -> FlowRunner {
        let registry = Arc::clone(&self.node_registry);
        let node_builder: NodeBuilder = Arc::new(move |kind, spec| {
            registry
                .build_node(kind, spec)
                .map_err(|error| {
                    dag_engine::Error::Execution(format!(
                        "cannot build node `{kind}`: {error}"
                    ))
                })
        });
        FlowRunner {
            scheduler: FlowScheduler::new(
                self.dag.task_executor(),
                node_builder,
                Arc::new(self.engine_ctx.clone()),
            ),
            storage: self.engine_ctx.opendal.clone(),
        }
    }
}

impl FlowRunner {
    /// Resolve sources through the VFS, build the plan, and execute it.
    ///
    /// Failure semantics follow the flow engine: source/plan failures (no
    /// storage configured for a glob, mismatched item sets, broken
    /// templates, unknown kinds) abort before anything dispatches.
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
        let resolution: SourceResolution =
            resolve_sources(flow, self.storage.as_deref()).await?;
        let plan = build_plan(flow, &resolution)?;
        let config = FlowRunConfig {
            max_concurrency: options
                .max_concurrency
                .unwrap_or_else(|| FlowRunConfig::default().max_concurrency),
        };
        self.scheduler.run(&plan, &config, cancel).await
    }
}

/// Exposed for the actor: build the runner under the engine lock.
pub(crate) fn runner_for(engine: &DataEngine) -> FlowRunner {
    engine.flow_runner()
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
            .process(Process::singleton("hello", "echo", serde_json::json!({})))
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
    async fn fanout_over_items_executes_one_instance_per_item() {
        let flow = FlowGraph::builder()
            .process(Process::fanout(
                "batch",
                "echo",
                serde_json::json!({}),
                Source::items(vec![
                    serde_json::json!(1),
                    serde_json::json!(2),
                    serde_json::json!(3),
                ]),
                "item",
            ))
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
            .process(Process::singleton(
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
            .process(Process::fanout(
                "scan",
                "echo",
                serde_json::json!({}),
                Source::glob("data/*.fastq"),
                "sample",
            ))
            .build()
            .unwrap();
        let error = engine()
            .flow_runner()
            .run(&flow, &FlowOptions::default(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no VFS storage"), "{error}");
    }
}
