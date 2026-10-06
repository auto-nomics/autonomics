//! Structural mutation: node/edge add/delete/replace, full reset, and the
//! incremental-execution fingerprint state.
//!
//! Mutating a node or an edge drops the affected node's recorded fingerprint
//! (see [`DAG::mark_dirty`]); descendants are never eagerly invalidated —
//! their identities chain through upstream fingerprints and re-evaluate at
//! dispatch time.

use petgraph::Direction;
use petgraph::algo::has_path_connecting;
use petgraph::visit::EdgeRef;

use super::{DAG, EdgeLabel, PortOutputs, Result};
use crate::dag::error::DagError;
use crate::dag::runtime::RuntimeStatus;
use crate::dag::{DagNode, NodeId};
use crate::value::{FileRef, NodeValue};

impl DAG {
    /// Register a node under `id`. Errors if the id is already taken.
    pub fn add_node(&mut self, id: NodeId, node: Box<dyn DagNode>) -> Result<()> {
        if self.nodes.contains_key(&id) {
            return Err(DagError::DuplicateNode(id));
        }
        let idx = self.graph.add_node(id.clone());
        self.id_to_idx.insert(id.clone(), idx);
        self.nodes.insert(id.clone(), node);
        // New node has no recorded fingerprint — must be executed.
        self.fingerprints.remove(&id);
        Ok(())
    }

    /// Like [`Self::add_node`] but also retains the `(kind, spec)` pair so the
    /// node can be serialized into a manifest for snapshot persistence.
    pub fn add_node_with_spec(
        &mut self,
        id: NodeId,
        node: Box<dyn DagNode>,
        kind: String,
        spec: serde_json::Value,
    ) -> Result<()> {
        self.add_node(id.clone(), node)?;
        self.specs.insert(id, (kind, spec));
        Ok(())
    }

    /// Add an edge from `from`'s `from_port` output port to `to`'s `to_port`
    /// input port. Enforces the strict 1:1 rule on declared input ports at
    /// insertion time (does not defer to [`Self::validate`]).
    ///
    /// Both endpoints must name ports the nodes actually declare (variadic
    /// targets may extend beyond their declared input ports) — an edge to a
    /// nonexistent port is rejected immediately rather than being accepted
    /// silently and later delivering no input.
    pub fn add_edge(
        &mut self,
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
        from_port: u8,
        to_port: u8,
    ) -> Result<()> {
        let from = from.into();
        let to = to.into();
        self.resolve_nodes(&from, &to)?;
        if self.nodes[&from].is_terminal() {
            return Err(DagError::Schedule(format!(
                "node `{from}` is terminal and cannot have downstream edges"
            )));
        }

        // Port existence — reject out-of-range indices here instead of
        // letting the edge validate but never deliver a value.
        if self.nodes[&from].ports().output_port(from_port).is_none() {
            return Err(DagError::PortNotFound {
                node: from.clone(),
                port: from_port,
                direction: "output",
            });
        }
        let to_ports = self.nodes[&to].ports();
        if to_ports.is_fixed_input() && to_ports.input_port(to_port).is_none() {
            return Err(DagError::PortNotFound {
                node: to.clone(),
                port: to_port,
                direction: "input",
            });
        }

        // Enforce strict 1:1 on declared input ports at edge-insertion time.
        if to_ports.is_fixed_input() && to_ports.input_port(to_port).is_some() {
            self.ensure_port_available(&to, to_port)?;
        }

        // Validate schema compatibility for this edge before inserting it.
        self.validate_edge_schema(&from, from_port, &to, to_port)?;

        if let (Some(&a), Some(&b)) = (self.id_to_idx.get(&from), self.id_to_idx.get(&to)) {
            // Reject if adding `from -> to` would close a cycle, i.e. `to` can
            // already reach `from` (also covers the self-loop case where from==to).
            if has_path_connecting(&self.graph, b, a, None) {
                return Err(DagError::Cycle(format!(
                    "adding edge {from} -> {to} would create a cycle"
                )));
            }
            self.graph.add_edge(a, b, EdgeLabel { from_port, to_port });
        }
        // The target node's input identity changed — drop its fingerprint so
        // it re-executes. Descendants re-evaluate through the identity chain.
        self.mark_dirty(&to);
        Ok(())
    }

