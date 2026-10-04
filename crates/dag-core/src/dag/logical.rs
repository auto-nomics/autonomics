//! Logical workflow graph and its expansion into physical jobs.
//!
//! The logical layer describes *what* the workflow contains; the physical
//! layer describes *which jobs* must run. Expansion currently supports static
//! scatter cardinality, axis propagation, broadcasting singleton values, and
//! gathering scattered jobs back into one job.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::error::DagError;
use super::physical::{GatherNode, PhysicalEdge, PhysicalGraph, PhysicalJobRef, PhysicalNode};
use super::{DagNode, NodeId};

pub type Result<T> = std::result::Result<T, DagError>;

/// How many physical jobs represent one logical node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogicalExecutionStrategy {
    /// Run one physical job.
    Once,
    /// Run one physical job per explicitly declared item.
    ForEach {
        axis: String,
        items: Vec<serde_json::Value>,
    },
    /// Collapse all upstream jobs into one physical job.
    Gather,
}

/// The executable definition behind a logical node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogicalNodeDefinition {
    Registry {
        kind: String,
        spec: serde_json::Value,
    },
    Gather,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalNode {
    pub id: String,
    pub definition: LogicalNodeDefinition,
    pub strategy: LogicalExecutionStrategy,
}

impl LogicalNode {
    pub fn registry(
        id: impl Into<String>,
        kind: impl Into<String>,
        spec: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            definition: LogicalNodeDefinition::Registry {
                kind: kind.into(),
                spec,
            },
            strategy: LogicalExecutionStrategy::Once,
        }
    }

    pub fn for_each(
        id: impl Into<String>,
        kind: impl Into<String>,
        spec: serde_json::Value,
        axis: impl Into<String>,
        items: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            id: id.into(),
            definition: LogicalNodeDefinition::Registry {
                kind: kind.into(),
                spec,
            },
            strategy: LogicalExecutionStrategy::ForEach {
                axis: axis.into(),
                items,
            },
        }
    }

    pub fn gather(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            definition: LogicalNodeDefinition::Gather,
            strategy: LogicalExecutionStrategy::Gather,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalEdge {
    pub from: String,
    pub from_port: u8,
    pub to: String,
    pub to_port: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogicalGraph {
    nodes: Vec<LogicalNode>,
    edges: Vec<LogicalEdge>,
}

#[derive(Default)]
pub struct LogicalGraphBuilder {
    graph: LogicalGraph,
}

impl LogicalGraphBuilder {
    pub fn add_node(mut self, node: LogicalNode) -> Self {
        self.graph.nodes.push(node);
        self
    }

    pub fn add_edge(
        mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        from_port: u8,
        to_port: u8,
    ) -> Self {
        self.graph.edges.push(LogicalEdge {
            from: from.into(),
            from_port,
            to: to.into(),
            to_port,
        });
        self
    }

    pub fn build(self) -> LogicalGraph {
        self.graph
    }
}

impl LogicalGraph {
    pub fn builder() -> LogicalGraphBuilder {
        LogicalGraphBuilder::default()
    }

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
        for edge in &self.edges {
            outgoing
                .entry(edge.from.as_str())
                .or_default()
                .push(&edge.to);
        }
        let mut state = HashMap::<&str, u8>::new();
        let mut ordered = Vec::new();
        for node in &self.nodes {
            self.visit_topological(node.id.as_str(), &outgoing, &mut state, &mut ordered)?;
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

    fn visit_topological<'a>(
        &'a self,
        id: &'a str,
        outgoing: &HashMap<&'a str, Vec<&'a str>>,
        state: &mut HashMap<&'a str, u8>,
        ordered: &mut Vec<&'a str>,
    ) -> Result<()> {
        match state.get(id).copied() {
            Some(2) => Ok(()),
            Some(_) => Err(DagError::Cycle(format!(
                "logical graph contains a cycle through `{id}`"
            ))),
            None => {
                state.insert(id, 1);
                if let Some(successors) = outgoing.get(id) {
                    for successor in successors {
                        self.visit_topological(successor, outgoing, state, ordered)?;
                    }
                }
                state.insert(id, 2);
                ordered.push(id);
                Ok(())
            }
        }
    }

    /// Expand this logical graph into runnable physical jobs.
    ///
    /// `build_node` receives the registry kind and the fully rendered spec for
    /// each job. Keeping construction outside the planner lets the same planner
    /// serve engine registries and test registries.
    pub fn compile<F>(&self, mut build_node: F) -> Result<PhysicalGraph>
    where
        F: FnMut(&str, serde_json::Value) -> Result<Box<dyn DagNode>>,
    {
        self.validate()?;
        let mut outgoing = HashMap::<&str, Vec<&str>>::new();
        let mut incoming = HashMap::<&str, Vec<&str>>::new();
        for edge in &self.edges {
            outgoing
                .entry(edge.from.as_str())
                .or_default()
                .push(&edge.to);
            incoming
                .entry(edge.to.as_str())
                .or_default()
                .push(&edge.from);
        }
        let mut state = HashMap::<&str, u8>::new();
        let mut logical_order = Vec::new();
        for node in &self.nodes {
            self.visit_topological(node.id.as_str(), &outgoing, &mut state, &mut logical_order)?;
        }
        logical_order.reverse();

        let by_id = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect::<HashMap<_, _>>();

        let mut axes = HashMap::<&str, String>::new();
        let mut axis_items = HashMap::<String, BTreeMap<String, serde_json::Value>>::new();
        let mut expanded = HashMap::<&str, Vec<PhysicalJobRef>>::new();
        let mut physical_nodes = Vec::new();

        for logical_id in logical_order {
            let node = by_id[logical_id];
            let (axis, items) = match &node.strategy {
                LogicalExecutionStrategy::ForEach { axis, items } => {
                    let keyed = keyed_items(&node.id, items)?;
                    if let Some(existing) = axis_items.get(axis) {
                        if existing != &keyed {
                            return Err(DagError::Schedule(format!(
                                "scatter axis `{axis}` has inconsistent item sets"
                            )));
                        }
                    }
                    axis_items.insert(axis.clone(), keyed.clone());
                    (Some(axis.clone()), keyed)
                }
                LogicalExecutionStrategy::Gather => (None, BTreeMap::new()),
                LogicalExecutionStrategy::Once => {
                    let incoming_axes = incoming
                        .get(logical_id)
                        .into_iter()
                        .flatten()
                        .filter_map(|from| axes.get(*from))
                        .cloned()
                        .collect::<BTreeSet<_>>();
                    if incoming_axes.len() > 1 {
                        return Err(DagError::Schedule(format!(
                            "logical node `{logical_id}` receives mismatched scatter axes: {}",
                            incoming_axes.into_iter().collect::<Vec<_>>().join(", ")
                        )));
                    }
                    match incoming_axes.into_iter().next() {
                        Some(axis) => (Some(axis.clone()), axis_items[&axis].clone()),
                        None => (None, BTreeMap::new()),
                    }
                }
            };
            if let Some(axis) = &axis {
                axes.insert(logical_id, axis.clone());
            }

            let jobs = if let LogicalNodeDefinition::Gather = node.definition {
                vec![PhysicalJobRef {
                    logical_node: node.id.clone(),
                    axis: None,
                    item_key: None,
                    item: None,
                }]
            } else {
                if axis.is_none() {
                    vec![PhysicalJobRef {
                        logical_node: node.id.clone(),
                        axis: None,
                        item_key: None,
                        item: None,
                    }]
                } else {
                    let axis = axis.clone().expect("checked cardinality");
                    items
                        .into_iter()
                        .map(|(item_key, item)| PhysicalJobRef {
                            logical_node: node.id.clone(),
                            axis: Some(axis.clone()),
                            item_key: Some(item_key),
                            item: Some(item),
                        })
                        .collect::<Vec<_>>()
                }
            };

            for job in &jobs {
                let id = physical_id(&node.id, &job.axis, &job.item_key);
                let (kind, spec, node_impl) = if let LogicalNodeDefinition::Gather = node.definition
                {
                    (
                        GatherNode::default().kind().to_string(),
                        serde_json::json!({ "kind": "logical_gather" }),
                        Box::new(GatherNode::default()) as Box<dyn DagNode>,
                    )
                } else {
                    let LogicalNodeDefinition::Registry { kind, spec } = &node.definition else {
                        unreachable!("gather handled above");
                    };
                    let rendered = if let Some(item) = &job.item {
                        render_spec(spec, item)?
                    } else {
                        spec.clone()
                    };
                    let node_impl = build_node(kind, rendered.clone())?;
                    (kind.clone(), rendered, node_impl)
                };
                physical_nodes.push(PhysicalNode {
                    id: id.clone(),
                    job: job.clone(),
                    kind,
                    spec,
                    node: node_impl,
                });
            }
            expanded.insert(logical_id, jobs);
        }

        let physical_by_id = physical_nodes
            .iter()
            .map(|node| (node.id.clone(), &node.job))
            .collect::<HashMap<_, _>>();
        let mut physical_edges = Vec::new();
        let mut gather_ports = HashMap::<String, u8>::new();
        for edge in &self.edges {
            let sources = &expanded[edge.from.as_str()];
            let targets = &expanded[edge.to.as_str()];

            if by_id[edge.to.as_str()].strategy == LogicalExecutionStrategy::Gather {
                let gather_id = physical_id(&edge.to, &None, &None);
                for source in sources {
                    let port = gather_ports
                        .entry(edge.to.clone())
                        .and_modify(|port| *port += 1)
                        .or_insert(0);
                    if port == &u8::MAX {
                        return Err(DagError::Schedule(format!(
                            "logical gather `{}` exceeds the maximum input port count",
                            edge.to
                        )));
                    }
                    physical_edges.push(PhysicalEdge {
                        from: physical_id(
                            &edge.from,
                            &axes.get(edge.from.as_str()).cloned(),
                            &source.item_key,
                        ),
                        from_port: edge.from_port,
                        to: gather_id.clone(),
                        to_port: *port,
                    });
                }
                continue;
            }

            let source_axis = axes.get(edge.from.as_str());
            let target_axis = axes.get(edge.to.as_str());
            match (source_axis, target_axis) {
                (Some(source_axis), Some(target_axis)) => {
                    if source_axis != target_axis {
                        return Err(DagError::Schedule(format!(
                            "edge {} -> {} connects mismatched scatter axes `{source_axis}` and `{target_axis}`",
                            edge.from, edge.to
                        )));
                    }
                    for source in sources {
                        for target in targets {
                            if source.item_key != target.item_key {
                                continue;
                            }
                            physical_edges.push(PhysicalEdge {
                                from: physical_id(
                                    &edge.from,
                                    &axes.get(edge.from.as_str()).cloned(),
                                    &source.item_key,
                                ),
                                from_port: edge.from_port,
                                to: physical_id(
                                    &edge.to,
                                    &axes.get(edge.to.as_str()).cloned(),
                                    &target.item_key,
                                ),
                                to_port: edge.to_port,
                            });
                        }
                    }
                }
                (None, None) => {
                    for source in sources {
                        for target in targets {
                            if source.item_key != target.item_key {
                                continue;
                            }
                            physical_edges.push(PhysicalEdge {
                                from: physical_id(
                                    &edge.from,
                                    &axes.get(edge.from.as_str()).cloned(),
                                    &source.item_key,
                                ),
                                from_port: edge.from_port,
                                to: physical_id(
                                    &edge.to,
                                    &axes.get(edge.to.as_str()).cloned(),
                                    &target.item_key,
                                ),
                                to_port: edge.to_port,
                            });
                        }
                    }
                }
                (None, Some(_)) => {
                    for target in targets {
                        physical_edges.push(PhysicalEdge {
                            from: physical_id(
                                &edge.from,
                                &axes.get(edge.from.as_str()).cloned(),
                                &sources[0].item_key,
                            ),
                            from_port: edge.from_port,
                            to: physical_id(
                                &edge.to,
                                &axes.get(edge.to.as_str()).cloned(),
                                &target.item_key,
                            ),
                            to_port: edge.to_port,
                        });
                    }
                }
                (Some(source_axis), None) => {
                    return Err(DagError::Schedule(format!(
                        "edge {} -> {} scatters over axis `{source_axis}` but target `{}` is not gathered",
                        edge.from, edge.to, edge.to
                    )));
                }
            }
        }

        if physical_nodes.len() != physical_by_id.len() {
            return Err(DagError::Schedule(
                "physical expansion produced duplicate job ids".into(),
            ));
        }
        Ok(PhysicalGraph::new(physical_nodes, physical_edges))
    }
}

fn keyed_items(
    logical_id: &str,
    items: &[serde_json::Value],
) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut keyed = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        let key = item_key(item, index)?;
        if keyed.insert(key.clone(), item.clone()).is_some() {
            return Err(DagError::Schedule(format!(
                "logical node `{logical_id}` has duplicate scatter key `{key}`"
            )));
        }
    }
    Ok(keyed)
}

