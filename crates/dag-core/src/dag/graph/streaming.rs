//! Channel streaming: item-by-item propagation between channel nodes, plus
//! stream finalization when every input of a streaming node has closed.
//!
//! The scheduler loop (see [`super::scheduler`]) routes `ChannelItem` /
//! `ChannelClosed` node events into [`DAG::propagate_stream_item`] and
//! [`DAG::close_stream_successors`], which drive channel nodes one item at a
//! time instead of waiting for whole inputs.

use std::collections::{BTreeSet, VecDeque};

use datafusion::common::HashMap;
use tokio::sync::mpsc;

use super::super::channel::{ChannelNode, ChannelOperator, ChannelStreamState};
use super::{DAG, EdgeLabel, PortOutputs, Result};
use crate::dag::NodeId;
use crate::dag::error::DagError;
use crate::dag::node_event::NodeReporter;
use crate::dag::runtime::RuntimeStatus;

/// Per-run bookkeeping for streaming (channel) nodes: buffered items, stream
/// states, expected/closed input sets.
#[derive(Default)]
pub(super) struct StreamRuntimeState {
    pub(super) node_items: HashMap<NodeId, Vec<serde_json::Value>>,
    pub(super) node_counts: HashMap<NodeId, usize>,
    pub(super) node_outputs: HashMap<NodeId, HashMap<u8, Vec<serde_json::Value>>>,
    pub(super) node_states: HashMap<NodeId, ChannelStreamState>,
    pub(super) expected_inputs: HashMap<NodeId, BTreeSet<(NodeId, u8)>>,
    pub(super) closed_inputs: HashMap<NodeId, BTreeSet<(NodeId, u8)>>,
    pub(super) pending_closes: BTreeSet<NodeId>,
}

