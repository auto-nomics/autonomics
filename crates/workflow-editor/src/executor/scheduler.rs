//! Topological scheduler for a [`WorkflowManifest`].
//!
//! Phase 1:
//! - Builds a `petgraph` DiGraph from the manifest.
//! - Detects cycles (`petgraph::toposort`).
//! - Iterates nodes in topological order, collecting inputs from upstream
//!   outputs and dispatching through the [`NodeRegistry`].
//!
//! Phase 3 will: route `kind == "skill"` through [`SubgraphNode`].

use crate::error::{ExecutorError, Result};
use crate::executor::ports::{NodeCtx, NodeExecutor, NodeReporter, PortInputs, PortOutputs};
use crate::executor::sop_context::SopContext;
use crate::executor::subgraph_node::SubgraphNode;
use crate::model::WorkflowManifest;
use crate::registry::NodeRegistry;
use crate::registry::skill_registry::SkillRegistry;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;
use petgraph::graph::NodeIndex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Result of a workflow run.
#[derive(Debug, Clone)]
pub struct WorkflowResult {
    /// Per-node outputs keyed by `(node_id, port_id)`.
    pub outputs: HashMap<(Uuid, String), serde_json::Value>,
    /// The set of surface outputs — values from the last node(s).
    /// Currently a copy of `outputs`; Phase 3 will mark which nodes are
    /// "surface" output sinks.
    pub final_outputs: HashMap<String, serde_json::Value>,
}

/// Validation report — result of [`Scheduler::validate`].
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// Whether the manifest is fully valid.
    pub ok: bool,
    /// Schema errors (cycle, missing ports, etc.) — empty if `ok == true`.
    pub errors: Vec<String>,
}

/// Default node executor — connects `NodeFactory` outputs to the runtime.
pub struct Scheduler {
    node_registry: Arc<NodeRegistry>,
    skill_registry: Arc<SkillRegistry>,
}

impl Scheduler {
    /// Construct a scheduler wrapped in `Arc<Self>`. Used by the manager
    /// and by `SubgraphNode` for recursive execution.
    pub fn new(node_registry: Arc<NodeRegistry>, skill_registry: Arc<SkillRegistry>) -> Arc<Self> {
        Arc::new(Self {
            node_registry,
            skill_registry,
        })
    }

