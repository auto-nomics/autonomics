//! Dynamic fanout: expanding a `dynamic_fanout` coordinator node into one
//! physical job per discovered channel item, at run time.
//!
//! Called from the scheduler loop ([`super::scheduler`]) when a coordinator
//! becomes ready, and from [`super::streaming`] when a streaming predecessor
//! delivers an item to a coordinator.

use std::collections::{BTreeMap, VecDeque};

use datafusion::common::HashMap;

use super::super::logical::{DynamicFanoutSpec, item_key, physical_id, render_spec};
use super::super::utils::build_inputs;
use super::{DAG, EdgeLabel, Result};
use crate::dag::NodeId;
use crate::dag::error::DagError;
use crate::dag::physical::PhysicalJobRef;
use crate::dag::runtime::RuntimeStatus;
use crate::engine_version;
use crate::fingerprint::{collect_input_identities, compute_node_fingerprint};

impl DAG {
    /// Materialize the physical job for one streaming item discovered by
    /// `coordinator_id`, wiring it into the scheduler's adjacency state.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn expand_dynamic_fanout_item(
        &mut self,
        coordinator_id: &str,
        item: serde_json::Value,
        sequence: usize,
        incoming: &mut HashMap<NodeId, Vec<(NodeId, EdgeLabel)>>,
        successors: &mut HashMap<NodeId, Vec<NodeId>>,
        pending: &mut HashMap<NodeId, usize>,
        ready: &mut VecDeque<NodeId>,
        all_ids: &mut Vec<NodeId>,
        streaming_expanded: &mut std::collections::BTreeSet<NodeId>,
    ) -> Result<NodeId> {
        let Some((coordinator_kind, coordinator_spec)) = self.specs.get(coordinator_id).cloned()
        else {
            return Err(DagError::Schedule(format!(
                "dynamic fanout `{coordinator_id}` has no retained specification"
            )));
        };
        if coordinator_kind != "dynamic_fanout" {
            return Err(DagError::Schedule(format!(
                "node `{coordinator_id}` is not a dynamic fanout coordinator"
            )));
        }
        let definition = serde_json::from_value::<DynamicFanoutSpec>(coordinator_spec.clone())
            .map_err(|error| {
                DagError::Schedule(format!(
                    "invalid dynamic fanout specification for `{coordinator_id}`: {error}"
                ))
            })?;
        let logical_id = self
            .physical_jobs
            .get(coordinator_id)
            .map(|job| job.logical_node.clone())
            .unwrap_or_else(|| definition.axis.clone());
        let item_key = item_key(&item, sequence)?;
        let actual_id = physical_id(
            &logical_id,
            &Some(definition.axis.clone()),
            &Some(item_key.clone()),
        );

        let builder = self.dynamic_node_builder.clone().ok_or_else(|| {
            DagError::Schedule(format!(
                "dynamic fanout `{coordinator_id}` has no registered node builder"
            ))
        })?;
        let rendered = render_spec(&definition.spec, &item)?;
        let existing_definition = self.specs.get(actual_id.as_str()).cloned();
        if existing_definition != Some((definition.kind.clone(), rendered.clone())) {
            let node = builder(&definition.kind, rendered.clone()).map_err(|error| {
                DagError::Schedule(format!(
                    "cannot build dynamic fanout job `{actual_id}`: {error}"
                ))
            })?;
            if self.nodes.contains_key(&actual_id) {
                self.replace_node_with_spec(
                    actual_id.as_str(),
                    node,
                    definition.kind.clone(),
                    rendered,
                )?;
            } else {
                self.add_node_with_spec(
                    actual_id.clone(),
                    node,
                    definition.kind.clone(),
                    rendered,
                )?;
            }
        }
        self.physical_jobs.insert(
            actual_id.clone(),
            PhysicalJobRef {
                logical_node: logical_id.clone(),
                axis: Some(definition.axis.clone()),
                item_key: Some(item_key),
                item: Some(item),
            },
        );

        let logical_edges = self
            .logical_graphs
            .iter()
            .flat_map(|graph| graph.edges().iter().cloned())
            .filter(|edge| edge.from == logical_id)
            .map(|edge| (edge.to.clone(), edge.from_port))
            .collect::<Vec<_>>();

        // The coordinator's placeholder edge is replaced by actual-job edges
        // once. On subsequent items only the new actual edge is appended.
        if !self.successors(coordinator_id).is_empty() {
            for successor in self.successors(coordinator_id) {
                if let Some(labels) = incoming.get_mut(&successor) {
                    labels.retain(|(from, _)| from != coordinator_id);
                }
                if let Some(count) = pending.get_mut(&successor) {
                    *count = count.saturating_sub(1);
                }
                while let Some((_, label)) = self
                    .incoming_edges_with_ports(&successor)
                    .into_iter()
                    .find(|(from, _)| from == coordinator_id)
                {
                    self.delete_edge(coordinator_id, &successor, label.from_port, label.to_port)?;
                }
            }
        }

        for (logical_target, from_port) in logical_edges {
            let target = physical_id(&logical_target, &None, &None);
            let mut used_ports = self
                .incoming_edges_with_ports(&target)
                .into_iter()
                .map(|(_, label)| label.to_port)
                .collect::<std::collections::BTreeSet<_>>();
            let mut to_port = 0u8;
            while used_ports.contains(&to_port) {
                to_port = to_port.checked_add(1).ok_or_else(|| {
                    DagError::Schedule(format!(
                        "dynamic fanout target `{target}` exceeds the port range"
                    ))
                })?;
            }
            used_ports.insert(to_port);
            self.add_edge(actual_id.clone(), target.clone(), from_port, to_port)?;
            successors
                .entry(actual_id.clone())
                .or_default()
                .push(target.clone());
            incoming
                .entry(target.clone())
                .or_default()
                .push((actual_id.clone(), EdgeLabel { from_port, to_port }));
            let count = pending.entry(target.clone()).or_insert(0);
            *count += 1;
        }

        if !all_ids.contains(&actual_id) {
            all_ids.push(actual_id.clone());
        }
        self.statuses
            .insert(actual_id.clone(), RuntimeStatus::Pending);
        pending.insert(actual_id.clone(), 0);
        ready.push_back(actual_id.clone());
        streaming_expanded.insert(coordinator_id.to_string());

        let actual_jobs = self
            .physical_jobs
            .iter()
            .filter(|(id, job)| id.as_str() != coordinator_id && job.logical_node == logical_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        successors.insert(coordinator_id.to_string(), actual_jobs);
        Ok(actual_id)
    }

    /// Expand `coordinator_id` into one physical job per channel item visible
    /// at its inputs, rewiring the scheduler's adjacency state. The
    /// coordinator's own fingerprint records the channel identity so an
    /// unchanged channel (and unchanged items) reuses the previous expansion.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn expand_dynamic_fanout(
        &mut self,
        coordinator_id: &str,
        incoming: &mut HashMap<NodeId, Vec<(NodeId, EdgeLabel)>>,
        successors: &mut HashMap<NodeId, Vec<NodeId>>,
        pending: &mut HashMap<NodeId, usize>,
        ready: &mut VecDeque<NodeId>,
        streaming_expanded: &mut std::collections::BTreeSet<NodeId>,
    ) -> Result<Vec<NodeId>> {
        let Some((coordinator_kind, coordinator_spec)) = self.specs.get(coordinator_id).cloned()
        else {
            return Err(DagError::Schedule(format!(
                "dynamic fanout `{coordinator_id}` has no retained specification"
            )));
        };
        if coordinator_kind != "dynamic_fanout" {
            return Err(DagError::Schedule(format!(
                "node `{coordinator_id}` is not a dynamic fanout coordinator"
            )));
        }
        let definition = serde_json::from_value::<DynamicFanoutSpec>(coordinator_spec.clone())
            .map_err(|error| {
                DagError::Schedule(format!(
                    "invalid dynamic fanout specification for `{coordinator_id}`: {error}"
                ))
            })?;
        let fallback_axis = definition.axis.clone();
        let logical_id = self
            .physical_jobs
            .get(coordinator_id)
            .map(|job| job.logical_node.clone())
            .unwrap_or_else(|| fallback_axis.clone());

        let inputs = build_inputs(coordinator_id, incoming, &self.outputs);
        let mut items = Vec::new();
        for input in &inputs {
            if let Ok(channel) = input.data.as_channel() {
                items.extend(channel.items.iter().cloned());
            }
        }
        if items.is_empty() {
            for (predecessor, _) in incoming.get(coordinator_id).into_iter().flatten() {
                if let Some(job) = self.physical_jobs.get(predecessor) {
                    if let Some(item) = &job.item {
                        items.push(item.clone());
                    }
                }
            }
        }

        let identities =
            collect_input_identities(coordinator_id, incoming, &self.outputs, &self.fingerprints);
        // Built-in coordinator (`dynamic_fanout`): plugin-less by definition,
        // hence `None` (WO-R09; plugin-backed nodes feed their identity at
        // the execution-time call site).
        let channel_fingerprint = compute_node_fingerprint(
            &coordinator_kind,
            Some(&coordinator_spec),
            crate::engine_version(),
            &identities,
            None,
        );
        let channel_unchanged = self.fingerprints.get(coordinator_id) == Some(&channel_fingerprint);

        let logical_edges = self
            .logical_graphs
            .iter()
            .flat_map(|graph| graph.edges().iter().cloned())
            .filter(|edge| edge.from == logical_id)
            .map(|edge| (edge.to.clone(), edge.from_port))
            .collect::<Vec<_>>();
        let mut keyed_items = BTreeMap::<String, (serde_json::Value, NodeId)>::new();
        for (index, item) in items.into_iter().enumerate() {
            let key = item_key(&item, index)?;
            let id = physical_id(
                &logical_id,
                &Some(definition.axis.clone()),
                &Some(key.clone()),
            );
            keyed_items.insert(key, (item, id));
        }

        let existing_actuals = self
            .physical_jobs
            .iter()
            .filter(|(id, job)| {
                id.as_str() != coordinator_id
                    && job.logical_node == logical_id
                    && self
                        .specs
                        .get(id.as_str())
                        .is_some_and(|(kind, _)| kind != "dynamic_fanout")
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();

        if streaming_expanded.contains(coordinator_id) {
            for successor in self.successors(coordinator_id) {
                if let Some(labels) = incoming.get_mut(&successor) {
                    labels.retain(|(from, _)| from != coordinator_id);
                }
                if let Some(count) = pending.get_mut(&successor) {
                    *count = count.saturating_sub(1);
                }
                while let Some((_, label)) = self
                    .incoming_edges_with_ports(&successor)
                    .into_iter()
                    .find(|(from, _)| from == coordinator_id)
                {
                    self.delete_edge(coordinator_id, &successor, label.from_port, label.to_port)?;
                }
            }
            self.fingerprints
                .insert(coordinator_id.to_string(), channel_fingerprint);
            self.statuses
                .insert(coordinator_id.to_string(), RuntimeStatus::Success);
            successors.insert(coordinator_id.to_string(), existing_actuals.clone());
            return Ok(existing_actuals);
        }

        for actual in &existing_actuals {
            for successor in successors.get(actual).cloned().unwrap_or_default() {
                if let Some(labels) = incoming.get_mut(&successor) {
                    labels.retain(|(from, _)| from != actual);
                }
                if let Some(count) = pending.get_mut(&successor) {
                    *count = count.saturating_sub(1);
                }
            }
            successors.remove(actual);
            if !keyed_items.values().any(|(_, id)| id == actual) {
                self.delete_node(actual)?;
            }
        }

        for successor in self.successors(coordinator_id) {
            if let Some(labels) = incoming.get_mut(&successor) {
                labels.retain(|(from, _)| from != coordinator_id);
            }
            if let Some(count) = pending.get_mut(&successor) {
                *count = count.saturating_sub(1);
            }
            while let Some((_, label)) = self
                .incoming_edges_with_ports(&successor)
                .into_iter()
                .find(|(from, _)| from == coordinator_id)
            {
                self.delete_edge(coordinator_id, &successor, label.from_port, label.to_port)?;
            }
        }

        let builder = self.dynamic_node_builder.clone().ok_or_else(|| {
            DagError::Schedule(format!(
                "dynamic fanout `{coordinator_id}` has no registered node builder"
            ))
        })?;
        let mut actual_ids = Vec::new();
        for (key, (item, actual_id)) in keyed_items {
            let rendered = render_spec(&definition.spec, &item)?;
            let existing_definition = self.specs.get(actual_id.as_str()).cloned();
            if existing_definition != Some((definition.kind.clone(), rendered.clone())) {
                let node = builder(&definition.kind, rendered.clone()).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot build dynamic fanout job `{actual_id}`: {error}"
                    ))
                })?;
                if self.nodes.contains_key(&actual_id) {
                    self.replace_node_with_spec(
                        actual_id.as_str(),
                        node,
                        definition.kind.clone(),
                        rendered,
                    )?;
                } else {
                    self.add_node_with_spec(
                        actual_id.clone(),
                        node,
                        definition.kind.clone(),
                        rendered,
                    )?;
                }
            }
            self.physical_jobs.insert(
                actual_id.clone(),
                PhysicalJobRef {
                    logical_node: logical_id.clone(),
                    axis: Some(definition.axis.clone()),
                    item_key: Some(key),
                    item: Some(item),
                },
            );
            self.statuses
                .insert(actual_id.clone(), RuntimeStatus::Pending);
            actual_ids.push(actual_id);
        }

        for (logical_target, from_port) in logical_edges {
            let target = physical_id(&logical_target, &None, &None);
            let mut used_ports = incoming
                .get(&target)
                .into_iter()
                .flatten()
                .map(|(_, label)| label.to_port)
                .collect::<std::collections::BTreeSet<_>>();
            for actual in &actual_ids {
                let mut to_port = 0u8;
                while used_ports.contains(&to_port) {
                    to_port = to_port.checked_add(1).ok_or_else(|| {
                        DagError::Schedule(format!(
                            "dynamic fanout target `{target}` exceeds the port range"
                        ))
                    })?;
                }
                used_ports.insert(to_port);
                self.add_edge(actual.clone(), target.clone(), from_port, to_port)?;
                successors
                    .entry(actual.clone())
                    .or_default()
                    .push(target.clone());
                incoming
                    .entry(target.clone())
                    .or_default()
                    .push((actual.clone(), EdgeLabel { from_port, to_port }));
                let count = pending.entry(target.clone()).or_insert(0);
                *count += 1;
            }
        }

        if !channel_unchanged {
            for actual in &actual_ids {
                self.fingerprints.remove(actual);
            }
        }
        self.fingerprints
            .insert(coordinator_id.to_string(), channel_fingerprint);
        self.statuses
            .insert(coordinator_id.to_string(), RuntimeStatus::Success);
        successors.insert(coordinator_id.to_string(), actual_ids.clone());
        for actual in &actual_ids {
            pending.insert(actual.clone(), 1);
            ready.push_back(actual.clone());
        }
        for (target, count) in pending.iter() {
            if *count == 0
                && self.statuses.get(target) == Some(&RuntimeStatus::Pending)
                && !actual_ids.contains(target)
            {
                ready.push_back(target.clone());
            }
        }
        Ok(actual_ids)
    }
}
