use std::sync::Arc;

use datafusion::{
    common::HashMap,
    execution::{runtime_env::RuntimeEnv, session_state::SessionStateBuilder},
    prelude::{SessionConfig, SessionContext},
};

use serde::Serialize;

use super::error::{Error, Result};
use crate::dag::DagNode;
use crate::node::{BundleRegistry, DataBundle, DataBundleBinding, NodePorts};
use std::collections::HashMap as BoundDataBundles;

/// Build a fresh, isolated [`SessionContext`].
///
/// Each call creates a **new** `CatalogList` (so `register_table("port_0", ...)`
/// never collides with another node's registration), while sharing the
/// engine-wide [`RuntimeEnv`] so object stores remain reachable.
pub fn new_isolated_ctx(runtime_env: Arc<RuntimeEnv>) -> SessionContext {
    let state = SessionStateBuilder::new()
        .with_default_features()
        .with_config(SessionConfig::new().with_information_schema(true))
        .with_runtime_env(runtime_env)
        .build();
    SessionContext::new_with_state(state)
}

pub trait NodeFactory: Send + Sync {
    fn kind(&self) -> &'static str;
    fn desc(&self) -> &'static str;
    fn doc(&self) -> &'static str;
    /// Whether this kind is retained only for legacy workflows.
    ///
    /// Deprecated kinds remain buildable so existing DAGs keep working, but
    /// clients should surface this flag in node listings and prefer a
    /// replacement kind.
    fn deprecated(&self) -> bool {
        false
    }
    fn spec_schema(&self) -> schemars::Schema;
    /// The static port layout for this node kind — the input/output ports
    /// every instance of this kind will declare. Queryable without
    /// instantiating a node (mirrors [`NodeFactory::spec_schema`]).
    fn ports(&self) -> NodePorts;
    /// Resolve the actual port layout for a concrete spec. Defaults to the
    /// static layout for kinds whose ports do not depend on configuration.
    fn ports_for_spec(&self, _spec: serde_json::Value) -> Result<NodePorts> {
        Ok(self.ports())
    }
    /// Data bundles required by every node of this kind.
    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        Vec::new()
    }
    /// Data bundles required by one node instance. Kinds whose dependency
    /// depends on the spec override this method.
    fn data_bundles_for_spec(&self, _spec: serde_json::Value) -> Result<Vec<DataBundleBinding>> {
        Ok(self.data_bundles())
    }
    fn build(&self, spec: serde_json::Value, node_ctx: NodeCtx) -> Result<Box<dyn DagNode>>;
}

/// Ingredients for building an isolated [`SessionContext`] per node execution.
///
/// Instead of sharing a single `SessionContext` (which causes CatalogList
/// collisions on `register_table`), nodes receive the `RuntimeEnv` and
/// construct their own context at execution time via [`new_isolated_ctx`].
#[derive(Clone)]
pub struct NodeCtx {
    /// Shared object-store registry — all nodes reference the same
    /// `RuntimeEnv` so file:// / s3:// stores registered by the engine
    /// builder are reachable.
    pub runtime_env: Arc<RuntimeEnv>,
    /// The opendal-backed file storage registered with the engine, used by
    /// artifact-producing nodes to write outputs into the
    /// engine's virtualized filesystem rather than the host filesystem.
    /// `None` when no opendal fs was registered.
    pub opendal: Option<Arc<vfs::OpendalFileStorage>>,
    /// Engine-wide mapping from stable bundle identifiers to logical bundles.
    pub bundle_registry: Arc<BundleRegistry>,
    /// Bindings resolved for the node currently being built. Empty on the
    /// scheduler-wide context.
    pub bound_data_bundles: BoundDataBundles<String, DataBundle>,
    /// **Cross-agent global concurrency limiter.**
    ///
    /// When `Some`, every node execution acquires a permit from this semaphore
    /// *before* the per-run semaphore. This caps the total number of
    /// concurrently executing nodes across **all** agents / DAG runs, so that
    /// CPU-bound node work (faer, ML algorithms, …) cannot saturate every
    /// tokio worker thread and starve agent message processing.
    ///
    /// Sized to `num_cpus.saturating_sub(2).max(1)` — leaving ≥ 2 worker
    /// threads for `SessionServer` actors and tool execution. `None` in
    /// tests and legacy code paths (unlimited).
    pub global_sem: Option<Arc<tokio::sync::Semaphore>>,
}