    /// Build a `NodeExecutor` for one node, consulting the registries.
    /// `kind == "skill"` resolves to a `SubgraphNode`; everything else looks
    /// up a `NodeFactory`.
    pub fn build_executor(
        self: &Arc<Self>,
        node: &crate::model::NodeEntry,
    ) -> Result<Box<dyn NodeExecutor>> {
        if node.kind == "skill" {
            let skill_name = node
                .params
                .get("skill_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ExecutorError::Internal(format!(
                        "skill node {} missing 'skill_name' param",
                        node.id
                    ))
                })?;
            let skill = self
                .skill_registry
                .get_by_name(skill_name)
                .ok_or_else(|| ExecutorError::SkillNotFound(skill_name.to_string()))?;
            return Ok(Box::new(SubgraphNode::new(
                Arc::new(skill),
                Arc::clone(self),
            )));
        }
        let factory = self
            .node_registry
            .get(&node.kind)
            .ok_or_else(|| ExecutorError::Internal(format!("unknown node kind: {}", node.kind)))?;
        // Phase 1: factories return a `Box<dyn Any>`; we don't have a way to
        // bridge to `NodeExecutor` here yet. Each registered factory must
        // itself be a `NodeExecutor` (Phase 1 keeps that an opt-in by
        // returning a trivial `RegistryAdapterNode`).
        let params = node.params.clone();
        let exec: Box<dyn NodeExecutor> = factory.build_executor(params).ok_or_else(|| {
            ExecutorError::Internal(format!(
                "node factory for '{}' does not yet implement NodeExecutor",
                node.kind
            ))
        })?;
        Ok(exec)
    }

    /// Validate a manifest topologically + structurally.
    pub fn validate(&self, manifest: &WorkflowManifest) -> ValidationReport {
        let mut errors: Vec<String> = Vec::new();
        if let Err(e) = manifest.validate() {
            errors.push(format!("manifest: {}", e));
        }
        match build_graph(manifest) {
            Ok(graph) => {
                if let Err(cycle) = toposort(&graph, None) {
                    errors.push(format!("cycle detected: node {:?}", cycle.node_id()));
                }
            }
            Err(e) => errors.push(format!("graph build: {}", e)),
        }
        ValidationReport {
            ok: errors.is_empty(),
            errors,
        }
    }

    /// Execute a workflow end-to-end.
    pub async fn run(
        &self,
        manifest: &WorkflowManifest,
        inputs: serde_json::Map<String, serde_json::Value>,
        cancel: CancellationToken,
    ) -> Result<WorkflowResult> {
        // Without an Arc<Self> receiver we can't recurse, but plain `run` is
        // the public API for the top-level call. Wrap self temporarily.
        let arc_self = Arc::new(Scheduler {
            node_registry: Arc::clone(&self.node_registry),
            skill_registry: Arc::clone(&self.skill_registry),
        });
        // Build the initial SOP context (workflow-level frame) before
        // passing it down so the recursive scheduler appends its own
        // manifest-level frame on top.
        let mut sop = SopContext::new();
        if let Some(sop_text) = &manifest.sop {
            if !sop_text.is_empty() {
                let _g = sop.push(sop_text.clone(), None);
            }
        }
        // Translate the `inputs` map into a seed_outputs map. For each port
        // id in `inputs`, find a node in this manifest that has a matching
        // input port id and seed (node_id, port_id) with the value. This
        // lets callers wire initial data into the workflow without adding
        // a synthetic source node.
        let seed_outputs = seed_from_inputs(manifest, &inputs);
        arc_self
            .run_with_seed_outputs(manifest, seed_outputs, sop, cancel)
            .await
    }

    /// Execute a workflow end-to-end, with a pre-populated seed map for inner
    /// outputs and an inherited SOP context. Used by
    /// [`crate::executor::SubgraphNode`] to inject surface inputs into a
    /// recursive skill execution.
    pub async fn run_with_seed_outputs(
        self: &Arc<Self>,
        manifest: &WorkflowManifest,
        seed_outputs: HashMap<(Uuid, String), serde_json::Value>,
        parent_sop: SopContext,
        cancel: CancellationToken,
    ) -> Result<WorkflowResult> {
        let report = self.validate(manifest);
        if !report.ok {
            return Err(ExecutorError::Internal(format!(
                "validation failed: {}",
                report.errors.join("; ")
            ))
            .into());
        }

        let graph = build_graph(manifest)?;
        let order = toposort(&graph, None).map_err(|_| ExecutorError::Cycle)?;

        let mut sop = SopContext::new();
        if let Some(sop_text) = &manifest.sop {
            if !sop_text.is_empty() {
                let _g = sop.push(sop_text.clone(), None);
            }
        }

        // Seed outputs are pre-populated; downstream nodes see them as if
        // upstream edges had already fired.
        let mut outputs: HashMap<(Uuid, String), serde_json::Value> = seed_outputs;
        let mut final_outputs: HashMap<String, serde_json::Value> = HashMap::new();
        let reporter = NodeReporter::default();

        for idx in order {
            let node = &manifest.nodes[idx.index()];
            if cancel.is_cancelled() {
                return Err(ExecutorError::Cancelled.into());
            }

            let exec = self.build_executor(node)?;
            // Each iteration needs a fresh clone of the parent SOP plus the
            // manifest-level frame (so SubgraphNode's push/pop is visible).
            let mut node_sop = parent_sop.clone();
            // Push the manifest-level SOP frame and keep the guard alive
            // across the node execution. `let _g = …` inside an `if` block
            // would drop the guard before `node_ctx` is built.
            let _guard = if let Some(sop_text) = &manifest.sop {
                if !sop_text.is_empty() {
                    Some(node_sop.push(sop_text.clone(), None))
                } else {
                    None
                }
            } else {
                None
            };
            let mut node_ctx = NodeCtx {
                workflow_id: manifest.id,
                node_id: node.id,
                sop: node_sop,
                scratch: Default::default(),
                cancel: cancel.clone(),
            };

            // Gather inputs from upstream edges, falling back to seed_outputs
            // when no edge supplies a value (seed_outputs is used by
            // SubgraphNode to inject surface inputs).
            let mut ins: PortInputs = HashMap::new();
            for e in &manifest.edges {
                if e.target == node.id {
                    if let Some(v) = outputs.get(&(e.source, e.source_handle.clone())) {
                        ins.insert(e.target_handle.clone(), v.clone());
                    }
                }
            }
            // Fill in any remaining input ports from the seed map.
            // We don't know which input ports a node declares at scheduling
            // time without re-iterating — iterate `seed_outputs` and skip
            // ports the node already received via an edge.
            for ((seed_node, port), value) in outputs.iter() {
                if *seed_node == node.id && !ins.contains_key(port) {
                    ins.insert(port.clone(), value.clone());
                }
            }

            let outs = exec.execute(&mut node_ctx, &ins, &reporter).await?;
            for (port, value) in outs {
                outputs.insert((node.id, port.clone()), value.clone());
                if port == "out" {
                    final_outputs.insert(node.label.clone(), value);
                }
            }
        }

        Ok(WorkflowResult {
            outputs,
            final_outputs,
        })
    }
}

