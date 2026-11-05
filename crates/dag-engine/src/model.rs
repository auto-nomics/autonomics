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

/// What a channel source produces — Nextflow's `Channel.fromPath` (glob) and
/// `Channel.of / fromList` (items). Resolved at **run start**; never at
/// assembly time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceSpec {
    /// Enumerate items by glob pattern at run start. The pattern addresses
    /// the storage layer (`vfs://…` or a plain local path); enumeration I/O
    /// belongs to the scheduler, not the model.
    Glob {
        pattern: PathPatterns,
        #[serde(default, rename = "pathType")]
        path_type: PathType,
        #[serde(default)]
        hidden: bool,
        #[serde(default, rename = "checkIfExists")]
        check_if_exists: bool,
        #[serde(default = "default_glob")]
        glob: bool,
        #[serde(default, rename = "maxDepth", skip_serializing_if = "Option::is_none")]
        max_depth: Option<i32>,
    },
    /// Group matching file paths by their Nextflow filename prefix.
    FilePairs {
        pattern: PathPatterns,
        #[serde(default, rename = "pathType")]
        path_type: PathType,
        #[serde(default)]
        hidden: bool,
        #[serde(default, rename = "checkIfExists")]
        check_if_exists: bool,
        #[serde(default = "default_glob")]
        glob: bool,
        #[serde(default, rename = "maxDepth", skip_serializing_if = "Option::is_none")]
        max_depth: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<i64>,
        #[serde(default)]
        flat: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grouping: Option<serde_json::Value>,
    },
    /// Explicit, static item list.
    Items { values: Vec<serde_json::Value> },
    /// A reusable value, corresponding to Nextflow Channel.value.
    Value { value: serde_json::Value },
}

fn default_glob() -> bool {
    true
}

/// Which entries a glob enumerates, corresponding to Nextflow's `type` option.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathType {
    #[default]
    File,
    Dir,
    Any,
}

/// A single path pattern or an ordered list of patterns, as in fromPath.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathPatterns {
    One(String),
    Many(Vec<String>),
}

impl From<String> for PathPatterns {
    fn from(value: String) -> Self {
        Self::One(value)
    }
}
impl From<&str> for PathPatterns {
    fn from(value: &str) -> Self {
        Self::One(value.to_owned())
    }
}
impl From<Vec<String>> for PathPatterns {
    fn from(value: Vec<String>) -> Self {
        Self::Many(value)
    }
}

impl PathPatterns {
    pub fn as_slice(&self) -> &[String] {
        match self {
            Self::One(pattern) => std::slice::from_ref(pattern),
            Self::Many(patterns) => patterns,
        }
    }
    fn is_missing(&self) -> bool {
        match self {
            Self::One(pattern) => pattern.is_empty(),
            Self::Many(patterns) => patterns.is_empty(),
        }
    }
}

/// A channel source: the authored origin of items. Nextflow has processes
/// declare nothing about how many tasks they run — the count comes from the
/// deliveries on their input channels, and a source is what feeds the first
/// process. Sources are first-class graph entities for exactly that reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// Channel name (`reads`, `samples`…).
    pub id: ProcessName,
    pub spec: SourceSpec,
}

impl Source {
    pub fn glob(id: impl Into<ProcessName>, pattern: impl Into<String>) -> Self {
        Source {
            id: id.into(),
            spec: SourceSpec::Glob {
                pattern: PathPatterns::One(pattern.into()),
                path_type: PathType::File,
                hidden: false,
                check_if_exists: false,
                glob: true,
                max_depth: None,
            },
        }
    }

    pub fn items(id: impl Into<ProcessName>, values: Vec<serde_json::Value>) -> Self {
        Source {
            id: id.into(),
            spec: SourceSpec::Items { values },
        }
    }
    pub fn value(id: impl Into<ProcessName>, value: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            spec: SourceSpec::Value { value },
        }
    }
}

impl SourceSpec {
    pub fn glob(pattern: impl Into<String>) -> Self {
        SourceSpec::Glob {
            pattern: PathPatterns::One(pattern.into()),
            path_type: PathType::File,
            hidden: false,
            check_if_exists: false,
            glob: true,
            max_depth: None,
        }
    }