impl NodeCtx {
    /// Convenience constructor from the two engine-level ingredients.
    ///
    pub fn new(
        runtime_env: Arc<RuntimeEnv>,
        opendal: Option<Arc<vfs::OpendalFileStorage>>,
    ) -> Self {
        Self {
            runtime_env,
            opendal,
            bundle_registry: Arc::new(BundleRegistry::new()),
            bound_data_bundles: BoundDataBundles::new(),
            global_sem: None,
        }
    }

    /// Attach the engine-wide bundle registry.
    pub fn with_bundle_registry(mut self, registry: Arc<BundleRegistry>) -> Self {
        self.bundle_registry = registry;
        self
    }

    /// Resolve factory-declared bindings into a node-local context.
    fn with_bound_data_bundles(
        mut self,
        kind: &str,
        bindings: Vec<DataBundleBinding>,
    ) -> Result<Self> {
        for requirement in bindings {
            let bundle = self
                .bundle_registry
                .get(&requirement.bundle_id)
                .ok_or_else(|| Error::DataBundleNotFound {
                    kind: kind.to_string(),
                    binding: requirement.binding.clone(),
                    bundle_id: requirement.bundle_id.clone(),
                })?
                .clone();
            if self
                .bound_data_bundles
                .insert(requirement.binding.clone(), bundle)
                .is_some()
            {
                return Err(Error::Unknown(format!(
                    "node kind `{kind}` declares data bundle binding `{}` more than once",
                    requirement.binding
                )));
            }
        }
        Ok(self)
    }

    /// Get a bundle resolved for the node currently being built.
    pub fn bound_data_bundle(&self, binding: &str) -> Result<&DataBundle> {
        self.bound_data_bundles.get(binding).ok_or_else(|| {
            Error::Unknown(format!("data bundle binding `{binding}` is not declared"))
        })
    }

    /// Build a **fresh**, isolated [`SessionContext`] from these ingredients.
    ///
    /// Each call returns a brand-new context with its own `CatalogList` (so
    /// `register_table("port_0", …)` / `register_table("sumstats", …)` never
    /// collide across nodes or across executions) while sharing the engine-wide
    /// [`RuntimeEnv`]. This is the *only* way a `DagNode` should obtain a
    /// `SessionContext` inside `execute`: the framework injects a `&NodeCtx`,
    /// the node calls `ctx.session()`, and the resulting context is dropped at
    /// the end of the execution — no mutable catalog state ever leaks across
    /// runs or between `clone_box` copies of a node.
    pub fn session(&self) -> SessionContext {
        new_isolated_ctx(self.runtime_env.clone())
    }
}

/// Summary of a registered node kind returned by [`NodeRegistry::list_nodes`].
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub kind: String,
    pub desc: String,
    pub deprecated: bool,
    pub data_bundles: Vec<DataBundleBinding>,
}

/// The single source of truth of "which node kinds exist and how to build one from spec."
///
/// This object handles DagNode building and generalize operation of different nodes into uniformed
/// methods.
pub struct NodeRegistry {
    node_ctx: NodeCtx,
    nodes: HashMap<String, Box<dyn NodeFactory>>,
}

impl NodeRegistry {
    /// Create an **empty** registry with the given node context.
    ///
    /// Node factories are registered separately via [`Self::register`] or
    /// [`Self::register_plugin`]. The engine host (data-engine crate) is
    /// responsible for populating the registry with concrete factories.
    pub fn new(node_ctx: NodeCtx) -> Self {
        Self {
            node_ctx,
            nodes: Default::default(),
        }
    }