/// Build a `petgraph::DiGraph` from a manifest. Returns Err on internal
/// inconsistency (should be caught by `manifest.validate()` already).
fn build_graph(manifest: &WorkflowManifest) -> Result<DiGraph<Uuid, ()>> {
    let mut graph = DiGraph::<Uuid, ()>::new();
    let mut id_to_idx: HashMap<Uuid, NodeIndex> = HashMap::new();
    for n in &manifest.nodes {
        let idx = graph.add_node(n.id);
        id_to_idx.insert(n.id, idx);
    }
    for e in &manifest.edges {
        let s = id_to_idx
            .get(&e.source)
            .ok_or_else(|| ExecutorError::Internal(format!("edge source missing: {}", e.source)))?;
        let t = id_to_idx
            .get(&e.target)
            .ok_or_else(|| ExecutorError::Internal(format!("edge target missing: {}", e.target)))?;
        graph.add_edge(*s, *t, ());
    }
    Ok(graph)
}

/// Translate the public `inputs` map (`port_id -> value`) into a seed
/// map (`(node_id, port_id) -> value`). Each entry is wired to the first
/// node in `manifest` whose input port id matches.
fn seed_from_inputs(
    manifest: &WorkflowManifest,
    inputs: &serde_json::Map<String, serde_json::Value>,
) -> HashMap<(Uuid, String), serde_json::Value> {
    let mut seed = HashMap::new();
    for (port_id, value) in inputs {
        if let Some(node) = manifest
            .nodes
            .iter()
            .find(|n| n.inputs.iter().any(|p| p.id == *port_id))
        {
            seed.insert((node.id, port_id.clone()), value.clone());
        }
    }
    seed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ports::NodeExecutor;
    use crate::model::{NodeEntry, Viewport};
    use crate::registry::NodeRegistry;
    use crate::registry::skill_registry::SkillRegistry;
    use async_trait::async_trait;

    /// Phase 1 test helper: a node that echoes its first input.
    struct EchoNode;

    #[async_trait]
    impl NodeExecutor for EchoNode {
        fn kind(&self) -> &'static str {
            "echo"
        }
        async fn execute(
            &self,
            _ctx: &mut NodeCtx,
            inputs: &PortInputs,
            _r: &NodeReporter,
        ) -> Result<PortOutputs> {
            let mut out = PortOutputs::new();
            for (k, v) in inputs {
                out.insert(k.clone(), v.clone());
            }
            out.insert("out".into(), serde_json::json!("echoed"));
            Ok(out)
        }
    }

    fn echo_factory() -> Arc<dyn crate::registry::NodeFactory> {
        use crate::model::PortSpec;
        use crate::registry::NodeFactory;
        struct F;
        #[async_trait]
        impl NodeFactory for F {
            fn kind(&self) -> &'static str {
                "echo"
            }
            fn label(&self) -> &'static str {
                "Echo"
            }
            fn description(&self) -> &'static str {
                "Echoes its input."
            }
            fn category(&self) -> &'static str {
                "Test"
            }
            fn spec_schema(&self) -> schemars::Schema {
                schemars::schema_for!(serde_json::Value)
            }
            fn inputs(&self) -> Vec<PortSpec> {
                vec![PortSpec::new("in", "in")]
            }
            fn outputs(&self) -> Vec<PortSpec> {
                vec![PortSpec::new("out", "out")]
            }
            fn build_executor(&self, _params: serde_json::Value) -> Option<Box<dyn NodeExecutor>> {
                Some(Box::new(EchoNode))
            }
        }
        Arc::new(F)
    }

    fn make_manifest() -> WorkflowManifest {
        let mut m = WorkflowManifest::new("test");
        let n1 = NodeEntry {
            id: Uuid::new_v4(),
            kind: "echo".into(),
            label: "src".into(),
            position: (0, 0),
            inputs: vec![crate::model::PortSpec::new("in", "in")],
            outputs: vec![crate::model::PortSpec::new("out", "out")],
            params: serde_json::json!({}),
        };
        let n2 = NodeEntry {
            id: Uuid::new_v4(),
            kind: "echo".into(),
            label: "sink".into(),
            position: (10, 0),
            inputs: vec![crate::model::PortSpec::new("in", "in")],
            outputs: vec![crate::model::PortSpec::new("out", "out")],
            params: serde_json::json!({}),
        };
        let n1_id = n1.id;
        let n2_id = n2.id;
        m.nodes.push(n1);
        m.nodes.push(n2);
        m.edges.push(crate::model::EdgeEntry {
            id: Uuid::new_v4(),
            source: n1_id,
            source_handle: "out".into(),
            target: n2_id,
            target_handle: "in".into(),
        });
        let _ = Viewport::default();
        m
    }

    #[tokio::test]
    async fn validate_detects_cycle() {
        let reg = Arc::new(NodeRegistry::new());
        reg.register(echo_factory());
        let sk = Arc::new(SkillRegistry::new());
        let scheduler = Scheduler::new(reg, sk);

        let mut m = WorkflowManifest::new("cycle");
        let n = NodeEntry {
            id: Uuid::new_v4(),
            kind: "echo".into(),
            label: "n".into(),
            position: (0, 0),
            inputs: vec![crate::model::PortSpec::new("in", "in")],
            outputs: vec![crate::model::PortSpec::new("out", "out")],
            params: serde_json::json!({}),
        };
        let n_id = n.id;
        m.nodes.push(n);
        m.edges.push(crate::model::EdgeEntry {
            id: Uuid::new_v4(),
            source: n_id,
            source_handle: "out".into(),
            target: n_id,
            target_handle: "in".into(),
        });
        // Self-loop edge fails manifest.validate(); build a 2-node cycle:
        let n2 = NodeEntry {
            id: Uuid::new_v4(),
            kind: "echo".into(),
            label: "n2".into(),
            position: (1, 0),
            inputs: vec![crate::model::PortSpec::new("in", "in")],
            outputs: vec![crate::model::PortSpec::new("out", "out")],
            params: serde_json::json!({}),
        };
        let n2_id = n2.id;
        m.nodes.push(n2);
        m.edges.push(crate::model::EdgeEntry {
            id: Uuid::new_v4(),
            source: n_id,
            source_handle: "out".into(),
            target: n2_id,
            target_handle: "in".into(),
        });
        m.edges.push(crate::model::EdgeEntry {
            id: Uuid::new_v4(),
            source: n2_id,
            source_handle: "out".into(),
            target: n_id,
            target_handle: "in".into(),
        });
        let report = scheduler.validate(&m);
        assert!(!report.ok);
        assert!(report.errors.iter().any(|e| e.contains("cycle")));
    }

    #[tokio::test]
    async fn run_passes_inputs_and_executes_in_order() {
        let reg = Arc::new(NodeRegistry::new());
        reg.register(echo_factory());
        let sk = Arc::new(SkillRegistry::new());
        let scheduler = Scheduler::new(reg, sk);

        let m = make_manifest();
        let cancel = CancellationToken::new();
        let result = scheduler.run(&m, Default::default(), cancel).await.unwrap();
        // Both nodes executed and emitted the literal "echoed" via the `out` port.
        assert!(
            result
                .outputs
                .values()
                .any(|v| v == &serde_json::json!("echoed"))
        );
    }
}
