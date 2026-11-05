//! Run-start planning: resolve channel sources, DERIVE instance counts from
//! the data flowing through the graph, materialize instances, compute routes.
//!
//! This is the Nextflow model: **a process never declares how many times it
//! runs**. The count comes from the deliveries on its input channels —
//!
//! - a process wired `Direct`ly to a **source** runs once per item the
//!   source produces (`Channel.fromPath` feeding a process);
//! - a process wired `Direct`ly from another item-driven process runs once
//!   per item **that process emits** — the item flows through, it is never
//!   re-declared downstream;
//! - a process whose `Direct` inputs all come from single-run processes runs
//!   once (Nextflow's value channel: the input is broadcast to the run);
//! - a process with only `Collect` inputs runs once, after every upstream
//!   item settled (Nextflow's `collect` operator emits one value).
//!
//! All of this is derived here, at run start, from the wiring plus the
//! resolved source items. Nothing about instance counts is authored.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use sha2::{Digest, Sha256};

use crate::instance::{Instance, InstanceId};
use crate::model::{Error, FlowGraph, PortIndex, ProcessName, WireOp};

/// Item sets for every source, enumerated by the scheduler at run start.
/// Must contain an entry for **every** source and nothing else.
#[derive(Debug, Default)]
pub struct SourceResolution(BTreeMap<ProcessName, Vec<serde_json::Value>>);

impl SourceResolution {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Record the enumerated items of one source.
    pub fn resolve(&mut self, source: impl Into<ProcessName>, items: Vec<serde_json::Value>) {
        self.0.insert(source.into(), items);
    }

    pub(crate) fn get(&self, source: &str) -> Option<&Vec<serde_json::Value>> {
        self.0.get(source)
    }

    fn keys(&self) -> impl Iterator<Item = &ProcessName> {
        self.0.keys()
    }
}

/// One instance-to-instance edge, computed from an authored wire. A `Collect`
/// wire fans several routes into the same `(to, to_port)`; aggregation of the
/// bound values is the scheduler's job, not the plan's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstanceRoute {
    pub from: InstanceId,
    pub from_port: PortIndex,
    pub to: InstanceId,
    pub to_port: PortIndex,
    /// The authored wire's operator, needed at completion time: an ignored
    /// failure skips `Direct` dependents but lets `Collect` consumers run on
    /// the surviving bindings.
    pub op: WireOp,
}

/// One item delivered by a source, as a pre-settled pseudo-instance: sources
/// do not execute — their output value **is** the item.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceItem {
    pub id: InstanceId,
    pub key: String,
    /// v1 channels carry file paths: the item must be a string, delivered
    /// downstream as a `File` value.
    pub path: String,
    /// Stable identity downstream instances hash into their fingerprints
    /// (content hash of the item value).
    pub fingerprint: String,
}

/// One source's resolved deliveries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourcePlan {
    pub source: ProcessName,
    /// In enumeration order (stable input order — reports read better when
    /// they match the listing).
    pub items: Vec<SourceItem>,
}

/// One process's materialized instance set for this run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProcessPlan {
    pub process: ProcessName,
    /// The node-registry kind every instance of this process builds from.
    pub kind: String,
    /// The raw (pre-rendering) spec template. Fingerprints hash this, never
    /// the rendered spec — mirroring Nextflow's script-source hashing.
    pub spec: serde_json::Value,
    /// Failure posture carried from the authored process.
    pub error_strategy: crate::model::ErrorStrategy,
    /// Item keys this process runs once per each of — derived from its
    /// driving channel, propagated from the originating source. Empty when
    /// the process has no driving channel ([`Self::driven`] is false) or the
    /// source delivered nothing.
    pub driving_items: Vec<String>,
    /// True when this process has a driving channel (an item source feeding
    /// it directly or through a chain). A driven process with zero deliveries
    /// materializes ZERO instances (Nextflow: no data, no tasks).
    #[serde(default)]
    pub driven: bool,
    pub instances: Vec<Instance>,
}

