//! Run-start planning: resolve sources, materialize instances, compute routes.
//!
//! [`build_plan`] is a **pure function** of the authored graph plus the
//! resolved item sets: all enumeration I/O (glob listing, directory scans)
//! belongs to the scheduler, which hands the results in via
//! [`SourceResolution`]. Everything the executor later needs — the full
//! instance set with rendered specs, and the instance-to-instance routing —
//! is decided here, before anything dispatches, so structural failures
//! (mismatched item sets, duplicate keys, broken templates) fail the run at
//! start instead of mid-flight.
//!
//! # Routing semantics (Nextflow correspondences)
//!
//! | Wire | Rule | Nextflow analogue |
//! |---|---|---|
//! | `Direct` fanout→fanout | zip by item key; key sets must be **equal** | channel 1:1 pairing |
//! | `Direct` singleton→fanout | broadcast to every target instance | value broadcast |
//! | `Direct` singleton→singleton | plain 1:1 edge | plain channel |
//! | `Collect` fanout→singleton | every source instance routes into the target's single input port | `collect` operator |
//!
//! Not in v1: cartesian pairing of mismatched fanouts (Nextflow `combine`,
//! `mix`). Item sets that do not agree are an error, not a cross product.

use std::collections::{BTreeMap, BTreeSet};

use crate::instance::{Instance, InstanceId};
use crate::model::{Error, FlowGraph, PortIndex, ProcessName, WireOp};

/// Item sets for every fanout process, enumerated by the scheduler at run
/// start. Must contain an entry for **every** fanout process (uniform for
/// `Glob` and `Items` sources) and no entry for anything else — strictness
/// here surfaces stale caller state immediately.
#[derive(Debug)]
pub struct SourceResolution(BTreeMap<ProcessName, Vec<serde_json::Value>>);

impl SourceResolution {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Record the enumerated items of one fanout process. Duplicate process
    /// names overwrite, matching the caller's "one authoritative listing"
    /// contract.
    pub fn resolve(&mut self, process: impl Into<ProcessName>, items: Vec<serde_json::Value>) {
        self.0.insert(process.into(), items);
    }

    fn get(&self, process: &str) -> Option<&Vec<serde_json::Value>> {
        self.0.get(process)
    }

    fn keys(&self) -> impl Iterator<Item = &ProcessName> {
        self.0.keys()
    }
}

impl Default for SourceResolution {
    fn default() -> Self {
        Self::new()
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
}

/// One process's materialized instance set for this run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProcessPlan {
    pub process: ProcessName,
    /// The node-registry kind every instance of this process builds from.
    pub kind: String,
    /// In item order given by the resolution (stable input order, not
    /// key-sorted — reports read better when they match the listing).
    pub instances: Vec<Instance>,
}

impl ProcessPlan {
    /// Item keys in resolution order. Empty key string is the singleton's
    /// marker.
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
}

/// The complete run-start plan: every instance that will exist this run, and
/// every edge between them. Serializable so run records can carry the exact
/// expansion next to the authored graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionPlan {
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

    /// Routes leaving one instance, in plan order.
    pub fn routes_from(&self, id: &InstanceId) -> impl Iterator<Item = &InstanceRoute> {
        self.routes.iter().filter(move |route| &route.from == id)
    }

    /// Routes entering one instance, in plan order.
    pub fn routes_to(&self, id: &InstanceId) -> impl Iterator<Item = &InstanceRoute> {
        self.routes.iter().filter(move |route| &route.to == id)
    }
}

/// Cap on how many mismatched keys an error message enumerates before
/// switching to counts.
const KEY_MISMATCH_LISTED: usize = 8;

