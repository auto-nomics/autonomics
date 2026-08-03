use std::sync::Arc;

use datafusion::{
    catalog::CatalogProvider,
    execution::{object_store::ObjectStoreUrl, runtime_env::RuntimeEnv},
    prelude::SessionContext,
};
use fs::OpendalFileStorage;

use crate::dag::{DAG, DagError, DagHistory, RunReport, SchedulerConfig};
use crate::error::{Error, Result};
use crate::node_registry::registry::NodeRegistry;
use crate::nodes::DagNode;
use datalake::Datalake;

pub use crate::nodes::{
    FileFormat, FileSinkNode, FileSourceNode, IcebergSinkNode, IcebergSourceNode, LdscHsqConfig,
    LdscHsqNode, LinearRegressionNode, SinkMode, SqlNode, WriteFormat,
};

/// `DataEngine` is the core object that implements the data analysis engine.
/// It orchestrates ingestion, transformation, and querying of datasets via a
/// [`DAG`] of nodes executed by an async scheduler.
pub struct DataEngine {
    ctx: SessionContext,
    /// Threaded through from the builder; retained for future engine-level
    /// object-store / catalog reconfiguration but not yet read here.
    datalake: Option<Arc<Datalake>>,

    /// The immutable engine ingredients, handed to the DAG scheduler on every
    /// `run` so each node `execute` can build a fresh isolated `SessionContext`
    /// via `NodeCtx::session()`. Owned here (not in the graph or the nodes) so
    /// mutable `SessionContext` state can never accumulate across runs.
    engine_ctx: crate::node_registry::registry::NodeCtx,
    dag: DAG,
    node_registry: NodeRegistry,
    config: SchedulerConfig,
    /// Optional DAG history store. When `Some`, every `run()` automatically
    /// commits a snapshot of the current DAG manifest + run report.
    history: Option<DagHistory>,
    /// The history ref that [`Self::run`] commits to (default `"main"`).
    /// Switch via [`Self::set_history_ref`] to isolate unrelated analysis
    /// tasks into independent snapshot lineages.
    history_ref: String,
}

impl DataEngine {
    fn new_from_parts(
        ctx: SessionContext,
        runtime_env: Arc<RuntimeEnv>,
        iceberg_catalog: Option<Arc<dyn CatalogProvider>>,
        datalake: Option<Arc<Datalake>>,
        opendal: Option<Arc<OpendalFileStorage>>,
    ) -> Self {
        let engine_ctx = crate::node_registry::registry::NodeCtx {
            runtime_env: runtime_env.clone(),
            iceberg_catalog: iceberg_catalog.clone(),
            datalake: datalake
                .clone()
                .unwrap_or_else(|| Arc::new(Datalake::default())),
            opendal: opendal.clone(),
        };
        let node_registry = NodeRegistry::new(
            runtime_env.clone(),
            iceberg_catalog.clone(),
            datalake
                .clone()
                .unwrap_or_else(|| Arc::new(Datalake::default())),
            opendal.clone(),
        );
        Self {
            ctx,
            datalake,
            engine_ctx,
            dag: DAG::default(),
            node_registry,
            config: SchedulerConfig::default(),
            history: None,
            history_ref: "main".to_string(),
        }
    }

    /// Returns the Iceberg catalog, if a datalake is registered.
    pub async fn catalog(&self) -> Option<Arc<iceberg_catalog_rest::RestCatalog>> {
        match &self.datalake {
            Some(dl) => dl.get_catalog().await.ok(),
            None => None,
        }
    }

    pub fn builder() -> DataEngineBuilder {
        DataEngineBuilder::default()
    }

    /// Returns the shared session context (object stores, catalogs, …).
    pub fn ctx(&self) -> SessionContext {
        self.ctx.clone()
    }