    pub fn items(values: Vec<serde_json::Value>) -> Self {
        SourceSpec::Items { values }
    }
}

/// Failure posture of one process — Nextflow's `errorStrategy` directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorMode {
    /// Default: a failed instance fatally skips its dependents, including
    /// `Collect` consumers.
    Fail,
    /// A failed instance is recorded but does not fail the run: key-matched
    /// `Direct` dependents are skipped (the item was never delivered), while
    /// `Collect` consumers run on the surviving bindings.
    Ignore,
    /// A failed instance is re-dispatched up to `max_retries` times (with an
    /// optional delay); exhaustion degrades to [`ErrorMode::Fail`].
    Retry,
}

/// Per-process error strategy. Defaults to fail-closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErrorStrategy {
    pub mode: ErrorMode,
    /// Total re-dispatches per instance for [`ErrorMode::Retry`].
    pub max_retries: usize,
    /// Delay between retry attempts.
    pub retry_delay_ms: u64,
}

impl Default for ErrorStrategy {
    fn default() -> Self {
        Self {
            mode: ErrorMode::Fail,
            max_retries: 0,
            retry_delay_ms: 0,
        }
    }
}

impl ErrorStrategy {
    pub fn ignore() -> Self {
        Self {
            mode: ErrorMode::Ignore,
            ..Self::default()
        }
    }

    pub fn retry(max_retries: usize, retry_delay_ms: u64) -> Self {
        Self {
            mode: ErrorMode::Retry,
            max_retries,
            retry_delay_ms,
        }
    }
}

/// One workflow process: the authored unit of work. `kind` names a node kind in
/// the (reused) dag-core node registry; `spec` is the raw template — `{{item}}`
/// / `{{item.field}}` placeholders are rendered per instance at
/// materialization time ([`crate::instance`]), never here.
#[derive(Debug, Clone, Serialize, Deserialize)]
/// A process declares WHAT runs, never HOW MANY times — Nextflow semantics:
/// the instance count is derived from the deliveries on its input channels
/// (see [`crate::plan`]).
pub struct Process {
    pub id: ProcessName,
    pub kind: String,
    pub spec: serde_json::Value,
    /// Failure posture (Nextflow's `errorStrategy`). Defaults to fail-closed.
    #[serde(default)]
    pub error_strategy: ErrorStrategy,
}

impl Process {
    pub fn new(
        id: impl Into<ProcessName>,
        kind: impl Into<String>,
        spec: serde_json::Value,
    ) -> Self {
        Process {
            id: id.into(),
            kind: kind.into(),
            spec,
            error_strategy: ErrorStrategy::default(),
        }
    }
}

/// Edge operator. Routing semantics are resolved by [`crate::plan`]; the
/// model only records intent and enforces the structural rules below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl std::fmt::Display for WireOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Direct => "direct",
            Self::Collect => "collect",
        })
    }
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
    #[serde(default)]
    sources: Vec<Source>,
    processes: Vec<Process>,
    wires: Vec<Wire>,
}