/// Build the execution plan.
///
/// Steps, all pure:
/// 1. re-validate the authored graph (cheap; guards callers that skipped it);
/// 2. materialize singleton instances and fanout instances (rendering every
///    spec template — a broken template fails the whole run here);
/// 3. check every fanout process has exactly one resolution entry and that
///    derived item keys are unique within the set;
/// 4. compute routes per the wire rules in the module docs.
pub fn build_plan(
    graph: &FlowGraph,
    resolution: &SourceResolution,
) -> Result<ExecutionPlan, Error> {
    graph.validate()?;

    // ── Materialize instances ────────────────────────────────────────────
    let mut processes = BTreeMap::new();
    for process in graph.processes() {
        let instances = if process.cardinality.is_fanout() {
            let items = resolution.get(&process.id).ok_or_else(|| {
                Error::Invalid(format!(
                    "fanout process `{}` has no resolved item set",
                    process.id
                ))
            })?;
            let mut seen_keys = BTreeSet::new();
            let mut instances = Vec::with_capacity(items.len());
            for item in items {
                let instance = Instance::for_item(process, item)?;
                let key = instance.item_key.clone().expect("fanout instances carry keys");
                if !seen_keys.insert(key.clone()) {
                    return Err(Error::Invalid(format!(
                        "fanout process `{}` derives duplicate item key `{key}`; \
                         two items would silently merge into one instance",
                        process.id
                    )));
                }
                instances.push(instance);
            }
            instances
        } else {
            vec![Instance::singleton(process)?]
        };
        processes.insert(
            process.id.clone(),
            ProcessPlan {
                process: process.id.clone(),
                kind: process.kind.clone(),
                instances,
            },
        );
    }

    // Resolution entries must correspond to actual fanout processes; extra
    // entries mean the caller resolved a stale graph.
    for name in resolution.keys() {
        let fanout = graph.process(name).is_some_and(|process| process.cardinality.is_fanout());
        if !fanout {
            return Err(Error::Invalid(format!(
                "source resolution contains `{name}`, which is not a fanout process of this graph"
            )));
        }
    }

    // ── Routes ───────────────────────────────────────────────────────────
    let mut routes = Vec::new();
    for wire in graph.wires() {
        let from_plan = &processes[&wire.from];
        let to_plan = &processes[&wire.to];
        match wire.op {
            WireOp::Direct => {
                let from_fanout = from_plan.instances.iter().any(|i| i.item_key.is_some());
                let to_fanout = to_plan.instances.iter().any(|i| i.item_key.is_some());
                match (from_fanout, to_fanout) {
                    (true, true) => {
                        // Zip by item key: the sets must agree exactly.
                        let from_keys: BTreeSet<&str> = from_plan
                            .instances
                            .iter()
                            .filter_map(|i| i.item_key.as_deref())
                            .collect();
                        let to_keys: BTreeSet<&str> = to_plan
                            .instances
                            .iter()
                            .filter_map(|i| i.item_key.as_deref())
                            .collect();
                        if from_keys != to_keys {
                            let missing: Vec<&str> =
                                from_keys.difference(&to_keys).copied().collect();
                            let extra: Vec<&str> = to_keys.difference(&from_keys).copied().collect();
                            return Err(Error::Invalid(format!(
                                "Direct wire {} -> {} connects fanouts with mismatched item keys \
                                 (missing on target: {:?}; unexpected on target: {:?})",
                                wire.from,
                                wire.to,
                                preview(&missing),
                                preview(&extra),
                            )));
                        }
                        for instance in &from_plan.instances {
                            let key = instance.item_key.as_deref().expect("fanout instance");
                            let target = to_plan.instance(key).expect("key sets are equal");
                            routes.push(InstanceRoute {
                                from: instance.id.clone(),
                                from_port: wire.from_port,
                                to: target.id.clone(),
                                to_port: wire.to_port,
                            });
                        }
                    }
                    (false, _) => {
                        // Singleton source: broadcast (a no-op fan-out of one
                        // edge when the target is also a singleton).
                        let source = &from_plan.instances[0];
                        for target in &to_plan.instances {
                            routes.push(InstanceRoute {
                                from: source.id.clone(),
                                from_port: wire.from_port,
                                to: target.id.clone(),
                                to_port: wire.to_port,
                            });
                        }
                    }
                    // Model validation already rejected fanout→singleton Direct.
                    (true, false) => unreachable!("validated: Direct fanout→singleton"),
                }
            }
            WireOp::Collect => {
                // Every source instance routes into the singleton's one port.
                let target = &to_plan.instances[0];
                for instance in &from_plan.instances {
                    routes.push(InstanceRoute {
                        from: instance.id.clone(),
                        from_port: wire.from_port,
                        to: target.id.clone(),
                        to_port: wire.to_port,
                    });
                }
            }
        }
    }

    Ok(ExecutionPlan { processes, routes })
}