    /// Register a node under `id`(for test purposes).
    ///
    /// Prefer [`Self::add_node_from_registry`] for all standard node kinds —
    /// it validates the spec against the registered JSON Schema and builds
    /// the node automatically. Use this raw `add_node` only for custom test
    /// nodes or ad-hoc types not in the registry.
    ///
    /// ```ignore
    /// let meta = NodePorts::new();
    /// engine.add_node("x", MyNode::new(meta, ...))?;
    /// ```
    pub fn add_node<N: DagNode + 'static>(
        &mut self,
        id: impl Into<String>,
        node: N,
    ) -> Result<&mut Self> {
        self.dag.add_node(id.into(), Box::new(node))?;
        Ok(self)
    }

    /// Create a node by its registered `kind` and a JSON `spec`.
    ///
    /// The spec is validated against the kind's JSON Schema and deserialized
    /// by the corresponding factory. This is the primary path for node
    /// creation — all standard node kinds (source_file, source_iceberg, sql,
    /// sink_file, sink_iceberg, ldsc, linear_regression, mock, mr) are available.
    pub fn add_node_from_registry(
        &mut self,
        node_id: impl Into<String>,
        kind: &str,
        spec: serde_json::Value,
    ) -> Result<()> {
        let node = self.node_registry.build_node(kind, spec.clone())?;
        self.dag
            .add_node_with_spec(node_id.into(), node, kind.to_string(), spec)?;
        Ok(())
    }

    /// Query the JSON Schema of a registered node kind.
    pub fn get_node_spec(&self, kind: &str) -> Result<schemars::Schema> {
        Ok(self.node_registry.get_node_spec(kind)?)
    }

    /// Query the input/output port layout of a registered node kind.
    pub fn get_node_ports(&self, kind: &str) -> Result<crate::nodes::meta::NodePorts> {
        Ok(self.node_registry.get_node_ports(kind)?)
    }

    /// Query the documentation string of a registered node kind.
    pub fn get_node_doc(&self, kind: &str) -> Result<String> {
        Ok(self.node_registry.get_node_doc(kind)?)
    }

    /// List metadata of every registered node kind (kind + JSON Schema).
    pub fn list_nodes(&self) -> Vec<crate::node_registry::NodeInfo> {
        self.node_registry.list_nodes()
    }

    pub fn remove_node(&mut self, id: impl Into<String>) -> Result<&mut Self> {
        let id = id.into();
        self.dag.delete_node(&id)?;
        Ok(self)
    }

    /// Update an existing node's spec in-place.
    ///
    /// The node's `kind` is discovered from its current `node_type()` (which
    /// equals the registry kind via the `DagNode` trait contract). A new
    /// instance is built through the same factory with `spec`, then the DAG
    /// payload is atomically replaced — all edges are preserved and
    /// re-validated against the new node's port topology.
    ///
    /// Errors if `id` does not exist, if the kind has no registered factory,
    /// if the spec fails deserialization, or if any existing edge becomes
    /// incompatible with the new port layout.
    pub fn update_node(&mut self, id: impl Into<String>, spec: serde_json::Value) -> Result<()> {
        let id = id.into();
        let kind = self
            .dag
            .get_node(&id)
            .ok_or_else(|| Error::Dag(DagError::UnknownNode(id.clone())))?
            .kind()
            .to_string();
        let node = self.node_registry.build_node(&kind, spec.clone())?;
        self.dag.replace_node_with_spec(&id, node, kind, spec)?;
        Ok(())
    }

    pub fn view_dag(&self) -> Result<String> {
        Ok(self.dag.to_dot())
    }

    /// Clear all nodes, edges, and runtime state — start fresh.
    pub fn clear_dag(&mut self) -> Result<()> {
        self.dag.clear();
        Ok(())
    }

    // ── history / ref management ───────────────────────────────────────────

    /// Create a new analysis ref: clears the in-memory DAG and switches the
    /// engine's history ref to `name`. If a history store is attached, the
    /// ref is created in the database (the first `run()` will create a root
    /// snapshot).
    ///
    /// This is the replacement for the old `clear_dag` — instead of wiping
    /// state without trace, it starts a new independent lineage.
    pub async fn new_dag_ref(&mut self, name: &str) -> Result<()> {
        self.dag.clear();
        self.history_ref = name.to_string();
        Ok(())
    }

    /// Switch the engine's history ref to an existing ref name.
    /// Does **not** clear or modify the in-memory DAG.
    pub fn switch_dag_ref(&mut self, name: &str) -> Result<()> {
        self.history_ref = name.to_string();
        Ok(())
    }

    /// List all refs in the history store. Returns `Ok(vec![])` if no history
    /// is attached.
    pub async fn list_dag_refs(&self) -> Result<Vec<(String, String, bool)>> {
        match &self.history {
            Some(h) => Ok(h.list_refs().await?),
            None => Ok(vec![]),
        }
    }

    /// Show the snapshot lineage for a ref (default: current ref).
    /// Returns snapshots newest-first.
    pub async fn dag_log(
        &self,
        ref_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::dag::Snapshot>> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;
        let r = ref_name.unwrap_or(&self.history_ref);
        history.log(r, limit).await.map_err(|e| Error::Dag(e))
    }

    /// Load a historical snapshot's DAG into memory **without** moving the
    /// history ref.
    ///
    /// Clears the current DAG, rebuilds every node + edge from the snapshot's
    /// manifest. The ref stays where it is — subsequent `run()` calls commit
    /// with the current ref head as parent (just like a normal run).
    ///
    /// This is the equivalent of `git checkout <commit>`: you inspect and
    /// work from a historical state, but the branch pointer doesn't move.
    /// Short-hash prefixes are accepted.
    pub async fn checkout_dag(&mut self, snapshot_id: &str) -> Result<()> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;

        let snap = history
            .resolve_snapshot(snapshot_id)
            .await
            .map_err(|e| Error::Dag(e))?
            .ok_or_else(|| Error::Custom(format!("snapshot '{snapshot_id}' not found")))?;

        let manifest = snap
            .manifest()
            .map_err(|e| Error::Custom(format!("manifest deserialization: {e}")))?;

        self.rebuild_dag_from_manifest(&manifest)?;

        Ok(())
    }

    /// Create a new ref diverging from an arbitrary snapshot, switch the
    /// engine to it, and load that snapshot's DAG into memory.
    ///
    /// This is the equivalent of `git checkout -b <branch> <commit>`: you
    /// start a new independent lineage from a historical point without
    /// affecting the original ref. Short-hash prefixes are accepted.
    pub async fn branch_from_snapshot(
        &mut self,
        snapshot_id: &str,
        new_ref_name: &str,
    ) -> Result<()> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| Error::Custom("no history store attached".into()))?;

        // Create the new ref pointing at the resolved snapshot.
        history
            .branch_from_snapshot(new_ref_name, snapshot_id)
            .await
            .map_err(|e| Error::Dag(e))?;

        // Switch the engine to the new ref.
        self.history_ref = new_ref_name.to_string();

        // Load the snapshot's DAG into memory.
        let snap = history
            .resolve_snapshot(snapshot_id)
            .await
            .map_err(|e| Error::Dag(e))?
            .ok_or_else(|| Error::Custom(format!("snapshot '{snapshot_id}' not found")))?;

        let manifest = snap
            .manifest()
            .map_err(|e| Error::Custom(format!("manifest deserialization: {e}")))?;

        self.rebuild_dag_from_manifest(&manifest)?;

        Ok(())
    }

    /// Internal helper: clear the DAG and rebuild nodes + edges from a
    /// manifest.
    fn rebuild_dag_from_manifest(&mut self, manifest: &crate::dag::DagManifest) -> Result<()> {
        self.dag.clear();
        for entry in &manifest.nodes {
            let node = self
                .node_registry
                .build_node(&entry.kind, entry.spec.clone())?;
            self.dag.add_node_with_spec(
                entry.id.clone(),
                node,
                entry.kind.clone(),
                entry.spec.clone(),
            )?;
        }
        for edge in &manifest.edges {
            self.dag.add_edge(
                edge.from.clone(),
                edge.to.clone(),
                edge.from_port,
                edge.to_port,
            )?;
        }
        Ok(())
    }

    /// Add an edge from `from`'s default output port to `to`'s default input
    /// port (convenience form for single-port nodes).
    pub fn add_edge(
        &mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        from_port: u8,
        to_port: u8,
    ) -> Result<&mut Self> {
        self.dag.add_edge(from, to, from_port, to_port)?;
        Ok(self)
    }

    /// Remove the edge from `from`'s `from_port` to `to`'s `to_port`.
    ///
    /// Errors if either node does not exist or no matching edge is found.
    pub fn delete_edge(
        &mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        from_port: u8,
        to_port: u8,
    ) -> Result<()> {
        self.dag.delete_edge(from, to, from_port, to_port)?;
        Ok(())
    }

    // /// Add an edge connecting `from`'s `from_port` output port to `to`'s
    // /// `to_port` input port.
    // pub fn add_edge_port(
    //     &mut self,
    //     from: impl Into<String>,
    //     from_port: impl Into<String>,
    //     to: impl Into<String>,
    //     to_port: impl Into<String>,
    // ) -> Result<&mut Self> {
    //     self.dag.add_edge_port(from, from_port, to, to_port)?;
    //     Ok(self)
    // }
    //
    /// Replace the scheduler configuration (concurrency, retry, …).
    pub fn with_config(mut self, config: SchedulerConfig) -> Self {
        self.config = config;
        self
    }

    /// Attach a DAG history store. After this, every [`Self::run`] /
    /// [`Self::run_with_events`] will automatically commit a snapshot
    /// (manifest + run report) to the history database.
    pub fn with_history(mut self, history: DagHistory) -> Self {
        self.history = Some(history);
        self
    }

    /// Set the history ref name for this engine instance (builder style).
    ///
    /// Every [`Self::run`] will commit snapshots under this ref, forming an
    /// independent lineage. Use different ref names for unrelated analysis
    /// tasks to keep their histories separate.
    pub fn with_history_ref(mut self, ref_name: impl Into<String>) -> Self {
        self.history_ref = ref_name.into();
        self
    }

    /// Switch the history ref at runtime.
    ///
    /// Subsequent [`Self::run`] calls will commit to the new ref. If the ref
    /// does not exist yet, the first commit creates it (as a root snapshot).
    /// The in-memory DAG is **not** affected — call [`Self::clear_dag`] and
    /// rebuild if you want to start a fresh DAG topology.
    ///
    /// Returns the previous ref name.
    pub fn set_history_ref(&mut self, ref_name: impl Into<String>) -> String {
        let old = std::mem::replace(&mut self.history_ref, ref_name.into());
        old
    }

    /// The current history ref name that [`Self::run`] commits to.
    pub fn history_ref(&self) -> &str {
        &self.history_ref
    }

    /// Borrow the history store (if attached) for direct queries — e.g.
    /// `log`, `refs`, `get_snapshot`.
    pub fn history(&self) -> Option<&DagHistory> {
        self.history.as_ref()
    }

    /// Validate and run every node of the DAG.
    ///
    /// If a [`DagHistory`] is attached ([`Self::with_history`]), a snapshot
    /// manifest is captured *before* execution and committed with the
    /// [`RunReport`] *after* execution under the current history ref
    /// ([`Self::history_ref`]).
    pub async fn run(&mut self) -> Result<RunReport> {
        let manifest = self.dag.to_manifest();
        let report = self.dag.run(&self.config, &self.engine_ctx, None).await?;

        if let Some(history) = &self.history {
            let _ = history
                .commit(
                    &self.history_ref,
                    &manifest,
                    Some(&report),
                    "auto-snapshot after run",
                )
                .await;
        }
        Ok(report)
    }

    /// Like [`run`](Self::run) but also streams lightweight per-node events
    /// (status/progress/log/finished) to `event_sink` as the run progresses.
    /// The returned [`RunReport`] is identical to [`run`].
    pub async fn run_with_events(
        &mut self,
        event_sink: tokio::sync::mpsc::Sender<crate::dag::node_event::NodeEvent>,
    ) -> Result<RunReport> {
        let manifest = self.dag.to_manifest();
        let report = self
            .dag
            .run(&self.config, &self.engine_ctx, Some(event_sink))
            .await?;

        if let Some(history) = &self.history {
            let _ = history
                .commit(
                    &self.history_ref,
                    &manifest,
                    Some(&report),
                    "auto-snapshot after run",
                )
                .await;
        }
        Ok(report)
    }

    pub async fn get_output(
        &self,
        node_id: impl Into<String>,
    ) -> Option<crate::dag::graph::PortOutputs> {
        self.dag.output(node_id.into().as_ref())
    }
}

