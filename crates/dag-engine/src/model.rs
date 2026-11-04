//! Authored workflow state: processes and wires.
//!
//! This is the **only** editable, persisted, snapshotted representation of a
//! workflow (see the crate-level invariant). One [`Process`] corresponds to one
//! workflow process — never to one item. Item instances are run-scoped and live
//! in [`crate::instance`] / [`crate::plan`].
//!
//! Serialization: [`FlowGraph`] round-trips through JSON so snapshots, refs,
//! and history can store authored state at process granularity regardless of
//! how many items a run expands to.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

/// Identifier of a process. Plain string: instance ids are derived from it as
/// `"{process}#{item_key}"`, matching the stability rules in [`crate::instance`].
pub type ProcessName = String;

/// Port index within a process, shared with dag-core's port numbering.
pub type PortIndex = u8;

/// Errors produced by authored-state construction and validation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A structural rule of the authored graph was violated. The message is
    /// user-facing (agent-facing) and states the exact offending element.
    #[error("{0}")]
    Invalid(String),
}

/// Where a fanout process's items come from. Resolved at **run start** (see
/// [`crate::plan`]); never at assembly time — a graph whose directory grows
/// from 10 to 100 files between runs is the same authored graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    /// Enumerate items by glob pattern at run start. The pattern addresses
    /// the storage layer (`vfs://…` or a plain local path); enumeration I/O
    /// belongs to the scheduler, not the model.
    Glob { pattern: String },
    /// Explicit, static item list. Cardinality is known without I/O, but
    /// still resolved through the same run-start path for uniformity.
    Items { values: Vec<serde_json::Value> },
}

impl Source {
    pub fn glob(pattern: impl Into<String>) -> Self {
        Source::Glob {
            pattern: pattern.into(),
        }
    }

    pub fn items(values: Vec<serde_json::Value>) -> Self {
        Source::Items { values }
    }
}

/// How many instances a process materializes into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Cardinality {
    /// Exactly one instance. A singleton may receive `Collect` wires from
    /// fanout stages; a `Direct` wire from a fanout process is a validation
    /// error (which item would it receive?).
    Singleton,
    /// One instance per item of [`Source`], resolved at run start.
    Fanout { source: Source, axis: String },
}

impl Cardinality {
    pub fn fanout(source: Source, axis: impl Into<String>) -> Self {
        Cardinality::Fanout {
            source,
            axis: axis.into(),
        }
    }

    pub fn is_fanout(&self) -> bool {
        matches!(self, Cardinality::Fanout { .. })
    }
}

/// One workflow process: the authored unit of work. `kind` names a node kind in
/// the (reused) dag-core node registry; `spec` is the raw template — `{{item}}`
/// / `{{item.field}}` placeholders are rendered per instance at
/// materialization time ([`crate::instance`]), never here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub id: ProcessName,
    pub kind: String,
    pub spec: serde_json::Value,
    pub cardinality: Cardinality,
}

impl Process {
    pub fn singleton(
        id: impl Into<ProcessName>,
        kind: impl Into<String>,
        spec: serde_json::Value,
    ) -> Self {
        Process {
            id: id.into(),
            kind: kind.into(),
            spec,
            cardinality: Cardinality::Singleton,
        }
    }

    pub fn fanout(
        id: impl Into<ProcessName>,
        kind: impl Into<String>,
        spec: serde_json::Value,
        source: Source,
        axis: impl Into<String>,
    ) -> Self {
        Process {
            id: id.into(),
            kind: kind.into(),
            spec,
            cardinality: Cardinality::fanout(source, axis),
        }
    }
}

/// Edge operator. Routing semantics are resolved by [`crate::plan`]; the
/// model only records intent and enforces the structural rules below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireOp {
    /// Item-preserving routing:
    /// - fanout → fanout: zip by item key (key sets must agree at run start);
    /// - singleton → any: broadcast to every target instance;
    /// - singleton → singleton: plain 1:1 edge.
    ///
    /// `Direct` from fanout into a singleton is invalid — use
    /// [`WireOp::Collect`].
    Direct,
    /// Collapse every source instance into the target's single input port
    /// (Nextflow's `collect` as an edge operator). Only meaningful from a
    /// fanout process into a singleton; the executor aggregates the bindings.
    Collect,
}

