//! Logical workflow graph and its expansion into physical jobs.
//!
//! The logical layer describes *what* the workflow contains; the physical
//! layer describes *which jobs* must run. Expansion currently supports static
//! scatter cardinality, axis propagation, broadcasting singleton values, and
//! gathering scattered jobs back into one job.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::channel::{ChannelNode, ChannelOperator, validate_branches};
use super::error::DagError;
use super::NodeId;

pub type Result<T> = std::result::Result<T, DagError>;

/// How many physical jobs represent one logical node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum LogicalExecutionStrategy {
    /// Run one physical job.
    Once,
    /// Run one physical job per explicitly declared item.
    ForEach {
        axis: String,
        items: Vec<serde_json::Value>,
    },
    /// Run one physical job per item discovered from an upstream Channel.
    DynamicForEach { axis: String },
    /// Collapse all upstream jobs into one physical job.
    Gather,
}

/// The executable definition behind a logical node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum LogicalNodeDefinition {
    Registry {
        kind: String,
        spec: serde_json::Value,
    },
    Gather,
    Channel(ChannelOperator),
}

/// Serializable payload retained by a dynamic fanout coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicFanoutSpec {
    pub kind: String,
    pub spec: serde_json::Value,
    pub axis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogicalNode {
    pub id: String,
    pub definition: LogicalNodeDefinition,
    pub strategy: LogicalExecutionStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogicalEdge {
    pub from: String,
    pub from_port: u8,
    pub to: String,
    pub to_port: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct LogicalGraph {
    nodes: Vec<LogicalNode>,
    edges: Vec<LogicalEdge>,
}

impl LogicalGraph {
    pub fn nodes(&self) -> &[LogicalNode] {
        &self.nodes
    }

    pub fn edges(&self) -> &[LogicalEdge] {
        &self.edges
    }

    pub fn validate(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() {
                return Err(DagError::Schedule("logical node id cannot be empty".into()));
            }
            if !ids.insert(node.id.clone()) {
                return Err(DagError::DuplicateNode(node.id.clone()));
            }
            if let LogicalNodeDefinition::Registry { kind, .. } = &node.definition {
                if kind.trim().is_empty() {
                    return Err(DagError::Schedule(format!(
                        "logical node `{}` has an empty registry kind",
                        node.id
                    )));
                }
            }
            if let LogicalExecutionStrategy::ForEach { axis, items } = &node.strategy {
                if axis.trim().is_empty() {
                    return Err(DagError::Schedule(format!(
                        "logical node `{}` has an empty scatter axis",
                        node.id
                    )));
                }
                if items.is_empty() {
                    return Err(DagError::Schedule(format!(
                        "logical node `{}` scatters over an empty item set",
                        node.id
                    )));
                }
            }
            if let LogicalExecutionStrategy::DynamicForEach { axis } = &node.strategy {
                if axis.trim().is_empty() {
                    return Err(DagError::Schedule(format!(
                        "logical node `{}` has an empty dynamic fanout axis",
                        node.id
                    )));
                }
                if !matches!(node.definition, LogicalNodeDefinition::Registry { .. }) {
                    return Err(DagError::Schedule(format!(
                        "logical node `{}` must use a registry definition with dynamic fanout",
                        node.id
                    )));
                }
            }
            match (&node.definition, &node.strategy) {
                (LogicalNodeDefinition::Gather, LogicalExecutionStrategy::Gather) => {}
                (LogicalNodeDefinition::Registry { .. }, LogicalExecutionStrategy::Gather) => {
                    return Err(DagError::Schedule(format!(
                        "logical node `{}` uses a registry definition with gather execution",
                        node.id
                    )));
                }
                (LogicalNodeDefinition::Gather, _) => {
                    return Err(DagError::Schedule(format!(
                        "logical node `{}` must use gather execution",
                        node.id
                    )));
                }
                _ => {}
            }
        }

        let strategy_by_id = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), &node.strategy))
            .collect::<HashMap<_, _>>();
        let definition_by_id = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), &node.definition))
            .collect::<HashMap<_, _>>();
        for edge in &self.edges {
            if !ids.contains(&edge.from) {
                return Err(DagError::UnknownNode(edge.from.clone()));
            }
            if !ids.contains(&edge.to) {
                return Err(DagError::UnknownNode(edge.to.clone()));
            }
            if edge.from == edge.to {
                return Err(DagError::Cycle(format!(
                    "logical node `{}` has a self edge",
                    edge.from
                )));
            }
        }

        let mut outgoing = HashMap::<&str, Vec<&str>>::new();
        let mut incoming_count = HashMap::<&str, usize>::new();
        for edge in &self.edges {
            outgoing
                .entry(edge.from.as_str())
                .or_default()
                .push(&edge.to);
            *incoming_count.entry(edge.to.as_str()).or_default() += 1;
        }
        // Acyclicity (Kahn) — the recursive topological visitor was removed
        // together with the compile path.
        let mut indegree = HashMap::<&str, usize>::new();
        for node in &self.nodes {
            indegree.entry(node.id.as_str()).or_insert(0);
        }
        for edge in &self.edges {
            *indegree.entry(edge.to.as_str()).or_default() += 1;
        }
        let mut queue: Vec<&str> = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(id, _)| *id)
            .collect();
        let mut visited = 0usize;
        while let Some(id) = queue.pop() {
            visited += 1;
            for successor in outgoing.get(id).cloned().unwrap_or_default() {
                let degree = indegree.get_mut(successor).expect("edge targets are nodes");
                *degree -= 1;
                if *degree == 0 {
                    queue.push(successor);
                }
            }
        }
        if visited != self.nodes.len() {
            return Err(DagError::Schedule(
                "logical graph contains a cycle".into(),
            ));
        }

        for node in &self.nodes {
            let incoming = incoming_count.get(node.id.as_str()).copied().unwrap_or(0);
            let expected_inputs = match &node.definition {
                LogicalNodeDefinition::Channel(ChannelOperator::OfItems { .. }) => 0,
                LogicalNodeDefinition::Channel(
                    ChannelOperator::Mix | ChannelOperator::Combine | ChannelOperator::Join { .. },
                ) => 2,
                LogicalNodeDefinition::Channel(_) => 1,
                LogicalNodeDefinition::Registry { .. } => {
                    if matches!(
                        node.strategy,
                        LogicalExecutionStrategy::DynamicForEach { .. }
                    ) {
                        1
                    } else {
                        continue;
                    }
                }
                LogicalNodeDefinition::Gather => continue,
            };
            if incoming != expected_inputs {
                return Err(DagError::Schedule(format!(
                    "logical node `{}` expects {expected_inputs} incoming edge(s), got {incoming}",
                    node.id
                )));
            }
        }

        for node in &self.nodes {
            if let LogicalNodeDefinition::Channel(ChannelOperator::Branch { branches }) =
                &node.definition
            {
                validate_branches(branches)?;
                for edge in self.edges.iter().filter(|edge| edge.from == node.id) {
                    if edge.from_port as usize >= branches.len() {
                        return Err(DagError::Schedule(format!(
                            "edge {} -> {} uses undefined branch output port {}",
                            edge.from, edge.to, edge.from_port
                        )));
                    }
                }
            }
        }

        for edge in &self.edges {
            if !matches!(
                strategy_by_id[edge.from.as_str()],
                LogicalExecutionStrategy::DynamicForEach { .. }
            ) {
                continue;
            }
            if matches!(
                definition_by_id[edge.to.as_str()],
                LogicalNodeDefinition::Channel(
                    ChannelOperator::Collect
                        | ChannelOperator::Mix
                        | ChannelOperator::Combine
                        | ChannelOperator::Join { .. }
                )
            ) || matches!(
                definition_by_id[edge.to.as_str()],
                LogicalNodeDefinition::Gather
            ) {
                continue;
            }
            return Err(DagError::Schedule(format!(
                "dynamic fanout target `{}` must be channel.collect or logical gather",
                edge.to
            )));
        }

        let gather_ids = self
            .nodes
            .iter()
            .filter(|node| node.strategy == LogicalExecutionStrategy::Gather)
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        for gather in gather_ids {
            if !self.edges.iter().any(|edge| edge.to == gather) {
                return Err(DagError::Schedule(format!(
                    "logical gather `{gather}` requires at least one input"
                )));
            }
        }

        Ok(())
    }

    // The compile path (scatter/gather expansion into physical jobs) was
    // removed with the logical authoring layer: stage-model flows
    // (`dag-engine`) own cardinality now. The types above remain because
    // persisted manifests deserialize into them and the scheduler still
    // reports provenance from `physical_jobs`.
}