pub struct DataEngineBuilder {
    runtime_env: Arc<RuntimeEnv>,
    iceberg_catalog: Option<Arc<dyn CatalogProvider>>,
    datalake: Option<Arc<Datalake>>,
    opendal: Option<Arc<OpendalFileStorage>>,
}

impl Default for DataEngineBuilder {
    fn default() -> Self {
        let ctx = SessionContext::new();
        let runtime_env = ctx.runtime_env();
        Self {
            runtime_env,
            iceberg_catalog: None,
            datalake: None,
            opendal: None,
        }
    }
}

impl DataEngineBuilder {
    pub fn register_opendal_fs(self, file_session: Arc<OpendalFileStorage>) -> Result<Self> {
        let object_url = ObjectStoreUrl::parse("file://")
            .map_err(|e| Error::Custom(format!("cannot parse datafusion url: {e}")))?;
        self.runtime_env
            .register_object_store(object_url.as_ref(), file_session.clone());
        // Keep the handle so artifact-producing nodes (e.g. VizNode) can write
        // into the virtualized filesystem instead of the host filesystem.
        Ok(Self {
            opendal: Some(file_session),
            ..self
        })
    }

    pub async fn register_iceberg(mut self) -> Result<Self> {
        let datalake = Arc::new(Datalake::default());
        let provider = datalake.get_provider().await?;
        self.iceberg_catalog = Some(Arc::new(provider));
        self.datalake = Some(datalake);

        Ok(self)
    }