impl FlowGraph {
    pub fn builder() -> FlowGraphBuilder {
        FlowGraphBuilder::default()
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|source| source.id == id)
    }

    /// The empty (no processes, no wires) flow — the initial session state.
    pub fn empty() -> Self {
        Self::default()
    }

    /// True when the flow declares no processes. Running an empty flow is an
    /// authoring error surfaced by the engine, not a valid no-op.
    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
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
    /// 1. ids are nonempty, unique across sources and processes, and free of
    ///    `#` (the instance-id separator);
    /// 2. wire endpoints exist (source or process); no self-wires; no
    ///    duplicate wires; ports are `None` (implicit single port) or valid;
    /// 3. the wire graph is acyclic;
    /// Queue and value inputs may be combined; matching is performed at runtime.
    pub fn validate(&self) -> Result<(), Error> {
        let mut ids = BTreeSet::new();
        let mut claim = |id: &str| -> Result<(), Error> {
            if id.trim().is_empty() {
                return Err(Error::Invalid("id must be nonempty".into()));
            }
            if id.contains('#') {
                return Err(Error::Invalid(format!(
                    "id `{id}` must not contain `#` (reserved instance-id separator)"
                )));
            }
            if !ids.insert(id.to_string()) {
                return Err(Error::Invalid(format!("duplicate id `{id}`")));
            }
            Ok(())
        };
        for source in &self.sources {
            claim(&source.id)?;
            if let SourceSpec::Glob { pattern, .. } | SourceSpec::FilePairs { pattern, .. } =
                &source.spec
            {
                if pattern.is_missing() {
                    return Err(Error::Invalid(format!(
                        "source `{}` requires a path pattern",
                        source.id
                    )));
                }
            }
            if let SourceSpec::FilePairs {
                grouping: Some(function),
                ..
            } = &source.spec
            {
                let function = function
                    .get("__dag_shell_function__")
                    .and_then(|value| value.as_object())
                    .ok_or_else(|| {
                        Error::Invalid(format!(
                            "source `{}` has invalid grouping function",
                            source.id
                        ))
                    })?;
                if !(function.len() == 2
                    || (function.len() == 3
                        && function
                            .get("functions")
                            .is_some_and(|value| value.is_array())))
                    || !function
                        .get("source")
                        .is_some_and(|value| value.is_string())
                    || !function
                        .get("captures")
                        .is_some_and(|value| value.is_object())
                {
                    return Err(Error::Invalid(format!(
                        "source `{}` has invalid grouping function",
                        source.id
                    )));
                }
            }
        }
        for process in &self.processes {
            claim(&process.id)?;
            if process.kind.trim().is_empty() {
                return Err(Error::Invalid(format!(
                    "process `{}` must name a node kind",
                    process.id
                )));
            }
        }

        let source_ids: BTreeSet<&str> = self.sources.iter().map(|s| s.id.as_str()).collect();
        let process_ids: BTreeSet<&str> = self.processes.iter().map(|p| p.id.as_str()).collect();
        let exists = |id: &str| source_ids.contains(id) || process_ids.contains(id);

        let mut seen_wires = BTreeSet::new();
        let mut successors = BTreeMap::<&str, Vec<&str>>::new();
        let mut predecessors = BTreeMap::<&str, Vec<&str>>::new();
        for wire in &self.wires {
            if !exists(&wire.from) {
                return Err(Error::Invalid(format!(
                    "wire references unknown endpoint `{}`",
                    wire.from
                )));
            }
            if !exists(&wire.to) {
                return Err(Error::Invalid(format!(
                    "wire references unknown endpoint `{}`",
                    wire.to
                )));
            }
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
            if !process_ids.contains(wire.to.as_str()) {
                return Err(Error::Invalid(format!(
                    "wire target `{}` must be a process (sources only produce)",
                    wire.to
                )));
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

        // Acyclicity over the graph (Kahn). Isolated nodes are allowed.
        let mut indegree = BTreeMap::<&str, usize>::new();
        for id in ids.iter().map(String::as_str) {
            indegree.entry(id).or_insert(0);
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
                let degree = indegree.get_mut(successor).expect("edge targets are nodes");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(successor);
                }
            }
        }
        if visited != self.sources.len() + self.processes.len() {
            return Err(Error::Invalid("flow graph contains a cycle".into()));
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
    pub fn source(mut self, source: Source) -> Self {
        self.graph.sources.push(source);
        self
    }

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

    #[test]
    fn legacy_glob_defaults_are_preserved_and_unknown_options_rejected() {
        let source: SourceSpec =
            serde_json::from_value(serde_json::json!({"type":"glob","pattern":"data/*"})).unwrap();
        assert_eq!(source, SourceSpec::glob("data/*"));
        assert!(
            serde_json::from_value::<SourceSpec>(
                serde_json::json!({"type":"glob","pattern":"data/*","hiddden":true})
            )
            .is_err()
        );
    }

    #[test]
    fn path_patterns_require_a_nonempty_input() {
        for pattern in [serde_json::json!(""), serde_json::json!([])] {
            let spec: SourceSpec =
                serde_json::from_value(serde_json::json!({"type":"glob", "pattern":pattern}))
                    .unwrap();
            let error = FlowGraphBuilder::default()
                .source(Source {
                    id: "reads".into(),
                    spec,
                })
                .build()
                .unwrap_err();
            assert!(
                error.to_string().contains("requires a path pattern"),
                "{error}"
            );
        }
    }

    fn sample_flow() -> FlowGraphBuilder {
        FlowGraphBuilder::default()
            .source(Source::glob("reads", "data/*.fastq"))
            .process(Process::new(
                "qc",
                "fastqc",
                serde_json::json!({ "input": "{{item}}" }),
            ))
            .process(Process::new("multiqc", "multiqc", serde_json::json!({})))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
    }

    fn pipeline() -> FlowGraphBuilder {
        sample_flow().wire(Wire::direct("reads", 0, "qc", 0))
    }

    #[test]
    fn valid_flow_passes() {
        pipeline().build().unwrap();
    }

    #[test]
    fn direct_from_source_is_the_driving_input() {
        pipeline().build().unwrap();
    }

    #[test]
    fn collect_can_mix_with_queue_input() {
        sample_flow()
            .source(Source::items("extra", vec![serde_json::json!("x")]))
            .wire(Wire::direct("extra", 0, "multiqc", 1))
            .build()
            .unwrap();
    }

    #[test]
    fn multiple_queue_inputs_are_allowed() {
        FlowGraphBuilder::default()
            .source(Source::items("a", vec![serde_json::json!(1)]))
            .source(Source::items("b", vec![serde_json::json!(2)]))
            .process(Process::new("m", "merge", serde_json::json!({})))
            .wire(Wire::direct("a", 0, "m", 0))
            .wire(Wire::direct("b", 0, "m", 1))
            .build()
            .unwrap();
    }

    #[test]
    fn cycles_are_rejected() {
        let error = FlowGraphBuilder::default()
            .process(Process::new("a", "kind", serde_json::json!({})))
            .process(Process::new("b", "kind", serde_json::json!({})))
            .wire(Wire::direct("a", 0, "b", 0))
            .wire(Wire::direct("b", 0, "a", 0))
            .build()
            .unwrap_err();
        assert!(error.to_string().contains("cycle"), "{error}");
    }

    #[test]
    fn duplicate_ids_and_reserved_characters_are_rejected() {
        let duplicate = FlowGraphBuilder::default()
            .process(Process::new("a", "kind", serde_json::json!({})))
            .process(Process::new("a", "kind", serde_json::json!({})))
            .build()
            .unwrap_err();
        assert!(
            duplicate.to_string().contains("duplicate id"),
            "{duplicate}"
        );

        let reserved = FlowGraphBuilder::default()
            .source(Source::items("r#1", vec![serde_json::json!("x")]))
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
        let unknown = pipeline()
            .wire(Wire::direct("qc", 0, "nowhere", 0))
            .build()
            .unwrap_err();
        assert!(
            unknown.to_string().contains("unknown endpoint"),
            "{unknown}"
        );

        let duplicate = pipeline()
            .wire(Wire::collect("qc", 0, "multiqc", 0))
            .build()
            .unwrap_err();
        assert!(
            duplicate.to_string().contains("duplicate wire"),
            "{duplicate}"
        );
    }

    #[test]
    fn wires_must_target_processes() {
        let error = FlowGraphBuilder::default()
            .source(Source::items("a", vec![serde_json::json!("x")]))
            .source(Source::items("b", vec![serde_json::json!("y")]))
            .wire(Wire::direct("a", 0, "b", 0))
            .build()
            .unwrap_err();
        assert!(error.to_string().contains("must be a process"), "{error}");
    }

    #[test]
    fn authored_state_round_trips_through_json() {
        let graph = pipeline().build().unwrap();
        let json = serde_json::to_value(&graph).unwrap();
        let restored: FlowGraph = serde_json::from_value(json).unwrap();
        assert_eq!(restored.sources().len(), 1);
        assert_eq!(restored.processes().len(), 2);
        assert_eq!(restored.wires().len(), 2);
        restored.validate().unwrap();
    }

    #[test]
    fn isolated_process_is_valid() {
        FlowGraphBuilder::default()
            .process(Process::new("lonely", "kind", serde_json::json!({})))
            .build()
            .unwrap();
    }
}