pub(crate) fn item_key(item: &serde_json::Value, index: usize) -> Result<String> {
    let raw = match item {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Object(fields) => fields
            .get("key")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        _ => None,
    };
    let raw = raw.unwrap_or_else(|| format!("item-{index:06}"));
    let sanitized = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let mut key = if sanitized.is_empty() {
        format!("item-{index:06}")
    } else {
        sanitized
    };
    if key.len() > 64 {
        key.truncate(48);
        key.push_str(&format!("-{index:06}"));
    }
    Ok(key)
}

pub(crate) fn physical_id(
    logical_id: &str,
    axis: &Option<String>,
    item_key: &Option<String>,
) -> NodeId {
    match item_key {
        Some(item_key) => match axis {
            Some(axis) => format!("{logical_id}#{axis}={item_key}"),
            None => format!("{logical_id}#{item_key}"),
        },
        None => format!("{logical_id}#0"),
    }
}

pub(crate) fn render_spec(
    spec: &serde_json::Value,
    item: &serde_json::Value,
) -> Result<serde_json::Value> {
    match spec {
        serde_json::Value::String(template) => render_string(template, item),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| render_spec(value, item))
            .collect::<Result<Vec<_>>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(fields) => {
            let mut rendered = serde_json::Map::new();
            for (name, value) in fields {
                rendered.insert(name.clone(), render_spec(value, item)?);
            }
            Ok(serde_json::Value::Object(rendered))
        }
        value => Ok(value.clone()),
    }
}