    pub fn build(self) -> DataEngine {
        // Keep a backward-compatible ctx for tests / ad-hoc table registration.
        let ctx = crate::node_registry::registry::new_isolated_ctx(
            self.runtime_env.clone(),
            self.iceberg_catalog.clone(),
        );
        DataEngine::new_from_parts(
            ctx,
            self.runtime_env,
            self.iceberg_catalog,
            self.datalake,
            self.opendal,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::DataEngine;
    use crate::dag::graph::PortOutputs;
    use crate::dag::{DagError, RuntimeStatus, SchedulerConfig};
    use crate::error::Error;
    use crate::nodes::{DagNode, NodeInput, NodePorts};
    use datafusion::common::HashMap;
    use datafusion::prelude::CsvReadOptions;
    use fs::OpendalFileStorage;

    fn datasets_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_datasets")
    }

    // ── Existing opendal+iceberg integration test ──────────────────────

    #[tokio::test]
    async fn test_dataengine_opendal_datafusion() {
        let file_session = Arc::new(OpendalFileStorage::new("/mnt/disk3/test"));
        let test_data_file = std::fs::read("test_datasets/Iris.csv").unwrap();
        let _write_res = file_session
            .op
            .write("/iris.csv", test_data_file)
            .await
            .unwrap();
        let engine = DataEngine::builder()
            .register_opendal_fs(file_session)
            .unwrap()
            .register_iceberg()
            .await
            .unwrap()
            .build();

        engine
            .ctx
            .register_csv("iris", "/iris.csv", CsvReadOptions::default())
            .await
            .unwrap();

        let df = engine.ctx.sql("SELECT * FROM iris LIMIT 5").await.unwrap();
        df.clone().show().await.unwrap();
        let length = df.clone().count().await.unwrap();
        assert_eq!(length, 5);
    }

    // ── DAG pipeline tests (migrated from tests/dag_pipeline.rs) ────────

    #[tokio::test]
    async fn insurance_pipeline_runs() {
        let mut engine = DataEngine::builder().build();
        let csv_path = datasets_dir().join("insurance.csv");
        let out_path = "/tmp/dag_insurance_out.csv";
        let _ = std::fs::remove_file(out_path);

        engine
            .add_node_from_registry(
                "load",
                "source_file",
                serde_json::json!({"path": csv_path.to_str().unwrap()}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "agg",
                "sql",
                // agg's single input (port 0) is registered as "port_0".
                serde_json::json!({"sql_query": "SELECT region, CAST(AVG(charges) AS BIGINT) AS avg_chg \
                 FROM port_0 GROUP BY region"}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "out",
                "sink_file",
                serde_json::json!({"path": out_path, "format": "csv"}),
            )
            .unwrap();
        // Default edges: each node has a single relevant port, resolved automatically.
        engine.add_edge("load", "agg", 0, 0).unwrap();
        engine.add_edge("agg", "out", 0, 0).unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok, "all nodes should succeed: {:?}", report.statuses);
        for n in ["load", "agg", "out"] {
            assert_eq!(report.status(n), Some(RuntimeStatus::Success), "{n}");
        }

        // 4 regions in the insurance dataset → 4 data rows + 1 header.
        let lines = std::fs::read_to_string(out_path).unwrap();
        assert_eq!(lines.lines().count(), 5, "expected 4 region rows + header");
    }

    #[tokio::test]
    async fn fanout_branch() {
        // One source → two independent SqlNodes (fan-out from one output port).
        let mut engine = DataEngine::builder().build();
        let iris = datasets_dir().join("Iris.csv");

        engine
            .add_node_from_registry(
                "load",
                "source_file",
                serde_json::json!({"path": iris.to_str().unwrap()}),
            )
            .unwrap();
        // Note: DataFusion lowercases unquoted identifiers, so quote the
        // mixed-case column name "Species".
        engine
            .add_node_from_registry(
                "setosa",
                "sql",
                serde_json::json!({"sql_query": r#"SELECT * FROM port_0 WHERE "Species" = 'Iris-setosa'"#}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "virginica",
                "sql",
                serde_json::json!({"sql_query": r#"SELECT * FROM port_0 WHERE "Species" = 'Iris-virginica'"#}),
            )
            .unwrap();
        engine.add_edge("load", "setosa", 0, 0).unwrap();
        engine.add_edge("load", "virginica", 0, 0).unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(
            report.ok,
            "statuses: {:?}; errors: {:?}",
            report.statuses, report.errors
        );
        for n in ["load", "setosa", "virginica"] {
            assert_eq!(report.status(n), Some(RuntimeStatus::Success), "{n}");
        }
    }

    #[tokio::test]
    async fn fanout_concurrent() {
        // Fan-out under concurrency=2 — verifies no SessionContext registration
        // collision between concurrent consumers of the same source port.
        let mut engine = DataEngine::builder().build().with_config(SchedulerConfig {
            max_concurrency: 2,
            ..SchedulerConfig::default()
        });
        let iris = datasets_dir().join("Iris.csv");

        engine
            .add_node_from_registry(
                "load",
                "source_file",
                serde_json::json!({"path": iris.to_str().unwrap()}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "a",
                "sql",
                serde_json::json!({"sql_query": r#"SELECT COUNT(*) AS cnt FROM port_0 WHERE "Species" = 'Iris-setosa'"#}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "b",
                "sql",
                serde_json::json!({"sql_query": r#"SELECT COUNT(*) AS cnt FROM port_0 WHERE "Species" = 'Iris-virginica'"#}),
            )
            .unwrap();
        engine.add_edge("load", "a", 0, 0).unwrap();
        engine.add_edge("load", "b", 0, 0).unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(
            report.ok,
            "statuses: {:?}; errors: {:?}",
            report.statuses, report.errors
        );
        for n in ["load", "a", "b"] {
            assert_eq!(report.status(n), Some(RuntimeStatus::Success), "{n}");
        }
    }

    /// A multi-input join: two sources feed a single SqlNode's `left`/`right`
    /// input ports, exercising named ports and `add_edge_port`.
    #[tokio::test]
    async fn join_named_ports() {
        let mut engine = DataEngine::builder().build();
        let iris = datasets_dir().join("Iris.csv");

        engine
            .add_node_from_registry(
                "src_a",
                "source_file",
                serde_json::json!({"path": iris.to_str().unwrap()}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "src_b",
                "source_file",
                serde_json::json!({"path": iris.to_str().unwrap()}),
            )
            .unwrap();

        // A join node with two named input ports. Inputs are registered as
        // "port_0" and "port_1" by the SqlNode (one table per upstream port).
        engine
            .add_node(
                "join",
                super::SqlNode::from_ports(
                    NodePorts::new()
                        .add_input_port(None)
                        .add_input_port(None)
                        .add_output_port(None),
                    r#"SELECT COUNT(*) AS cnt FROM port_0"#.to_string(),
                ),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "out",
                "sink_file",
                serde_json::json!({"path": "/tmp/dag_join_out.csv", "format": "csv"}),
            )
            .unwrap();
        engine
            .add_edge("src_a", "join", 0, 0)
            .unwrap()
            .add_edge("src_b", "join", 0, 1)
            .unwrap()
            .add_edge("join", "out", 0, 0)
            .unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(
            report.ok,
            "statuses: {:?}; errors: {:?}",
            report.statuses, report.errors
        );
        for n in ["src_a", "src_b", "join", "out"] {
            assert_eq!(report.status(n), Some(RuntimeStatus::Success), "{n}");
        }
    }

    #[tokio::test]
    async fn bio_source_reads_vcf() {
        // SourceNode auto-detects the VCF.gz format from the extension.
        let mut engine = DataEngine::builder().build();
        let vcf = datasets_dir().join("sample.vcf.gz");

        engine
            .add_node_from_registry(
                "vcf",
                "source_file",
                serde_json::json!({"path": vcf.to_str().unwrap()}),
            )
            .unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);
        assert_eq!(report.status("vcf"), Some(RuntimeStatus::Success));
    }

    /// Regression: with default `compute_row_counts = false`, running a
    /// source-only VCF DAG must NOT eagerly `count()` the dataset. The
    /// `output_rows` field of the report should stay `None`, which proves the
    /// eager COUNT(*) scan never ran.
    ///
    /// Before the fix, `build_node_reports` unconditionally called `df.count()`
    /// on every output, forcing a full decompression + parse of the VCF — the
    /// worst-case behavior for "source node, no downstream consumer".
    #[tokio::test]
    async fn vcf_source_skips_count_by_default() {
        let mut engine = DataEngine::builder().build();
        let vcf = datasets_dir().join("sample.vcf.gz");

        engine
            .add_node_from_registry(
                "vcf",
                "source_file",
                serde_json::json!({"path": vcf.to_str().unwrap()}),
            )
            .unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);
        assert_eq!(report.status("vcf"), Some(RuntimeStatus::Success));

        let node = report
            .nodes
            .iter()
            .find(|n| n.id == "vcf")
            .expect("source node should be in the report");

        // The whole point of the fix: default config must NOT compute rows.
        assert!(
            node.output_rows.is_none(),
            "default config should skip row counting; got {:?}",
            node.output_rows
        );
    }

    /// Opt-in path: when the caller explicitly sets
    /// `compute_row_counts = true`, the report must contain a row count for
    /// every successful source node — proving that the parallel `join_all`
    /// path works end-to-end.
    #[tokio::test]
    async fn vcf_source_computes_rows_when_opted_in() {
        let mut engine = DataEngine::builder().build().with_config(SchedulerConfig {
            compute_row_counts: true,
            ..SchedulerConfig::default()
        });
        let vcf = datasets_dir().join("sample.vcf.gz");

        engine
            .add_node_from_registry(
                "vcf",
                "source_file",
                serde_json::json!({"path": vcf.to_str().unwrap()}),
            )
            .unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);

        let node = report
            .nodes
            .iter()
            .find(|n| n.id == "vcf")
            .expect("source node should be in the report");

        let rows = node
            .output_rows
            .expect("opted-in run must populate row counts");
        assert!(
            rows > 0,
            "sample VCF should report a positive row count, got {rows}"
        );

        // Output schema should still be populated regardless of the count
        // setting — `schema()` inspects the LogicalPlan only, not data.
        assert!(
            node.output_schema.is_some(),
            "schema should be visible from the LogicalPlan without execution"
        );
    }

    #[tokio::test]
    #[ignore = "cycle will be detected and rejected earlier in edge creation"]
    async fn cycle_is_rejected() {
        let mut engine = DataEngine::builder().build();
        engine
            .add_node_from_registry("a", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        engine
            .add_node_from_registry("b", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        engine
            .add_edge("a", "b", 0, 0)
            .unwrap()
            .add_edge("b", "a", 0, 0)
            .unwrap();

        let err = engine.run().await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("cycle"), "{msg}");
    }

    #[tokio::test]
    async fn disconnected_input_port_rejected() {
        // A fixed-input node with no incoming edge → validation must reject the
        // dangling input port. LinearRegressionNode declares exactly one required
        // (fixed) input port. Variadic nodes like SqlNode are exempt: they
        // declare no ports and accept any number of inputs, so this check only
        // applies to fixed-input nodes. Validation runs before execution, so the
        // placeholder column names never get used.
        let mut engine = DataEngine::builder().build();
        engine
            .add_node_from_registry(
                "lr",
                "linear_regression",
                serde_json::json!({"x_columns": ["x"], "y_column": "y", "intercept": true}),
            )
            .unwrap();

        let err = engine.run().await.unwrap_err();
        assert!(
            matches!(err, Error::Dag(DagError::PortDisconnected { ref node, .. }) if node == "lr"),
            "expected PortDisconnected, got {err:?}"
        );
    }

    #[tokio::test]
    async fn overconnected_input_port_rejected() {
        // Two edges into one input port (strict 1:1 violation).
        let mut engine = DataEngine::builder().build();
        let iris = datasets_dir().join("Iris.csv");
        engine
            .add_node_from_registry(
                "s1",
                "source_file",
                serde_json::json!({"path": iris.to_str().unwrap()}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "s2",
                "source_file",
                serde_json::json!({"path": iris.to_str().unwrap()}),
            )
            .unwrap();
        engine
            .add_node_from_registry("c", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        engine.add_edge("s1", "c", 0, 0).unwrap();
        engine.add_edge("s2", "c", 0, 0).unwrap();

        let err = engine.run().await.unwrap_err();
        assert!(
            matches!(err, Error::Dag(DagError::PortOverconnected { ref node, .. }) if node == "c"),
            "expected PortOverconnected, got {err:?}"
        );
    }

    #[tokio::test]
    async fn unknown_port_rejected() {
        let mut engine = DataEngine::builder().build();
        engine
            .add_node_from_registry("a", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        engine
            .add_node_from_registry("b", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        // "a" has no output port named "nope".
        engine.add_edge("a", "b", 99, 0).unwrap();

        let err = engine.run().await.unwrap_err();
        assert!(
            matches!(err, Error::Dag(DagError::PortNotFound { ref node, direction: "output", .. }) if node == "a"),
            "expected PortNotFound(output), got {err:?}"
        );
    }

    /// A node that always fails — for the cascade test. Source-like (no inputs).
    #[derive(Clone)]
    struct BoomNode(NodePorts);
    #[async_trait::async_trait]
    impl DagNode for BoomNode {
        fn ports(&self) -> &NodePorts {
            &self.0
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }
        fn kind(&self) -> &'static str {
            "boom"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            _ctx: &crate::node_registry::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &crate::dag::node_event::NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            Err(DagError::Schedule("kaboom".into()))
        }
    }

    #[tokio::test]
    async fn failure_cascades() {
        let mut engine = DataEngine::builder().build();
        // Source-like boom node (no input ports) so it passes port validation.
        let boom_ports = NodePorts::new().add_output_port(None);
        engine.add_node("boom", BoomNode(boom_ports)).unwrap();
        engine
            .add_node_from_registry("child", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        engine.add_edge("boom", "child", 0, 0).unwrap();

        let report = engine.run().await.expect("run completes even on failure");
        assert!(!report.ok, "run should report failure");
        assert_eq!(report.status("boom"), Some(RuntimeStatus::Failed));
        assert_eq!(
            report.status("child"),
            Some(RuntimeStatus::Skipped),
            "descendant of a failed node must be skipped"
        );
    }

    /// A node that sleeps — for the parallelism test. No inputs, no real output.
    #[derive(Clone)]
    struct SleepNode(NodePorts);
    #[async_trait::async_trait]
    impl DagNode for SleepNode {
        fn ports(&self) -> &NodePorts {
            &self.0
        }
        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new((*self).clone())
        }
        fn kind(&self) -> &'static str {
            "sleep"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &mut self,
            _ctx: &crate::node_registry::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &crate::dag::node_event::NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(HashMap::new())
        }
    }

    #[tokio::test]
    async fn scheduler_runs_in_parallel() {
        use std::time::{Duration, Instant};

        // 4 independent sleep nodes, concurrency 4 → ~100ms; concurrency 1 → ~400ms.
        for &concurrency in &[4usize, 1] {
            let mut engine = DataEngine::builder().build().with_config(SchedulerConfig {
                max_concurrency: concurrency,
                ..SchedulerConfig::default()
            });
            for i in 0..4 {
                let id = format!("s{i}");
                // No input ports: standalone nodes pass port validation.
                let meta = NodePorts::new().add_output_port(None);
                engine.add_node(id, SleepNode(meta)).unwrap();
            }
            let start = Instant::now();
            let report = engine.run().await.unwrap();
            let elapsed = start.elapsed();
            assert!(report.ok);
            assert_eq!(report.status("s0"), Some(RuntimeStatus::Success));
            if concurrency == 4 {
                assert!(
                    elapsed < Duration::from_millis(350),
                    "parallel run took too long: {elapsed:?}"
                );
            } else {
                assert!(
                    elapsed >= Duration::from_millis(350),
                    "serial run was too fast: {elapsed:?}"
                );
            }
        }
    }

    // ── Registry-based node creation tests ──────────────────────────

    /// All node kinds must be discoverable via list_nodes.
    #[tokio::test]
    async fn list_nodes_returns_all_registered_kinds() {
        let engine = DataEngine::builder().build();
        let nodes = engine.list_nodes();
        let kinds: Vec<&str> = nodes.iter().map(|n| n.kind.as_str()).collect();
        for expected in [
            "sql",
            "source_file",
            "source_iceberg",
            "sink_file",
            "sink_iceberg",
            "ldsc",
            "linear_regression",
            "echo",
            "test_source",
            "two_sample_mr",
        ] {
            assert!(
                kinds.contains(&expected),
                "missing kind '{expected}'; got {kinds:?}"
            );
        }
    }

    /// Each registered kind must have a non-null JSON Schema.
    #[tokio::test]
    async fn get_node_spec_returns_schema_for_every_kind() {
        let engine = DataEngine::builder().build();
        for kind in [
            "sql",
            "source_file",
            "source_iceberg",
            "sink_file",
            "sink_iceberg",
            "ldsc",
            "linear_regression",
            "echo",
            "test_source",
            "two_sample_mr",
        ] {
            let schema = engine
                .get_node_spec(kind)
                .unwrap_or_else(|e| panic!("get_node_spec({kind}) failed: {e}"));
            let raw = serde_json::to_value(&schema).unwrap();
            assert!(
                raw.is_object(),
                "schema for {kind} should be a JSON object; got {raw}"
            );
        }
    }

    /// Unknown kind → FactoryNotFound
    #[tokio::test]
    async fn add_node_via_registry_unknown_kind() {
        let mut engine = DataEngine::builder().build();
        let err = engine
            .add_node_from_registry("n", "nonexistent_kind_42", serde_json::json!({}))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("nonexistent_kind_42"),
            "error should mention the kind; got: {msg}"
        );
    }

    /// Malformed spec → SpecDeserialize
    #[tokio::test]
    async fn add_node_via_registry_bad_spec() {
        let mut engine = DataEngine::builder().build();

        // sql requires { sql_query: String }; passing an empty object fails.
        let err = engine
            .add_node_from_registry("n", "sql", serde_json::json!({}))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("sql_query") || msg.contains("missing field"),
            "sql with empty spec should fail deserialization; got: {msg}"
        );

        // source_file requires { path }; omitting it fails.
        let err = engine
            .add_node_from_registry("n", "source_file", serde_json::json!({"type": "ftp"}))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path") || msg.contains("missing field"),
            "source_file without a `path` should fail deserialization; got: {msg}"
        );
    }

    // ── Per-kind smoke tests: create via registry + run a minimal DAG ──

    /// sql node: create via registry, wire to a source, run.
    #[tokio::test]
    async fn registry_sql_node_runs() {
        let mut engine = DataEngine::builder().build();
        let csv = datasets_dir().join("insurance.csv");

        engine
            .add_node_from_registry(
                "src",
                "source_file",
                serde_json::json!({"path": csv.to_str().unwrap()}),
            )
            .unwrap();

        engine
            .add_node_from_registry(
                "agg",
                "sql",
                serde_json::json!({"sql_query": "SELECT region, CAST(AVG(charges) AS BIGINT) AS avg_chg FROM port_0 GROUP BY region"}),
            )
            .unwrap();

        engine.add_edge("src", "agg", 0, 0).unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);
        assert_eq!(report.status("agg"), Some(RuntimeStatus::Success));
    }

    /// source node (file): create via registry, run.
    #[tokio::test]
    async fn registry_source_node_file_runs() {
        let mut engine = DataEngine::builder().build();
        let csv = datasets_dir().join("insurance.csv");

        engine
            .add_node_from_registry(
                "src",
                "source_file",
                serde_json::json!({"path": csv.to_str().unwrap()}),
            )
            .unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);
        assert_eq!(report.status("src"), Some(RuntimeStatus::Success));
    }

    /// sink node (file): create via registry, wire source→sink, run.
    #[tokio::test]
    async fn registry_sink_node_file_runs() {
        let mut engine = DataEngine::builder().build();
        let csv = datasets_dir().join("insurance.csv");
        let out = "/tmp/dag_registry_sink_test.csv";
        let _ = std::fs::remove_file(out);

        engine
            .add_node_from_registry(
                "src",
                "source_file",
                serde_json::json!({"path": csv.to_str().unwrap()}),
            )
            .unwrap();

        engine
            .add_node_from_registry(
                "out",
                "sink_file",
                serde_json::json!({"path": out, "format": "csv"}),
            )
            .unwrap();

        engine.add_edge("src", "out", 0, 0).unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);
        assert_eq!(report.status("out"), Some(RuntimeStatus::Success));
        assert!(std::path::Path::new(out).exists());
    }

    /// linear_regression node: create via registry, succeeds execution.
    #[tokio::test]
    async fn registry_linear_regression_node_runs() {
        let mut engine = DataEngine::builder().build();
        let csv = datasets_dir().join("insurance.csv");

        engine
            .add_node_from_registry(
                "src",
                "source_file",
                serde_json::json!({"path": csv.to_str().unwrap()}),
            )
            .unwrap();

        engine
            .add_node_from_registry(
                "lr",
                "linear_regression",
                serde_json::json!({"x_columns": ["age"], "y_column": "charges"}),
            )
            .unwrap();

        engine.add_edge("src", "lr", 0, 0).unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);
        assert_eq!(report.status("lr"), Some(RuntimeStatus::Success));
    }

    /// linear_regression with intercept defaulting to true.
    #[tokio::test]
    async fn registry_linear_regression_default_intercept() {
        let mut engine = DataEngine::builder().build();
        let csv = datasets_dir().join("insurance.csv");

        engine
            .add_node_from_registry(
                "src",
                "source_file",
                serde_json::json!({"path": csv.to_str().unwrap()}),
            )
            .unwrap();

        // omit intercept → defaults to true.
        engine
            .add_node_from_registry(
                "lr",
                "linear_regression",
                serde_json::json!({"x_columns": ["bmi"], "y_column": "charges"}),
            )
            .unwrap();

        engine.add_edge("src", "lr", 0, 0).unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);
        assert_eq!(report.status("lr"), Some(RuntimeStatus::Success));
    }

    /// test_source node: creates with spec, returns Iris dataset.
    #[tokio::test]
    async fn registry_test_source_node_runs() {
        let mut engine = DataEngine::builder().build();

        engine
            .add_node_from_registry("m", "test_source", serde_json::json!({"dataset": "iris"}))
            .unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok);
        assert_eq!(report.status("m"), Some(RuntimeStatus::Success));
    }

    /// Multiple registry-created nodes wired together end-to-end.
    #[tokio::test]
    async fn registry_full_pipeline() {
        let mut engine = DataEngine::builder().build();
        let csv = datasets_dir().join("insurance.csv");
        let out = "/tmp/dag_registry_full_pipeline.csv";
        let _ = std::fs::remove_file(out);

        engine
            .add_node_from_registry(
                "src",
                "source_file",
                serde_json::json!({"path": csv.to_str().unwrap()}),
            )
            .unwrap();

        engine
            .add_node_from_registry(
                "filter",
                "sql",
                serde_json::json!({"sql_query": "SELECT * FROM port_0 WHERE age > 30"}),
            )
            .unwrap();

        engine
            .add_node_from_registry(
                "lr",
                "linear_regression",
                serde_json::json!({"x_columns": ["age", "bmi"], "y_column": "charges"}),
            )
            .unwrap();

        engine
            .add_node_from_registry(
                "out",
                "sink_file",
                serde_json::json!({"path": out, "format": "csv"}),
            )
            .unwrap();

        // src → filter → lr → out
        engine.add_edge("src", "filter", 0, 0).unwrap();
        engine.add_edge("filter", "lr", 0, 0).unwrap();
        engine.add_edge("lr", "out", 0, 0).unwrap();

        let report = engine.run().await.expect("run should succeed");
        assert!(report.ok, "full pipeline failed: {:?}", report.statuses);
        for n in ["src", "filter", "lr", "out"] {
            assert_eq!(
                report.status(n),
                Some(RuntimeStatus::Success),
                "node {n} should succeed"
            );
        }
        assert!(std::path::Path::new(out).exists());
    }

    // ── update_node tests ────────────────────────────────────────────

    /// update_node can change a sql node's query and the DAG still runs
    /// correctly with the downstream edges preserved.
    #[tokio::test]
    async fn update_node_changes_sql_and_runs() {
        let mut engine = DataEngine::builder().build();
        let csv = datasets_dir().join("insurance.csv");
        let out = "/tmp/dag_update_sql_test.csv";
        let _ = std::fs::remove_file(out);

        engine
            .add_node_from_registry(
                "src",
                "source_file",
                serde_json::json!({"path": csv.to_str().unwrap()}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "agg",
                "sql",
                serde_json::json!({"sql_query": "SELECT region, CAST(AVG(charges) AS BIGINT) AS avg_chg FROM port_0 GROUP BY region"}),
            )
            .unwrap();
        engine
            .add_node_from_registry(
                "out",
                "sink_file",
                serde_json::json!({"path": out, "format": "csv"}),
            )
            .unwrap();
        engine.add_edge("src", "agg", 0, 0).unwrap();
        engine.add_edge("agg", "out", 0, 0).unwrap();

        // First run with original SQL.
        let report1 = engine.run().await.expect("run should succeed");
        assert!(report1.ok);

        // Update agg to a different query.
        engine
            .update_node(
                "agg",
                serde_json::json!({"sql_query": "SELECT COUNT(*) AS cnt FROM port_0"}),
            )
            .expect("update_node should succeed");

        // Second run with updated SQL — edges preserved.
        let report2 = engine.run().await.expect("run after update should succeed");
        assert!(report2.ok);
        for n in ["src", "agg", "out"] {
            assert_eq!(report2.status(n), Some(RuntimeStatus::Success), "{n}");
        }
    }

    /// update_node rejects a non-existent node id.
    #[tokio::test]
    async fn update_node_unknown_id_rejected() {
        let mut engine = DataEngine::builder().build();
        let err = engine
            .update_node("ghost", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap_err();
        assert!(err.to_string().contains("unknown node"), "{err}");
    }

    /// update_node rejects a malformed spec.
    #[tokio::test]
    async fn update_node_bad_spec_rejected() {
        let mut engine = DataEngine::builder().build();
        engine
            .add_node_from_registry("x", "sql", serde_json::json!({"sql_query": "SELECT 1"}))
            .unwrap();
        // Empty spec missing sql_query.
        let err = engine.update_node("x", serde_json::json!({})).unwrap_err();
        assert!(
            err.to_string().contains("sql_query") || err.to_string().contains("missing field"),
            "expected deserialization error, got {err}"
        );
    }
}