/// One wire between two stages' ports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wire {
    pub from: ProcessName,
    pub from_port: PortIndex,
    pub to: ProcessName,
    pub to_port: PortIndex,
    pub op: WireOp,
}

impl Wire {
    pub fn direct(
        from: impl Into<ProcessName>,
        from_port: PortIndex,
        to: impl Into<ProcessName>,
        to_port: PortIndex,
    ) -> Self {
        Wire {
            from: from.into(),
            from_port,
            to: to.into(),
            to_port,
            op: WireOp::Direct,
        }
    }

    pub fn collect(
        from: impl Into<ProcessName>,
        from_port: PortIndex,
        to: impl Into<ProcessName>,
        to_port: PortIndex,
    ) -> Self {
        Wire {
            from: from.into(),
            from_port,
            to: to.into(),
            to_port,
            op: WireOp::Collect,
        }
    }
}

/// Process-granular workflow graph: the authored state.
///
/// Structural validation ([`FlowGraph::validate`]) is pure — no registry
/// lookup (kind existence is checked at install time against the real node
/// registry), no I/O (source enumeration is run start work).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowGraph {
    processes: Vec<Process>,
    wires: Vec<Wire>,
}

impl FlowGraph {
    pub fn builder() -> FlowGraphBuilder {
        FlowGraphBuilder::default()
    }

    pub fn processes(&self) -> &[Process] {
        &self.processes
    }

    pub fn wires(&self) -> &[Wire] {
        &self.wires
    }

    pub fn process(&self, id: &str) -> Option<&Process> {
        self.processes.iter().find(|process| process.id == id)
    }

    /// Incoming wires of `id`, in insertion order.
    pub fn incoming(&self, id: &str) -> impl Iterator<Item = &Wire> {
        self.wires.iter().filter(move |wire| wire.to == id)
    }

    /// Outgoing wires of `id`, in insertion order.
    pub fn outgoing(&self, id: &str) -> impl Iterator<Item = &Wire> {
        self.wires.iter().filter(move |wire| wire.from == id)
    }