impl ProcessPlan {
    /// Item keys in driving order; the derived singleton reports `"0"`.
    pub fn keys(&self) -> Vec<&str> {
        self.instances
            .iter()
            .map(|instance| instance.item_key.as_deref().unwrap_or("0"))
            .collect()
    }

    pub fn instance(&self, key: &str) -> Option<&Instance> {
        self.instances
            .iter()
            .find(|instance| instance.item_key.as_deref() == Some(key))
    }

    fn is_driven(&self) -> bool {
        self.driven
    }
}

/// The complete run-start plan: every source's deliveries, every instance
/// that will exist this run, and every edge between them. Serializable so
/// run records can carry the exact expansion next to the authored graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ExecutionPlan {
    pub sources: BTreeMap<ProcessName, SourcePlan>,
    pub processes: BTreeMap<ProcessName, ProcessPlan>,
    pub routes: Vec<InstanceRoute>,
}

impl ExecutionPlan {
    pub fn total_instances(&self) -> usize {
        self.processes
            .values()
            .map(|plan| plan.instances.len())
            .sum()
    }

    pub fn routes_from(&self, id: &InstanceId) -> impl Iterator<Item = &InstanceRoute> {
        self.routes.iter().filter(move |route| &route.from == id)
    }

    pub fn routes_to(&self, id: &InstanceId) -> impl Iterator<Item = &InstanceRoute> {
        self.routes.iter().filter(move |route| &route.to == id)
    }

    /// Is `id` a source pseudo-instance (pre-settled, never executed)?
    pub fn is_source_item(&self, id: &InstanceId) -> bool {
        let (owner, _) = id.parts();
        self.sources.keys().any(|name| name == owner)
    }
}