impl DAG {
    /// The streaming operator spec of `node_id`, if the node is a
    /// stream-capable channel node.
    pub(super) fn streaming_operator(&self, node_id: &str) -> Result<Option<ChannelOperator>> {
        let Some((kind, spec)) = self.specs.get(node_id) else {
            return Ok(None);
        };
        if kind != "channel" {
            return Ok(None);
        }
        let operator =
            serde_json::from_value::<ChannelOperator>(spec.clone()).map_err(|error| {
                DagError::Schedule(format!("invalid channel operator for `{node_id}`: {error}"))
            })?;
        let supports_stream = self
            .nodes
            .get(node_id)
            .and_then(|node| node.as_any().downcast_ref::<ChannelNode>())
            .is_some_and(|node| node.supports_stream_item());
        Ok(supports_stream.then_some(operator))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn propagate_stream_item(
        &mut self,
        from: &str,
        from_port: u8,
        item: serde_json::Value,
        incoming: &mut HashMap<NodeId, Vec<(NodeId, EdgeLabel)>>,
        successors: &mut HashMap<NodeId, Vec<NodeId>>,
        pending: &mut HashMap<NodeId, usize>,
        ready: &mut VecDeque<NodeId>,
        all_ids: &mut Vec<NodeId>,
        streaming_expanded: &mut BTreeSet<NodeId>,
        stream: &mut StreamRuntimeState,
        tx: mpsc::Sender<crate::dag::node_event::NodeEvent>,
    ) -> Result<()> {
        let targets = successors.get(from).cloned().unwrap_or_default();
        for target in targets {
            let labels = incoming
                .get(&target)
                .into_iter()
                .flatten()
                .filter(|(producer, label)| {
                    producer.as_str() == from && label.from_port == from_port
                })
                .map(|(_, label)| label.clone())
                .collect::<Vec<_>>();
            if labels.is_empty() {
                continue;
            }

            if self
                .specs
                .get(target.as_str())
                .is_some_and(|(kind, _)| kind == "dynamic_fanout")
            {
                for _label in labels {
                    let sequence = *stream.node_counts.entry(target.clone()).or_insert(0);
                    self.expand_dynamic_fanout_item(
                        target.as_str(),
                        item.clone(),
                        sequence as usize,
                        incoming,
                        successors,
                        pending,
                        ready,
                        all_ids,
                        streaming_expanded,
                    )?;
                    *stream.node_counts.entry(target.clone()).or_insert(0) += 1;
                }
                continue;
            }

            let Some(operator) = self.streaming_operator(target.as_str())? else {
                continue;
            };
            let _ = operator;
            if !stream.expected_inputs.contains_key(&target) {
                continue;
            }
            let input_port = labels
                .first()
                .map(|label| label.to_port)
                .unwrap_or_default();

            for label in labels {
                let Some(from_idx) = self.id_to_idx.get(from) else {
                    continue;
                };
                let Some(to_idx) = self.id_to_idx.get(&target) else {
                    continue;
                };
                let exists = self.graph.edges_connecting(*from_idx, *to_idx).any(|edge| {
                    edge.weight().from_port == label.from_port
                        && edge.weight().to_port == label.to_port
                });
                if !exists {
                    continue;
                }
                self.delete_edge(from, &target, label.from_port, label.to_port)?;
                if let Some(count) = pending.get_mut(&target) {
                    *count = count.saturating_sub(1);
                }
            }
            self.statuses.insert(target.clone(), RuntimeStatus::Running);
            stream
                .node_items
                .entry(target.clone())
                .or_default()
                .push(item.clone());
            let input_count = stream
                .node_items
                .get(&target)
                .map(Vec::len)
                .unwrap_or_default();
            stream.node_counts.insert(target.clone(), input_count);

            let outputs = {
                let Some(node) = self.nodes.get(&target) else {
                    continue;
                };
                let mut node = node
                    .clone_box()
                    .as_any()
                    .downcast_ref::<ChannelNode>()
                    .unwrap()
                    .clone();
                let state = stream.node_states.entry(target.clone()).or_default();
                node.process_stream_item(state, input_port, item.clone())
                    .await?
            };
            for (output_port, output_item) in outputs {
                let sequence = stream
                    .node_outputs
                    .entry(target.clone())
                    .or_default()
                    .entry(output_port)
                    .or_default()
                    .len() as u64;
                stream
                    .node_outputs
                    .entry(target.clone())
                    .or_default()
                    .entry(output_port)
                    .or_default()
                    .push(output_item.clone());
                let reporter = NodeReporter::new(target.clone(), tx.clone());
                reporter
                    .emit_channel_item(output_port, sequence, output_item)
                    .await;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn close_stream_successors(
        &mut self,
        from: &str,
        from_port: u8,
        successors: &mut HashMap<NodeId, Vec<NodeId>>,
        pending: &mut HashMap<NodeId, usize>,
        ready: &mut VecDeque<NodeId>,
        stream: &mut StreamRuntimeState,
        tx: mpsc::Sender<crate::dag::node_event::NodeEvent>,
    ) -> Result<()> {
        let targets = successors.get(from).cloned().unwrap_or_default();
        for target in targets {
            let Some(_operator) = self.streaming_operator(target.as_str())? else {
                continue;
            };
            let Some(expected) = stream.expected_inputs.get(&target).cloned() else {
                continue;
            };
            if !expected.contains(&(from.to_string(), from_port)) {
                continue;
            }
            stream
                .closed_inputs
                .entry(target.clone())
                .or_default()
                .insert((from.to_string(), from_port));
            if stream.closed_inputs.get(&target) != Some(&expected) {
                continue;
            }

            let _items = stream.node_items.remove(&target);
            let _input_count = stream.node_counts.remove(&target).unwrap_or_default();
            let state = stream.node_states.remove(&target).unwrap_or_default();
            let existing_output_items = stream.node_outputs.remove(&target).unwrap_or_default();
            let finalized = {
                let Some(node) = self.nodes.get(&target) else {
                    continue;
                };
                let node_box = node.clone_box();
                let node = node_box.as_any().downcast_ref::<ChannelNode>().unwrap();
                let mut state = state;
                node.finish_stream(&mut state).await?
            };
            let finalized_count = finalized.len();
            stream
                .node_outputs
                .insert(target.clone(), existing_output_items);
            for (output_port, output_item) in finalized {
                let sequence = stream
                    .node_outputs
                    .get_mut(&target)
                    .unwrap()
                    .entry(output_port)
                    .or_default()
                    .len() as u64;
                stream
                    .node_outputs
                    .get_mut(&target)
                    .unwrap()
                    .get_mut(&output_port)
                    .unwrap()
                    .push(output_item.clone());
                let reporter = NodeReporter::new(target.clone(), tx.clone());
                reporter
                    .emit_channel_item(output_port, sequence, output_item)
                    .await;
            }

            let declared_ports = self
                .nodes
                .get(&target)
                .map(|node| {
                    node.ports()
                        .output_ports()
                        .iter()
                        .map(|port| port.index)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut outputs = PortOutputs::new();
            for port in declared_ports {
                let items = stream
                    .node_outputs
                    .get(&target)
                    .and_then(|items| items.get(&port))
                    .cloned()
                    .unwrap_or_default();
                outputs.insert(port, crate::ChannelValue { items });
            }
            self.outputs.insert(target.clone(), outputs);
            self.statuses.insert(target.clone(), RuntimeStatus::Success);
            self.fingerprints.remove(&target);

            let ports = self
                .outputs
                .get(&target)
                .map(|outputs| outputs.iter().map(|(port, _)| *port).collect::<Vec<_>>())
                .unwrap_or_default();
            for port in ports {
                let item_count = stream
                    .node_outputs
                    .get(&target)
                    .and_then(|items| items.get(&port))
                    .map(Vec::len)
                    .unwrap_or_default();
                let reporter = NodeReporter::new(target.clone(), tx.clone());
                stream.pending_closes.insert(target.clone());
                reporter.close_channel(port, item_count as u64).await;
            }

            for successor in successors.get(&target).cloned().unwrap_or_default() {
                let streaming_successor = stream.expected_inputs.contains_key(&successor)
                    || self
                        .specs
                        .get(successor.as_str())
                        .is_some_and(|(kind, _)| kind == "dynamic_fanout");
                if streaming_successor && finalized_count > 0 {
                    continue;
                }
                let left = {
                    let count = pending.entry(successor.clone()).or_insert(0);
                    count.saturating_sub(1)
                };
                if left == 0 && self.statuses.get(&successor) == Some(&RuntimeStatus::Pending) {
                    ready.push_back(successor);
                }
            }
        }
        Ok(())
    }
}