    fn resolve_nodes(&self, from: &str, to: &str) -> Result<()> {
        if !self.nodes.contains_key(from) {
            return Err(DagError::UnknownNode(from.to_string()));
        }

        if !self.nodes.contains_key(to) {
            return Err(DagError::UnknownNode(to.to_string()));
        };

        Ok(())
    }

    /// Reject if `node`'s input `port` already has an incoming edge.
    fn ensure_port_available(&self, node: &str, port: u8) -> Result<()> {
        let Some(&idx) = self.id_to_idx.get(node) else {
            return Ok(());
        };
        if self
            .graph
            .edges_directed(idx, Direction::Incoming)
            .any(|e| e.weight().to_port == port)
        {
            return Err(DagError::PortOverconnected {
                node: node.to_string(),
                port,
            });
        }
        Ok(())
    }

    pub fn delete_node(&mut self, id: &str) -> Result<()> {
        let target_node_idx = *self
            .id_to_idx
            .get(id)
            .ok_or_else(|| DagError::UnknownNode(id.to_string()))?;
        let successors = self.successors(id);
        if !successors.is_empty() {
            return Err(DagError::Schedule(format!(
                "Cannot delete node `{id}`: the following successor node(s) still depend on it: [{}]. \
                 Remove those nodes (or their incoming edges) first.",
                successors.join(", ")
            )));
        }
        self.graph.remove_node(target_node_idx);
        self.nodes.remove(id);
        self.id_to_idx.remove(id);
        // petgraph's Graph uses swap-remove: if the removed node wasn't the
        // last, the trailing node was moved into its slot, changing its
        // NodeIndex.  Update the id → index mapping for the swapped node.
        if let Some(swapped_id) = self.graph.node_weight(target_node_idx) {
            self.id_to_idx.insert(swapped_id.clone(), target_node_idx);
        }
        self.statuses.remove(id);
        self.outputs.remove(id);
        self.specs.remove(id);
        self.fingerprints.remove(id);
        if let Some(job) = self.physical_jobs.remove(id) {
            self.logical_graphs.retain(|graph| {
                let contains_deleted_source =
                    graph.nodes().iter().any(|node| node.id == job.logical_node);
                let has_remaining_job = self.physical_jobs.values().any(|remaining| {
                    graph
                        .nodes()
                        .iter()
                        .any(|node| node.id == remaining.logical_node)
                });
                !contains_deleted_source || has_remaining_job
            });
        }
        Ok(())
    }

    /// Remove the edge from `from`'s `from_port` to `to`'s `to_port`.
    ///
    /// Returns [`DagError::UnknownNode`] if either endpoint does not exist, or
    /// [`DagError::EdgeNotFound`] if no matching edge is present.
    pub fn delete_edge(
        &mut self,
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
        from_port: u8,
        to_port: u8,
    ) -> Result<()> {
        let from = from.into();
        let to = to.into();
        self.resolve_nodes(&from, &to)?;

        let &a = self
            .id_to_idx
            .get(&from)
            .ok_or_else(|| DagError::CannotResolveNodeIdx {
                node_id: from.clone(),
            })?;
        let &b = self
            .id_to_idx
            .get(&to)
            .ok_or_else(|| DagError::CannotResolveNodeIdx {
                node_id: to.clone(),
            })?;

        let edge_id = self
            .graph
            .edges_connecting(a, b)
            .find(|e| e.weight().from_port == from_port && e.weight().to_port == to_port)
            .map(|e| e.id());

        match edge_id {
            Some(id) => {
                self.graph.remove_edge(id);
                // The target lost an input — its identity changed, so drop
                // its fingerprint. Descendants re-evaluate naturally.
                self.mark_dirty(&to);
                Ok(())
            }
            None => Err(DagError::EdgeNotFound {
                from,
                from_port,
                to,
                to_port,
            }),
        }
    }