/// Build the execution plan: resolve sources, DERIVE each process's item set
/// from the wiring, materialize instances, compute routes. Structural
/// failures (missing/stale resolution, non-string items, two queue-like
/// inputs on one process, broken templates) abort before anything dispatches.
pub fn build_plan(
    graph: &FlowGraph,
    resolution: &SourceResolution,
) -> Result<ExecutionPlan, Error> {
    graph.validate()?;

    // ── Source plans ─────────────────────────────────────────────────────
    let mut sources = BTreeMap::new();
    for source in graph.sources() {
        let items = resolution.get(&source.id).ok_or_else(|| {
            Error::Invalid(format!("source `{}` has no resolved item set", source.id))
        })?;
        let mut seen_keys = BTreeSet::new();
        let mut resolved = Vec::with_capacity(items.len());
        for item in items {
            let path = item.as_str().ok_or_else(|| {
                Error::Invalid(format!(
                    "source `{}` produced a non-string item; v1 channels carry file paths",
                    source.id
                ))
            })?;
            let pseudo = Instance::for_item_source(&source.id, item)?;
            let key = pseudo.item_key.clone().expect("source items derive keys");
            if !seen_keys.insert(key.clone()) {
                return Err(Error::Invalid(format!(
                    "source `{}` derives duplicate item key `{key}`; \
                     two items would silently merge into one instance",
                    source.id
                )));
            }
            resolved.push(SourceItem {
                id: pseudo.id,
                key,
                path: path.to_string(),
                fingerprint: item_fingerprint(item),
            });
        }
        sources.insert(
            source.id.clone(),
            SourcePlan {
                source: source.id.clone(),
                items: resolved,
            },
        );
    }
    for name in resolution.keys() {
        if graph.source(name).is_none() {
            return Err(Error::Invalid(format!(
                "source resolution contains `{name}`, which is not a source of this graph"
            )));
        }
    }

    // ── Derive driving items per process (topological) ───────────────────
    // `driving[name] = Some(items)` → run once per item; `None` → run once.
    let source_ids: BTreeSet<&str> = graph.sources().iter().map(|s| s.id.as_str()).collect();
    let process_ids: BTreeSet<&str> = graph.processes().iter().map(|p| p.id.as_str()).collect();

    let mut indegree: BTreeMap<&str, usize> = BTreeMap::new();
    let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for id in source_ids.iter().chain(process_ids.iter()) {
        indegree.insert(id, 0);
        successors.insert(id, Vec::new());
    }
    for wire in graph.wires() {
        successors
            .entry(wire.from.as_str())
            .or_default()
            .push(wire.to.as_str());
        *indegree.entry(wire.to.as_str()).or_default() += 1;
    }
    let mut queue: VecDeque<&str> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut driving: BTreeMap<&str, Option<Vec<serde_json::Value>>> = BTreeMap::new();
    let mut settled = 0usize;
    while let Some(id) = queue.pop_front() {
        settled += 1;
        if process_ids.contains(id) {
            let mut items: Option<Vec<serde_json::Value>> = None;
            let mut queue_like: Vec<&str> = Vec::new();
            for wire in graph.wires() {
                if wire.to != id || wire.op != WireOp::Direct {
                    continue;
                }
                if source_ids.contains(wire.from.as_str()) {
                    queue_like.push(wire.from.as_str());
                    items = Some(resolution.get(&wire.from).expect("resolved above").clone());
                } else if let Some(upstream) = driving.get(wire.from.as_str()) {
                    if let Some(upstream_items) = upstream {
                        // Item-driven upstream: its items flow through.
                        queue_like.push(wire.from.as_str());
                        if items.is_none() {
                            items = Some(upstream_items.clone());
                        }
                    }
                    // A single-run upstream delivers one value: broadcast,
                    // not driving.
                }
            }
            if queue_like.len() > 1 {
                return Err(Error::Invalid(format!(
                    "process `{}` consumes {} item channels ({:?}); pair them with a \
                     join process first — one queue-like input per process",
                    id,
                    queue_like.len(),
                    queue_like
                )));
            }
            driving.insert(id, items);
        }
        for successor in successors.get(id).cloned().unwrap_or_default() {
            let degree = indegree.get_mut(successor).expect("validated topology");
            *degree -= 1;
            if *degree == 0 {
                queue.push_back(successor);
            }
        }
    }
    if settled != source_ids.len() + process_ids.len() {
        return Err(Error::Invalid("flow graph contains a cycle".into()));
    }

    // ── Materialize process instances ────────────────────────────────────
    let mut processes = BTreeMap::new();
    for process in graph.processes() {
        let driving_channel = driving.get(process.id.as_str()).cloned().flatten();
        let driven = driving_channel.is_some();
        let items = driving_channel.unwrap_or_default();
        let driving_keys: Vec<String> = items.iter().map(crate::instance::item_key).collect();
        let instances = if !driven {
            vec![Instance::singleton(process)?]
        } else {
            items
                .iter()
                .map(|item| Instance::for_item(process, item))
                .collect::<Result<Vec<_>, Error>>()?
        };
        processes.insert(
            process.id.clone(),
            ProcessPlan {
                process: process.id.clone(),
                kind: process.kind.clone(),
                spec: process.spec.clone(),
                error_strategy: process.error_strategy.clone(),
                driving_items: driving_keys,
                driven,
                instances,
            },
        );
    }

    // ── Routes ───────────────────────────────────────────────────────────
    let mut routes = Vec::new();
    for wire in graph.wires() {
        let to_plan = &processes[&wire.to];
        match wire.op {
            WireOp::Direct => {
                if let Some(source_plan) = sources.get(&wire.from) {
                    // Source item → one downstream instance per item.
                    for item in &source_plan.items {
                        let target = to_plan
                            .instance(&item.key)
                            .expect("downstream items derive from the same source");
                        routes.push(InstanceRoute {
                            from: item.id.clone(),
                            from_port: wire.from_port,
                            to: target.id.clone(),
                            to_port: wire.to_port,
                            op: wire.op,
                        });
                    }
                } else {
                    let from_plan = &processes[&wire.from];
                    if from_plan.is_driven() {
                        // Item-driven upstream: downstream shares the same
                        // items; pair per key.
                        for instance in &from_plan.instances {
                            let key = instance.item_key.as_deref().expect("driven instance");
                            let target = to_plan.instance(key).expect("items flow: same keys");
                            routes.push(InstanceRoute {
                                from: instance.id.clone(),
                                from_port: wire.from_port,
                                to: target.id.clone(),
                                to_port: wire.to_port,
                                op: wire.op,
                            });
                        }
                    } else {
                        // Single-run upstream: broadcast to every target
                        // instance (value-channel semantics).
                        let origin = &from_plan.instances[0];
                        for target in &to_plan.instances {
                            routes.push(InstanceRoute {
                                from: origin.id.clone(),
                                from_port: wire.from_port,
                                to: target.id.clone(),
                                to_port: wire.to_port,
                                op: wire.op,
                            });
                        }
                    }
                }
            }
            WireOp::Collect => {
                // Every upstream instance routes into the singleton's port.
                let target = &to_plan.instances[0];
                let origins: Vec<InstanceId> = if let Some(source_plan) = sources.get(&wire.from) {
                    source_plan
                        .items
                        .iter()
                        .map(|item| item.id.clone())
                        .collect()
                } else {
                    processes[&wire.from]
                        .instances
                        .iter()
                        .map(|instance| instance.id.clone())
                        .collect()
                };
                for origin in origins {
                    routes.push(InstanceRoute {
                        from: origin,
                        from_port: wire.from_port,
                        to: target.id.clone(),
                        to_port: wire.to_port,
                        op: wire.op,
                    });
                }
            }
        }
    }

    Ok(ExecutionPlan {
        sources,
        processes,
        routes,
    })
}