    /// Convenience: create an empty registry from the two engine-level
    /// ingredients (wraps [`NodeCtx::new`] + [`Self::new`]).
    pub fn with_ingredients(
        runtime_env: Arc<RuntimeEnv>,
        opendal: Option<Arc<vfs::OpendalFileStorage>>,
    ) -> Self {
        Self::new(NodeCtx::new(runtime_env, opendal))
    }

    /// Register a single node factory.
    pub fn register(&mut self, factory: Box<dyn NodeFactory>) {
        self.nodes.insert(factory.kind().to_string(), factory);
    }

    /// Register all factories from a [`NodePlugin`](crate::plugin::NodePlugin).
    pub fn register_plugin(&mut self, plugin: &dyn crate::plugin::NodePlugin) {
        plugin.register(self);
    }

    /// Borrow the shared [`NodeCtx`] (handed to every factory's `build`).
    pub fn ctx(&self) -> &NodeCtx {
        &self.node_ctx
    }

    fn get_node_factory(&self, node_kind: &str) -> Result<&dyn NodeFactory> {
        self.nodes
            .get(node_kind)
            .map(|b| b.as_ref())
            .ok_or(Error::FactoryNotFound {
                kind: node_kind.to_string(),
            })
    }

    pub fn build_node(&self, node_kind: &str, spec: serde_json::Value) -> Result<Box<dyn DagNode>> {
        let node_factory = self.get_node_factory(node_kind)?;
        // Repair common LLM spec pathologies (object-wrapped arrays like
        // `{"item": x}`, numeric strings for number fields) against the
        // factory's own JSON Schema before deserializing. Schema-driven, so
        // well-formed specs pass through unchanged.
        let schema = serde_json::to_value(node_factory.spec_schema()).map_err(|e| {
            Error::Unknown(format!(
                "failed to serialize spec schema for kind '{node_kind}': {e}"
            ))
        })?;
        let spec = super::spec_normalize::normalize_against_schema(spec, &schema);
        let bindings = node_factory.data_bundles_for_spec(spec.clone())?;
        let build_ctx = self
            .node_ctx
            .clone()
            .with_bound_data_bundles(node_kind, bindings)?;
        // If the factory still can't deserialize the spec, upgrade the bare
        // serde error into an agent-facing SpecRejection carrying the kind,
        // the expected schema, and concrete remediation guidance.
        let node = node_factory
            .build(spec, build_ctx)
            .map_err(|err| match err {
                super::error::Error::SpecDeserialize { source } => {
                    super::error::Error::spec_rejection_from(node_kind, &schema, source)
                }
                other => other,
            })?;
        Ok(node)
    }

    /// Return the JSON Schema that validates [`kind`]'s node spec.
    pub fn get_node_spec(&self, node_kind: &str) -> Result<schemars::Schema> {
        Ok(self.get_node_factory(node_kind)?.spec_schema())
    }

    /// Look up a node factory by kind string.
    pub fn get_factory(&self, kind: &str) -> Result<&dyn NodeFactory> {
        self.get_node_factory(kind)
    }

    pub fn get_node_ports(&self, node_kind: &str) -> Result<NodePorts> {
        Ok(self.get_node_factory(node_kind)?.ports())
    }

    /// Resolve the concrete port layout for a node spec. This is the metadata
    /// contract used by agent wiring for dynamic-port
    /// node kinds such as `container_command`.
    pub fn get_node_ports_for_spec(
        &self,
        node_kind: &str,
        spec: serde_json::Value,
    ) -> Result<NodePorts> {
        self.get_node_factory(node_kind)?.ports_for_spec(spec)
    }

    pub fn get_node_doc(&self, node_kind: &str) -> Result<String> {
        Ok(self.get_node_factory(node_kind)?.doc().to_string())
    }

    /// Return metadata of every registered node kind (kind + JSON Schema + ports).
    pub fn list_nodes(&self) -> Vec<NodeInfo> {
        self.nodes
            .iter()
            .map(|(kind, factory)| NodeInfo {
                kind: kind.clone(),
                desc: factory.desc().to_string(),
                deprecated: factory.deprecated(),
                data_bundles: factory.data_bundles(),
            })
            .collect()
    }
}