fn render_string(template: &str, item: &serde_json::Value) -> Result<serde_json::Value> {
    let mut rendered = String::new();
    let mut cursor = 0;
    while let Some(start) = template[cursor..].find("{{") {
        let start = cursor + start;
        rendered.push_str(&template[cursor..start]);
        let Some(end_relative) = template[start..].find("}}") else {
            return Err(DagError::Schedule(format!(
                "invalid item template `{template}`: missing `}}`"
            )));
        };
        let end = start + end_relative + 2;
        let token = &template[start..end];
        let path = token
            .strip_prefix("{{item")
            .and_then(|path| path.strip_suffix("}}"))
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "unsupported template token `{token}`; expected `{{{{item}}}}` or `{{{{item.field}}}}`"
                ))
            })?;
        let value = if path.is_empty() {
            Some(item.clone())
        } else {
            item.pointer(&format!(
                "/{}",
                path.trim_start_matches('.').replace('.', "/")
            ))
            .cloned()
        };
        let Some(value) = value else {
            return Err(DagError::Schedule(format!(
                "template token `{token}` refers to a missing item field"
            )));
        };
        rendered.push_str(&scalar_display(&value));
        cursor = end;
    }
    rendered.push_str(&template[cursor..]);
    if template == "{{item}}" {
        return Ok(match item {
            serde_json::Value::String(value) => serde_json::Value::String(value.clone()),
            value => value.clone(),
        });
    }
    Ok(serde_json::Value::String(rendered))
}

fn scalar_display(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}