/// Short human-facing preview of a key list: the first few entries, then a
/// count, so a 100-file mismatch stays a one-line error.
fn preview(keys: &[&str]) -> Vec<String> {
    let mut shown: Vec<String> = keys
        .iter()
        .take(KEY_MISMATCH_LISTED)
        .map(|key| (*key).to_string())
        .collect();
    let hidden = keys.len().saturating_sub(KEY_MISMATCH_LISTED);
    if hidden > 0 {
        shown.push(format!("… and {hidden} more"));
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FlowGraphBuilder, Process, Source, Wire};

    fn fanout(id: &str, items: Vec<serde_json::Value>) -> Process {
        Process::fanout(
            id,
            "tool",
            serde_json::json!({ "input": "{{item}}" }),
            Source::items(items),
            "sample",
        )
    }

    fn pipeline() -> FlowGraph {
        FlowGraphBuilder::default()
            .process(fanout("qc", vec![serde_json::json!("a"), serde_json::json!("b")]))
            .process(fanout(
                "align",
                vec![serde_json::json!("a"), serde_json::json!("b")],
            ))
            .process(Process::singleton(
                "multiqc",
                "multiqc",
                serde_json::json!({}),
            ))
            .wire(Wire::direct("qc", 0, "align", 0))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
            .build()
            .unwrap()
    }

    fn resolve(graph: &FlowGraph) -> SourceResolution {
        let mut resolution = SourceResolution::new();
        for process in graph.processes() {
            if let crate::model::Cardinality::Fanout { source, .. } = &process.cardinality {
                if let Source::Items { values } = source {
                    resolution.resolve(process.id.clone(), values.clone());
                }
            }
        }
        resolution
    }

    #[test]
    fn pipeline_plan_materializes_and_routes() {
        let graph = pipeline();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();

        assert_eq!(plan.total_instances(), 5); // 2 qc + 2 align + 1 multiqc
        let qc = &plan.processes["qc"];
        assert_eq!(qc.keys(), vec!["a", "b"]);
        // Templates are rendered at plan time.
        assert_eq!(qc.instances[0].rendered_spec["input"], serde_json::json!("a"));

        // Direct qc→align zips by key.
        let qc_align: Vec<_> = plan
            .routes
            .iter()
            .filter(|route| route.from.as_str().starts_with("qc#") && route.to.as_str().starts_with("align#"))
            .collect();
        assert_eq!(qc_align.len(), 2);
        assert!(qc_align
            .iter()
            .all(|route| route.from.as_str().split('#').nth(1)
                == route.to.as_str().split('#').nth(1)));

        // Collect qc→multiqc collapses both instances into one port.
        let collected: Vec<_> = plan
            .routes
            .iter()
            .filter(|route| route.to.as_str() == "multiqc#0")
            .collect();
        assert_eq!(collected.len(), 2);
        assert!(collected.iter().all(|route| route.to_port == 0));
    }

    #[test]
    fn singleton_broadcasts_into_fanout() {
        let graph = FlowGraphBuilder::default()
            .process(Process::singleton("ref", "ref", serde_json::json!({})))
            .process(fanout("align", vec![serde_json::json!("a"), serde_json::json!("b")]))
            .wire(Wire::direct("ref", 0, "align", 1))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        let routes: Vec<_> = plan.routes_from(&InstanceId::new("ref", None)).collect();
        assert_eq!(routes.len(), 2);
        assert!(routes.iter().all(|route| route.to_port == 1));
    }

    #[test]
    fn singleton_to_singleton_is_one_edge() {
        let graph = FlowGraphBuilder::default()
            .process(Process::singleton("a", "t", serde_json::json!({})))
            .process(Process::singleton("b", "t", serde_json::json!({})))
            .wire(Wire::direct("a", 0, "b", 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &SourceResolution::new()).unwrap();
        assert_eq!(plan.routes.len(), 1);
        assert_eq!(plan.total_instances(), 2);
    }

    #[test]
    fn mismatched_item_sets_are_rejected_with_a_preview() {
        let graph = FlowGraphBuilder::default()
            .process(fanout("qc", vec![serde_json::json!("a")]))
            .process(fanout("align", vec![serde_json::json!("b")]))
            .wire(Wire::direct("qc", 0, "align", 0))
            .build()
            .unwrap();
        let error = build_plan(&graph, &resolve(&graph)).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("mismatched item keys"), "{message}");
        assert!(message.contains("a"), "{message}");
    }

    #[test]
    fn duplicate_derived_keys_are_rejected() {
        let graph = FlowGraphBuilder::default()
            .process(fanout(
                "qc",
                vec![serde_json::json!("a"), serde_json::json!("a")],
            ))
            .build()
            .unwrap();
        let error = build_plan(&graph, &resolve(&graph)).unwrap_err();
        assert!(
            error.to_string().contains("duplicate item key"),
            "{error}"
        );
    }

    #[test]
    fn missing_resolution_entry_is_rejected() {
        let graph = FlowGraphBuilder::default()
            .process(fanout("qc", vec![serde_json::json!("a")]))
            .build()
            .unwrap();
        let error = build_plan(&graph, &SourceResolution::new()).unwrap_err();
        assert!(
            error.to_string().contains("no resolved item set"),
            "{error}"
        );
    }

    #[test]
    fn stale_resolution_entries_are_rejected() {
        let graph = FlowGraphBuilder::default()
            .process(Process::singleton("a", "t", serde_json::json!({})))
            .build()
            .unwrap();
        let mut resolution = SourceResolution::new();
        resolution.resolve("ghost", vec![serde_json::json!(1)]);
        let error = build_plan(&graph, &resolution).unwrap_err();
        assert!(
            error.to_string().contains("not a fanout process"),
            "{error}"
        );
    }

    #[test]
    fn broken_templates_fail_the_plan_not_the_dispatch() {
        let mut process = fanout("qc", vec![serde_json::json!("a")]);
        process.spec = serde_json::json!({ "input": "{{upstream}}" });
        let graph = FlowGraphBuilder::default().process(process).build().unwrap();
        let error = build_plan(&graph, &resolve(&graph)).unwrap_err();
        assert!(error.to_string().contains("unsupported token"), "{error}");
    }

    #[test]
    fn empty_item_sets_produce_zero_instances_and_no_routes() {
        let graph = FlowGraphBuilder::default()
            .process(fanout("qc", vec![]))
            .process(Process::singleton("multiqc", "t", serde_json::json!({})))
            .wire(Wire::collect("qc", 0, "multiqc", 0))
            .build()
            .unwrap();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        assert_eq!(plan.processes["qc"].instances.len(), 0);
        assert_eq!(plan.routes.len(), 0);
        assert_eq!(plan.total_instances(), 1);
    }

    #[test]
    fn plan_serializes_for_run_records() {
        let graph = pipeline();
        let plan = build_plan(&graph, &resolve(&graph)).unwrap();
        let json = serde_json::to_value(&plan).unwrap();
        let restored: ExecutionPlan = serde_json::from_value(json).unwrap();
        assert_eq!(restored.total_instances(), 5);
        assert_eq!(restored.routes.len(), 4);
    }
}