fn item_fingerprint(item: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(item).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ErrorStrategy, FlowGraphBuilder, Process, Source, Wire};

    fn items_flow() -> FlowGraph {
        FlowGraphBuilder::default()
            .source(Source::items(
                "reads",
                vec![serde_json::json!("a.fastq"), serde_json::json!("b.fastq")],
            ))
            .process(Process::new(
                "qc",
                "fastqc",
                serde_json::json!({ "input": "{{item}}" }),
            ))
            .process(Process::new("align", "bowtie", serde_json::json!({})))
            .process(Process::new("multiqc", "multiqc", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::direct("qc", 0, "align", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
            .build()
            .unwrap()
    }

    fn resolve(graph: &FlowGraph) -> SourceResolution {
        let mut resolution = SourceResolution::new();
        for source in graph.sources() {
            if let crate::model::SourceSpec::Items { values } = &source.spec {
                resolution.resolve(source.id.clone(), values.clone());
            }
        }
        resolution
    }

    #[test]
    fn instance_counts_are_derived_not_declared() {
        let graph = items_flow();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        // Source delivers 2 items → qc 2; align consumes qc's items → 2
        // (NOT re-declared); multiqc collects → 1.
        assert_eq!(plan.processes["qc"].instances.len(), 2);
        assert_eq!(plan.processes["align"].instances.len(), 2);
        assert_eq!(plan.processes["multiqc"].instances.len(), 1);
        assert_eq!(plan.sources["reads"].items.len(), 2);
        assert_eq!(plan.total_instances(), 5);
    }

    #[test]
    fn templates_render_from_the_flowing_item() {
        let graph = items_flow();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        let qc_a = plan.processes["qc"].instance("a.fastq").unwrap();
        assert_eq!(qc_a.rendered_spec["input"], serde_json::json!("a.fastq"));
        // align inherits the SAME item — it flows through the graph.
        let align_a = plan.processes["align"].instance("a.fastq").unwrap();
        assert_eq!(align_a.item_key.as_deref(), Some("a.fastq"));
    }

    #[test]
    fn routes_pair_items_and_collect() {
        let graph = items_flow();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();

        let source_to_qc: Vec<_> = plan
            .routes
            .iter()
            .filter(|r| r.from.as_str().starts_with("reads#") && r.to.as_str().starts_with("qc#"))
            .collect();
        assert_eq!(source_to_qc.len(), 2);

        let qc_to_align: Vec<_> = plan
            .routes
            .iter()
            .filter(|r| r.from.as_str().starts_with("qc#") && r.to.as_str().starts_with("align#"))
            .collect();
        assert_eq!(qc_to_align.len(), 2);
        assert!(
            qc_to_align
                .iter()
                .all(|r| r.from.parts().1 == r.to.parts().1)
        );

        let collected: Vec<_> = plan
            .routes
            .iter()
            .filter(|r| r.to.as_str() == "multiqc#0")
            .collect();
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn value_channel_broadcast_from_single_run_process() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "reads",
                vec![serde_json::json!("a"), serde_json::json!("b")],
            ))
            .process(Process::new("qc", "fastqc", serde_json::json!({})))
            .process(Process::new("ref", "index", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::direct("ref", 0, "qc", 1))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        // ref runs once; its single output broadcasts to every qc instance.
        let broadcasts: Vec<_> = plan
            .routes
            .iter()
            .filter(|r| r.from.as_str() == "ref#0")
            .collect();
        assert_eq!(broadcasts.len(), 2);
    }

    #[test]
    fn one_queue_like_input_per_process() {
        // A source AND an item-driven process both feed `m` — two queue-like
        // channels. (Two sources are rejected earlier by graph validation.)
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![serde_json::json!("x")]))
            .process(Process::new("qc", "fastqc", serde_json::json!({})))
            .process(Process::new("m", "merge", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::direct("reads", 0, "m", 0))
            .wire(Wire::direct("qc", 0, "m", 1))
            .build()
            .unwrap();
        let error = build_plan(&graph, &resolve(&graph)).unwrap_err();
        assert!(
            error.to_string().contains("one queue-like input"),
            "{error}"
        );
    }

    #[test]
    fn missing_resolution_entry_is_rejected() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![serde_json::json!("a")]))
            .process(Process::new("qc", "fastqc", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .build()
            .unwrap();
        let error = build_plan(&graph, &SourceResolution::new()).unwrap_err();
        assert!(
            error.to_string().contains("no resolved item set"),
            "{error}"
        );
    }

    #[test]
    fn duplicate_item_keys_are_rejected() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items(
                "reads",
                vec![serde_json::json!("a"), serde_json::json!("a")],
            ))
            .process(Process::new("qc", "fastqc", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .build()
            .unwrap();
        let error = build_plan(&graph, &resolve(&graph)).unwrap_err();
        assert!(error.to_string().contains("duplicate item key"), "{error}");
    }

    #[test]
    fn non_string_items_are_rejected() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![serde_json::json!(42)]))
            .process(Process::new("qc", "fastqc", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .build()
            .unwrap();
        let error = build_plan(&graph, &resolve(&graph)).unwrap_err();
        assert!(error.to_string().contains("file paths"), "{error}");
    }

    #[test]
    fn broken_templates_fail_the_plan() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![serde_json::json!("a")]))
            .process(Process::new(
                "qc",
                "fastqc",
                serde_json::json!({ "x": "{{upstream}}" }),
            ))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .build()
            .unwrap();
        let error = build_plan(&graph, &resolve(&graph)).unwrap_err();
        assert!(error.to_string().contains("unsupported token"), "{error}");
    }

    #[test]
    fn empty_source_is_an_empty_fanout() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![]))
            .process(Process::new("qc", "fastqc", serde_json::json!({})))
            .process(Process::new("multiqc", "multiqc", serde_json::json!({})))
            .wire(Wire::direct("reads", 0, "qc", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        assert_eq!(plan.processes["qc"].instances.len(), 0);
        assert_eq!(plan.processes["multiqc"].instances.len(), 1);
        assert_eq!(plan.routes.len(), 0);
    }

    #[test]
    fn error_strategy_flows_into_the_plan() {
        let mut qc = Process::new("qc", "fastqc", serde_json::json!({}));
        qc.error_strategy = ErrorStrategy::retry(2, 10);
        let graph = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![serde_json::json!("a")]))
            .process(qc)
            .wire(Wire::direct("reads", 0, "qc", 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        assert_eq!(plan.processes["qc"].error_strategy.max_retries, 2);
    }

    #[test]
    fn plan_serializes_for_run_records() {
        let graph = items_flow();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        let json = serde_json::to_value(&plan).unwrap();
        let restored: ExecutionPlan = serde_json::from_value(json).unwrap();
        assert_eq!(restored.total_instances(), 5);
        assert_eq!(restored.routes.len(), 6);
    }
}