fn item_key(item: &serde_json::Value, index: usize) -> Result<String> {
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

fn physical_id(logical_id: &str, axis: &Option<String>, item_key: &Option<String>) -> NodeId {
    match item_key {
        Some(item_key) => match axis {
            Some(axis) => format!("{logical_id}#{axis}={item_key}"),
            None => format!("{logical_id}#{item_key}"),
        },
        None => format!("{logical_id}#0"),
    }
}

fn render_spec(spec: &serde_json::Value, item: &serde_json::Value) -> Result<serde_json::Value> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodePorts;
    use crate::dag::graph::PortOutputs;
    use crate::dag::node_event::NodeReporter;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct StubNode {
        meta: NodePorts,
    }

    #[async_trait::async_trait]
    impl DagNode for StubNode {
        fn ports(&self) -> &NodePorts {
            &self.meta
        }

        async fn execute(
            &mut self,
            _ctx: &crate::registry::NodeCtx,
            _inputs: &[crate::dag::NodeInput],
            _reporter: &NodeReporter,
        ) -> std::result::Result<PortOutputs, DagError> {
            unreachable!("planner tests do not execute nodes")
        }

        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }

        fn kind(&self) -> &'static str {
            "stub"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn graph() -> LogicalGraph {
        LogicalGraph::builder()
            .add_node(LogicalNode::registry(
                "source",
                "stub",
                serde_json::json!({"id": 1}),
            ))
            .add_node(LogicalNode::for_each(
                "read",
                "stub",
                serde_json::json!({"path": "{{item.path}}"}),
                "sample",
                vec![
                    serde_json::json!({"key": "a", "path": "/a.csv"}),
                    serde_json::json!({"key": "b", "path": "/b.csv"}),
                ],
            ))
            .add_node(LogicalNode::registry(
                "transform",
                "stub",
                serde_json::json!({"sample": "{{item.key}}"}),
            ))
            .add_node(LogicalNode::gather("gather"))
            .add_node(LogicalNode::registry(
                "summary",
                "stub",
                serde_json::json!({"mode": "summary"}),
            ))
            .add_edge("source", "read", 0, 0)
            .add_edge("read", "transform", 0, 0)
            .add_edge("transform", "gather", 0, 0)
            .add_edge("gather", "summary", 0, 0)
            .build()
    }

    #[test]
    fn expands_scatter_axes_and_gathers_them_back() {
        let built = Mutex::new(Vec::new());
        let physical = graph()
            .compile(|kind, spec| {
                built.lock().unwrap().push((kind.to_string(), spec));
                Ok(Box::new(StubNode {
                    meta: NodePorts::new().add_output_port(None),
                }) as Box<dyn DagNode>)
            })
            .unwrap();

        let ids = physical
            .nodes()
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "source#0",
                "read#sample=a",
                "read#sample=b",
                "transform#sample=a",
                "transform#sample=b",
                "gather#0",
                "summary#0"
            ]
        );
        assert_eq!(physical.edge_count(), 7);
        let read = &physical.nodes()[1];
        assert_eq!(read.spec, serde_json::json!({"path": "/a.csv"}));
        let transform = &physical.nodes()[3];
        assert_eq!(transform.spec, serde_json::json!({"sample": "a"}));
        let gather_job = physical.nodes()[5].job.clone();
        assert_eq!(gather_job.logical_node, "gather");
        assert_eq!(gather_job.item_key, None);

        let built = built.into_inner().unwrap();
        assert_eq!(built.len(), 6);
    }

    #[test]
    fn rejects_mismatched_scatter_axes() {
        let graph = LogicalGraph::builder()
            .add_node(LogicalNode::for_each(
                "a",
                "stub",
                serde_json::json!({}),
                "one",
                vec![serde_json::json!(1)],
            ))
            .add_node(LogicalNode::for_each(
                "b",
                "stub",
                serde_json::json!({}),
                "two",
                vec![serde_json::json!(1)],
            ))
            .add_node(LogicalNode::registry("c", "stub", serde_json::json!({})))
            .add_edge("a", "c", 0, 0)
            .add_edge("b", "c", 0, 0)
            .build();
        let error = match graph.compile(|_, _| {
            Ok(Box::new(StubNode {
                meta: NodePorts::new().add_output_port(None),
            }) as Box<dyn DagNode>)
        }) {
            Ok(_) => panic!("mismatched scatter axes should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("mismatched scatter axes"));
    }

    #[test]
    fn rejects_direct_edges_between_mismatched_axes() {
        let graph = LogicalGraph::builder()
            .add_node(LogicalNode::for_each(
                "a",
                "stub",
                serde_json::json!({}),
                "one",
                vec![serde_json::json!(1)],
            ))
            .add_node(LogicalNode::for_each(
                "b",
                "stub",
                serde_json::json!({}),
                "two",
                vec![serde_json::json!(1)],
            ))
            .add_edge("a", "b", 0, 0)
            .build();
        let error = match graph.compile(|_, _| {
            Ok(Box::new(StubNode {
                meta: NodePorts::new().add_output_port(None),
            }) as Box<dyn DagNode>)
        }) {
            Ok(_) => panic!("direct mismatched axes should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("mismatched scatter axes"));
    }

    #[test]
    fn rejects_inconsistent_items_for_one_axis() {
        let graph = LogicalGraph::builder()
            .add_node(LogicalNode::for_each(
                "a",
                "stub",
                serde_json::json!({}),
                "axis",
                vec![serde_json::json!(1)],
            ))
            .add_node(LogicalNode::for_each(
                "b",
                "stub",
                serde_json::json!({}),
                "axis",
                vec![serde_json::json!(2)],
            ))
            .add_node(LogicalNode::registry("c", "stub", serde_json::json!({})))
            .add_edge("a", "c", 0, 0)
            .add_edge("b", "c", 0, 0)
            .build();
        let error = match graph.compile(|_, _| {
            Ok(Box::new(StubNode {
                meta: NodePorts::new().add_output_port(None),
            }) as Box<dyn DagNode>)
        }) {
            Ok(_) => panic!("inconsistent axis items should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("inconsistent item sets"));
    }

    #[test]
    fn rejects_cycles_and_empty_scatters() {
        let cycle = LogicalGraph::builder()
            .add_node(LogicalNode::registry("a", "stub", serde_json::json!({})))
            .add_node(LogicalNode::registry("b", "stub", serde_json::json!({})))
            .add_edge("a", "b", 0, 0)
            .add_edge("b", "a", 0, 0)
            .build();
        assert!(cycle.validate().unwrap_err().to_string().contains("cycle"));

        let empty_gather = LogicalGraph::builder()
            .add_node(LogicalNode::registry("a", "stub", serde_json::json!({})))
            .add_node(LogicalNode::gather("gather"))
            .build();
        assert!(
            empty_gather
                .validate()
                .unwrap_err()
                .to_string()
                .contains("logical gather `gather` requires at least one input")
        );

        let empty = LogicalGraph::builder()
            .add_node(LogicalNode::for_each(
                "a",
                "stub",
                serde_json::json!({}),
                "axis",
                Vec::new(),
            ))
            .build();
        assert!(
            empty
                .validate()
                .unwrap_err()
                .to_string()
                .contains("empty item set")
        );
    }
}