    /// Validate the structural rules of the authored state:
    ///
    /// 1. stage ids are nonempty, unique, and free of `#` (reserved as the
    ///    instance-id separator);
    /// 2. wire endpoints exist; no self-wires; no duplicate wires;
    /// 3. the wire graph is acyclic;
    /// 4. `Direct` never routes a fanout into a singleton;
    /// 5. `Collect` is only used from a fanout into a singleton.
    ///
    /// Run-start-only rules (fanout→fanout key-set agreement) intentionally
    /// live in [`crate::plan`], not here.
    pub fn validate(&self) -> Result<(), Error> {
        let mut ids = BTreeSet::new();
        for process in &self.processes {
            if process.id.trim().is_empty() {
                return Err(Error::Invalid("process name must be nonempty".into()));
            }
            if process.id.contains('#') {
                return Err(Error::Invalid(format!(
                    "process name `{}` must not contain `#` (reserved instance-id separator)",
                    process.id
                )));
            }
            if !ids.insert(process.id.as_str()) {
                return Err(Error::Invalid(format!(
                    "duplicate process name `{}`",
                    process.id
                )));
            }
            if process.kind.trim().is_empty() {
                return Err(Error::Invalid(format!(
                    "process `{}` must name a node kind",
                    process.id
                )));
            }
        }

        let by_id = self
            .processes
            .iter()
            .map(|process| (process.id.as_str(), process))
            .collect::<BTreeMap<_, _>>();

        let mut seen_wires = BTreeSet::new();
        let mut successors = BTreeMap::<&str, Vec<&str>>::new();
        let mut predecessors = BTreeMap::<&str, Vec<&str>>::new();
        for wire in &self.wires {
            let from = by_id.get(wire.from.as_str()).ok_or_else(|| {
                Error::Invalid(format!("wire references unknown process `{}`", wire.from))
            })?;
            let to = by_id.get(wire.to.as_str()).ok_or_else(|| {
                Error::Invalid(format!("wire references unknown process `{}`", wire.to))
            })?;
            if wire.from == wire.to {
                return Err(Error::Invalid(format!(
                    "process `{}` wired to itself",
                    wire.from
                )));
            }
            if !seen_wires.insert((
                wire.from.as_str(),
                wire.from_port,
                wire.to.as_str(),
                wire.to_port,
                wire.op,
            )) {
                return Err(Error::Invalid(format!(
                    "duplicate wire {}:{} -> {}:{}",
                    wire.from, wire.from_port, wire.to, wire.to_port
                )));
            }
            match wire.op {
                WireOp::Direct => {
                    if from.cardinality.is_fanout() && !to.cardinality.is_fanout() {
                        return Err(Error::Invalid(format!(
                            "Direct wire {} -> {} routes a fanout into a singleton; \
                             use a Collect wire",
                            wire.from, wire.to
                        )));
                    }
                }
                WireOp::Collect => {
                    if !from.cardinality.is_fanout() || to.cardinality.is_fanout() {
                        return Err(Error::Invalid(format!(
                            "Collect wire {} -> {} must route a fanout into a singleton",
                            wire.from, wire.to
                        )));
                    }
                }
            }
            successors
                .entry(wire.from.as_str())
                .or_default()
                .push(wire.to.as_str());
            predecessors
                .entry(wire.to.as_str())
                .or_default()
                .push(wire.from.as_str());
        }

        // Acyclicity over the stage graph (Kahn). Process ids without wires are
        // isolated and never queued, which is allowed: a lone source stage is
        // a valid (if trivial) flow.
        let mut indegree = BTreeMap::<&str, usize>::new();
        for process in &self.processes {
            indegree.entry(process.id.as_str()).or_insert(0);
        }
        for (id, preds) in &predecessors {
            indegree.insert(id, preds.len());
        }
        let mut queue = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(id, _)| *id)
            .collect::<VecDeque<_>>();
        let mut visited = 0usize;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            for successor in successors.get(id).cloned().unwrap_or_default() {
                let degree = indegree
                    .get_mut(successor)
                    .expect("edge targets are processes");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(successor);
                }
            }
        }
        if visited != self.processes.len() {
            return Err(Error::Invalid(
                "flow graph contains a cycle among processes".into(),
            ));
        }

        Ok(())
    }
}

/// Chained construction of a [`FlowGraph`]. [`FlowGraphBuilder::build`]
/// validates; earlier mutations are kept so callers can surface every error
/// in one pass by calling `build` repeatedly after fixes.
#[derive(Debug, Clone, Default)]
pub struct FlowGraphBuilder {
    graph: FlowGraph,
}

impl FlowGraphBuilder {
    pub fn process(mut self, process: Process) -> Self {
        self.graph.processes.push(process);
        self
    }

    pub fn wire(mut self, wire: Wire) -> Self {
        self.graph.wires.push(wire);
        self
    }

