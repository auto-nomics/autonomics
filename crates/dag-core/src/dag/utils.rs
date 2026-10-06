use std::collections::VecDeque;

use datafusion::common::HashMap;

use crate::dag::{
    NodeId, NodeInput, RuntimeStatus,
    graph::{EdgeLabel, PortOutputs},
    runtime::InputBinding,
};
use crate::value::{FileFingerprint, NodeValue};

/// Gather a node's predecessor outputs into [`NodeInput`]s, one per connected
/// input port, in declared edge order. DataFrame handles remain cheap to clone;
/// file values are path references.
///
/// Each edge routes exactly the DataFrame produced on its `from_port`. The
/// injected `NodeInput.port` is the edge's `to_port`, and `df_name` is a
/// globally-unique table name (`"{from}__{from_port}__{to}"`) so it never
/// collides in the shared `SessionContext`.
pub fn build_inputs(
    id: &str,
    incoming: &HashMap<NodeId, Vec<(NodeId, EdgeLabel)>>,
    outputs: &HashMap<NodeId, PortOutputs>,
) -> Vec<NodeInput> {
    let mut inputs = Vec::new();
    if let Some(edges) = incoming.get(id) {
        for (from, edge) in edges {
            let Some(pred_outputs) = outputs.get(from) else {
                continue;
            };
            // Pull exactly the DataFrame produced on this edge's output port.
            let Some(value) = pred_outputs.get(&edge.from_port) else {
                continue;
            };
            inputs.push(NodeInput {
                port: edge.to_port,
                // Minimal globally-unique table name: (to_node, to_port) uniquely
                // identifies an edge under strict 1:1, so this never collides in
                // the shared SessionContext.
                // df_name: edge.to_port.clone(),
                data: value.clone(),
            });
        }
    }
    inputs
}

/// Record the resolved upstream bindings of a node at dispatch time — the
/// audit-side counterpart of [`build_inputs`]. Mirrors its edge iteration
/// (including the "upstream output not yet produced / port absent → skip"
/// semantics) so the recorded set matches exactly what was injected.
pub fn build_input_bindings(
    id: &str,
    incoming: &HashMap<NodeId, Vec<(NodeId, EdgeLabel)>>,
    outputs: &HashMap<NodeId, PortOutputs>,
) -> Vec<InputBinding> {
    let mut bindings = Vec::new();
    let Some(edges) = incoming.get(id) else {
        return bindings;
    };
    for (from, edge) in edges {
        let Some(pred_outputs) = outputs.get(from) else {
            continue;
        };
        let Some(value) = pred_outputs.get(&edge.from_port) else {
            continue;
        };
        bindings.push(InputBinding {
            from: from.clone(),
            from_port: edge.from_port,
            to_port: edge.to_port,
            kind: value_kind(value).to_string(),
            path: bound_path(value),
            fingerprint: bound_fingerprint(value).cloned(),
        });
    }
    bindings
}

/// Variant name of a [`NodeValue`], for audit records.
fn value_kind(value: &NodeValue) -> &'static str {
    match value {
        NodeValue::DataFrame(_) => "DataFrame",
        NodeValue::File(_) => "File",
        NodeValue::FileSet(_) => "FileSet",
        NodeValue::Channel(_) => "Channel",
    }
}

/// First path/URI of a file-like value (`None` for DataFrame handles, which
/// have no stable address).
fn bound_path(value: &NodeValue) -> Option<String> {
    match value {
        NodeValue::File(file) => Some(file.path.clone()),
        NodeValue::FileSet(files) => files.first().map(|file| file.path.clone()),
        NodeValue::DataFrame(_) => None,
        NodeValue::Channel(_) => None,
    }
}

/// Fingerprint of the first entry of a file-like value, when recorded.
fn bound_fingerprint(value: &NodeValue) -> Option<&FileFingerprint> {
    match value {
        NodeValue::File(file) => file.fingerprint.as_ref(),
        NodeValue::FileSet(files) => files.first().and_then(|file| file.fingerprint.as_ref()),
        NodeValue::DataFrame(_) => None,
        NodeValue::Channel(_) => None,
    }
}

/// Mark every transitive descendant of `failed` as [`RuntimeStatus::Skipped`].
/// Stops at nodes that already have a terminal status so independent branches
/// keep running.
///
/// `skipped_because` records the *root-cause* failed node id for every skipped
/// descendant so agents can trace the failure chain.
pub fn cascade_skip(
    failed: &str,
    successors: &HashMap<NodeId, Vec<NodeId>>,
    statuses: &mut HashMap<NodeId, RuntimeStatus>,
    ready: &mut VecDeque<NodeId>,
    skipped_because: &mut HashMap<NodeId, NodeId>,
) {
    let mut queue: VecDeque<NodeId> = successors
        .get(failed)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    while let Some(id) = queue.pop_front() {
        if statuses[&id] != RuntimeStatus::Pending {
            continue;
        }
        statuses.insert(id.clone(), RuntimeStatus::Skipped);
        skipped_because.insert(id.clone(), failed.to_string());
        ready.retain(|r| r != &id);
        if let Some(succs) = successors.get(&id) {
            for s in succs {
                queue.push_back(s.clone());
            }
        }
    }
}