    /// Replace the payload of an existing node, keeping its id, `NodeIndex`,
    /// and all incoming/outgoing edges intact.
    ///
    /// Before swapping, every edge touching this node is re-validated:
    ///   - the port referenced by the edge must still exist on the new node's
    ///     `NodePorts`
    ///   - if both endpoints declare a port schema, they must remain compatible
    ///
    /// If any edge fails validation the replacement is rejected and the old
    /// payload is left untouched (atomic — no partial state).
    pub fn replace_node(&mut self, id: &str, new_node: Box<dyn DagNode>) -> Result<()> {
        let &idx = self
            .id_to_idx
            .get(id)
            .ok_or_else(|| DagError::UnknownNode(id.to_string()))?;

        let new_ports = new_node.ports();

        // Validate incoming edges: each upstream's from_port must exist on the
        // source node (unchanged) and the to_port must exist on the new node.
        for e in self.graph.edges_directed(idx, Direction::Incoming) {
            let from = &self.graph[e.source()];
            let label = e.weight();
            let from_node = self.nodes.get(from).map(|b| b.as_ref());
            let Some(from_node) = from_node else {
                continue;
            };
            let from_ports = from_node.ports();

            // Source port still exists.
            if from_ports.output_port(label.from_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: from.clone(),
                    port: label.from_port,
                    direction: "output",
                });
            }
            // Target port must exist on the new node.
            if new_ports.is_fixed_input() && new_ports.input_port(label.to_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: id.to_string(),
                    port: label.to_port,
                    direction: "input",
                });
            }
            // Schema compatibility.
            self.validate_edge_schema(from, label.from_port, id, label.to_port)?;
        }

        // Validate outgoing edges: each from_port must exist on the new node
        // and the target port must still exist on the downstream node.
        for e in self.graph.edges_directed(idx, Direction::Outgoing) {
            let to = &self.graph[e.target()];
            let label = e.weight();
            let to_node = self.nodes.get(to).map(|b| b.as_ref());
            let Some(to_node) = to_node else {
                continue;
            };
            let to_ports = to_node.ports();

            // Source port must exist on the new node.
            if new_ports.output_port(label.from_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: id.to_string(),
                    port: label.from_port,
                    direction: "output",
                });
            }
            // Target port still exists downstream.
            if to_ports.is_fixed_input() && to_ports.input_port(label.to_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: to.clone(),
                    port: label.to_port,
                    direction: "input",
                });
            }
            // Schema compatibility.
            self.validate_edge_schema(id, label.from_port, to, label.to_port)?;
        }

        // All edges valid — swap the payload.
        self.nodes.insert(id.to_string(), new_node);

        // Old outputs are stale; clear them so a re-run produces fresh results.
        self.outputs.remove(id);
        self.errors.remove(id);
        self.statuses.insert(id.to_string(), RuntimeStatus::Pending);
        // The node's identity changed — drop its fingerprint. Descendants
        // re-evaluate through the identity chain at dispatch.
        self.mark_dirty(id);
        Ok(())
    }

    /// Like [`Self::replace_node`] but also updates the retained spec, so the
    /// manifest stays in sync after an `update_node` operation.
    pub fn replace_node_with_spec(
        &mut self,
        id: &str,
        new_node: Box<dyn DagNode>,
        kind: String,
        spec: serde_json::Value,
    ) -> Result<()> {
        self.replace_node(id, new_node)?;
        self.specs.insert(id.to_string(), (kind, spec));
        Ok(())
    }

    /// Remove all nodes, edges, statuses, outputs, and errors — a full reset.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.graph.clear();
        self.id_to_idx.clear();
        self.statuses.clear();
        self.outputs.clear();
        self.errors.clear();
        self.specs.clear();
        self.fingerprints.clear();
        self.input_bindings.clear();
        self.node_run_details.clear();
        self.physical_jobs.clear();
        self.logical_graphs.clear();
    }

    /// Reset all node statuses to [`RuntimeStatus::Pending`] and drop every
    /// recorded fingerprint, preparing for a full re-run.
    pub fn reset(&mut self) {
        for id in self.nodes.keys() {
            self.statuses.insert(id.clone(), RuntimeStatus::Pending);
        }
        self.mark_all_dirty();
    }

    // ── incremental-execution API (fingerprint reuse) ───────────────────

    /// Whether `id` will re-execute on the next incremental run, to the
    /// extent knowable without dispatching: a node is reusable only when it
    /// has both a recorded fingerprint and cached outputs.
    ///
    /// This is a cheap approximation — a node whose *upstream* identity has
    /// changed stays `false` until dispatch computes its candidate
    /// fingerprint and finds the mismatch (and if the upstream reproduces
    /// identical outputs, the node is correctly *not* re-executed).
    pub fn is_dirty(&self, id: &str) -> bool {
        !(self.fingerprints.contains_key(id) && self.outputs.contains_key(id))
    }

    /// Drop `id`'s recorded fingerprint, forcing its re-execution on the next
    /// incremental run.
    ///
    /// Descendants are deliberately **not** touched: their identities chain
    /// through this node's fingerprint / output content, so they re-evaluate
    /// naturally at dispatch — and are correctly reused when this node
    /// reproduces identical outputs. Use this when an external input (file,
    /// VFS dataset, API response) has changed outside the engine.
    pub fn mark_dirty(&mut self, id: &str) {
        self.fingerprints.remove(id);
    }

    /// Drop **every** recorded fingerprint — forces a full re-run on the next
    /// incremental `run`. Equivalent to the default (non-incremental) behavior.
    pub fn mark_all_dirty(&mut self) {
        self.fingerprints.clear();
    }

    /// Restore a successful file-producing node from a persisted checkpoint.
    ///
    /// DataFrame and Channel outputs are not reconstructible from a run
    /// report; those nodes remain dirty and execute on the next incremental
    /// run. File/FileSet outputs, which dominate container and assay jobs,
    /// can be restored without rerunning their producers.
    pub fn restore_cached_file_outputs(
        &mut self,
        id: &str,
        fingerprint: &str,
        outputs: Vec<(u8, FileRef)>,
    ) -> Result<()> {
        if !self.nodes.contains_key(id) {
            return Err(DagError::UnknownNode(id.to_string()));
        }
        if outputs.is_empty() {
            return Ok(());
        }

        let mut restored = PortOutputs::new();
        for (port, file) in outputs {
            restored.insert(port, file);
        }
        self.outputs.insert(id.to_string(), restored);
        self.fingerprints
            .insert(id.to_string(), fingerprint.to_string());
        self.statuses.insert(id.to_string(), RuntimeStatus::Success);
        self.errors.remove(id);
        Ok(())
    }

    /// Drop the recorded fingerprints of nodes whose cached file outputs no
    /// longer match their recorded identity.
    ///
    /// Three-stage freshness per file: declared-immutable remotes are clean
    /// outright; matching size + mtime is clean; a recorded `sha256:` hash is
    /// re-computed and compared so a touched-but-unchanged file stays clean.
    pub(super) async fn invalidate_stale_file_outputs(
        &mut self,
        storage: Option<&vfs::OpendalFileStorage>,
    ) {
        let mut stale = Vec::new();
        for (id, outputs) in self.outputs.iter() {
            let mut changed = false;
            for value in outputs.values() {
                match value {
                    NodeValue::File(file) => {
                        if crate::fingerprint::cached_file_changed(file, storage).await {
                            changed = true;
                        }
                    }
                    NodeValue::FileSet(files) => {
                        for file in files {
                            if crate::fingerprint::cached_file_changed(file, storage).await {
                                changed = true;
                            }
                        }
                    }
                    NodeValue::DataFrame(_) => {}
                    NodeValue::Channel(_) => {}
                }
                if changed {
                    break;
                }
            }
            if changed {
                stale.push(id.clone());
            }
        }

        for id in stale {
            // The cached outputs no longer match their recorded identity:
            // drop the fingerprint so the node re-executes. Descendants
            // re-evaluate through the identity chain — if this node
            // reproduces identical outputs, they stay reused.
            self.fingerprints.remove(&id);
        }
    }
}