    pub fn build(self) -> Result<FlowGraph, Error> {
        self.graph.validate()?;
        Ok(self.graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qc_to_multiqc(op: WireOp) -> Wire {
        Wire {
            from: "qc".into(),
            from_port: 0,
            to: "multiqc".into(),
            to_port: 0,
            op,
        }
    }

    fn sample_flow() -> FlowGraphBuilder {
        FlowGraph::builder()
            .process(Process::fanout(
                "qc",
                "fastqc",
                serde_json::json!({ "input": "{{item}}" }),
                Source::glob("data/*.fastq"),
                "sample",
            ))
            .process(Process::singleton(
                "multiqc",
                "multiqc",
                serde_json::json!({ "inputs": "{{qc.collect}}" }),
            ))
    }

    #[test]
    fn valid_flow_passes() {
        sample_flow()
            .wire(qc_to_multiqc(WireOp::Collect))
            .build()
            .unwrap();
    }

    #[test]
    fn direct_from_fanout_into_singleton_is_rejected() {
        let error = sample_flow()
            .wire(qc_to_multiqc(WireOp::Direct))
            .build()
            .unwrap_err();
        assert!(error.to_string().contains("use a Collect wire"), "{error}");
    }

    #[test]
    fn collect_from_singleton_is_rejected() {
        let error = FlowGraph::builder()
            .process(Process::singleton("a", "kind", serde_json::json!({})))
            .process(Process::singleton("b", "kind", serde_json::json!({})))
            .wire(Wire::collect("a", 0, "b", 0))
            .build()
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("must route a fanout into a singleton"),
            "{error}"
        );
    }

    #[test]
    fn cycles_are_rejected() {
        let error = FlowGraph::builder()
            .process(Process::singleton("a", "kind", serde_json::json!({})))
            .process(Process::singleton("b", "kind", serde_json::json!({})))
            .wire(Wire::direct("a", 0, "b", 0))
            .wire(Wire::direct("b", 0, "a", 0))
            .build()
            .unwrap_err();
        assert!(error.to_string().contains("cycle"), "{error}");
    }

    #[test]
    fn duplicate_stage_ids_and_reserved_characters_are_rejected() {
        let duplicate = FlowGraph::builder()
            .process(Process::singleton("a", "kind", serde_json::json!({})))
            .process(Process::singleton("a", "kind", serde_json::json!({})))
            .build()
            .unwrap_err();
        assert!(
            duplicate.to_string().contains("duplicate process name"),
            "{duplicate}"
        );

        let reserved = FlowGraph::builder()
            .process(Process::singleton("a#1", "kind", serde_json::json!({})))
            .build()
            .unwrap_err();
        assert!(
            reserved
                .to_string()
                .contains("reserved instance-id separator"),
            "{reserved}"
        );
    }

    #[test]
    fn unknown_wire_endpoints_and_duplicates_are_rejected() {
        let unknown = sample_flow()
            .wire(Wire::direct("qc", 0, "nowhere", 0))
            .build()
            .unwrap_err();
        assert!(unknown.to_string().contains("unknown process"), "{unknown}");

        let duplicate = sample_flow()
            .wire(qc_to_multiqc(WireOp::Collect))
            .wire(qc_to_multiqc(WireOp::Collect))
            .build()
            .unwrap_err();
        assert!(
            duplicate.to_string().contains("duplicate wire"),
            "{duplicate}"
        );
    }

    #[test]
    fn fanout_to_fanout_direct_is_valid() {
        FlowGraph::builder()
            .process(Process::fanout(
                "qc",
                "fastqc",
                serde_json::json!({}),
                Source::items(vec![serde_json::json!("a"), serde_json::json!("b")]),
                "sample",
            ))
            .process(Process::fanout(
                "align",
                "bowtie",
                serde_json::json!({}),
                Source::items(vec![serde_json::json!("a"), serde_json::json!("b")]),
                "sample",
            ))
            .wire(Wire::direct("qc", 0, "align", 0))
            .build()
            .unwrap();
    }

    #[test]
    fn authored_state_round_trips_through_json() {
        let graph = sample_flow()
            .wire(qc_to_multiqc(WireOp::Collect))
            .build()
            .unwrap();
        let json = serde_json::to_value(&graph).unwrap();
        let restored: FlowGraph = serde_json::from_value(json).unwrap();
        assert_eq!(restored.processes().len(), 2);
        assert_eq!(restored.wires().len(), 1);
        restored.validate().unwrap();
    }

    #[test]
    fn isolated_process_is_valid() {
        FlowGraph::builder()
            .process(Process::singleton("lonely", "kind", serde_json::json!({})))
            .build()
            .unwrap();
    }
}
