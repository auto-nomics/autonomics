//! Built-in dataflow channel operators.
//!
//! A channel is a value containing ordered JSON items. Logical edges route
//! channel values between operators; dynamic fanout turns each item into a
//! physical job at runtime.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::error::DagError;
use super::logical::render_spec;
use super::{DagNode, NodeInput, NodePorts};
use crate::dag::graph::PortOutputs;
use crate::dag::node_event::NodeReporter;
use crate::value::{ChannelValue, FileRef, NodeValue, PortType};

pub type Result<T> = std::result::Result<T, DagError>;

/// A declarative, deterministic subset of Nextflow-style channel operators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operator", rename_all = "snake_case")]
pub enum ChannelOperator {
    OfItems { items: Vec<Value> },
    Map { template: Value },
    Filter { path: String, equals: Value },
    Flatten,
    Mix,
    Collect,
    Combine,
    Join { left_key: String, right_key: String },
    GroupTuple { key: String },
    Branch { branches: Vec<ChannelBranch> },
}

/// One ordered branch predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelBranch {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ChannelNode {
    operator: ChannelOperator,
    ports: NodePorts,
}

impl ChannelNode {
    pub fn new(operator: ChannelOperator) -> Self {
        let mut ports = Self::base_ports(&operator);
        ports = match input_count(&operator) {
            0 => ports.set_fixed_input(true),
            1 if matches!(operator, ChannelOperator::Collect) => ports,
            1 => ports
                .add_input_port_of_type(None, input_type(&operator))
                .set_fixed_input(true),
            _ => {
                let ports = ports
                    .add_input_port_of_type(None, PortType::Channel)
                    .add_input_port_of_type(None, PortType::Channel);
                ports
            }
        };
        if let ChannelOperator::Branch { branches } = &operator {
            for branch in branches {
                ports = ports.add_output_port_of_type_with_label(
                    None,
                    PortType::Channel,
                    branch.name.clone(),
                );
            }
        } else {
            ports = ports.add_output_port_of_type(None, PortType::Channel);
        }
        Self { operator, ports }
    }

    pub fn from_spec(spec: &Value) -> Result<Self> {
        let operator =
            serde_json::from_value::<ChannelOperator>(spec.clone()).map_err(|error| {
                DagError::Schedule(format!("invalid channel operator specification: {error}"))
            })?;
        if let ChannelOperator::Branch { branches } = &operator {
            validate_branches(branches)?;
        }
        Ok(Self::new(operator))
    }

    pub fn operator(&self) -> &ChannelOperator {
        &self.operator
    }

    /// Whether this operator can be applied one item at a time.
    ///
    /// Stateful fan-in operators intentionally retain batch execution until a
    /// dedicated streaming state machine is added.
    pub fn supports_stream_item(&self) -> bool {
        matches!(
            self.operator,
            ChannelOperator::Map { .. }
                | ChannelOperator::Filter { .. }
                | ChannelOperator::Flatten
                | ChannelOperator::Branch { .. }
        )
    }

    pub fn spec(&self) -> Value {
        serde_json::to_value(&self.operator).unwrap_or(Value::Null)
    }

    fn base_ports(operator: &ChannelOperator) -> NodePorts {
        let mut ports = NodePorts::new();
        if let ChannelOperator::Collect = operator {
            ports = ports.add_input_port_of_type(None, PortType::Any);
            return ports.set_fixed_input(false);
        }
        ports
    }

    fn apply(&self, inputs: &[NodeInput]) -> Result<ChannelValue> {
        let operator = self.operator.clone();
        match operator {
            ChannelOperator::OfItems { items } => Ok(ChannelValue { items }),
            ChannelOperator::Map { template } => {
                let items = channel_items(inputs, 0)?;
                let items = items
                    .into_iter()
                    .map(|item| render_spec(&template, &item))
                    .collect::<Result<Vec<_>>>()?;
                Ok(ChannelValue { items })
            }
            ChannelOperator::Filter { path, equals } => {
                let items = channel_items(inputs, 0)?;
                Ok(ChannelValue {
                    items: items
                        .into_iter()
                        .filter(|item| item.pointer(&pointer_path(&path)) == Some(&equals))
                        .collect(),
                })
            }
            ChannelOperator::Flatten => {
                let mut items = Vec::new();
                for item in channel_items(inputs, 0)? {
                    match item {
                        Value::Array(values) => items.extend(values),
                        value => {
                            return Err(DagError::Schedule(format!(
                                "channel.flatten requires array items, got {value}"
                            )));
                        }
                    }
                }
                Ok(ChannelValue { items })
            }
            ChannelOperator::Mix => {
                let mut items = Vec::new();
                for input in inputs {
                    items.extend(channel_items(std::slice::from_ref(input), input.port)?);
                }
                Ok(ChannelValue { items })
            }
            ChannelOperator::Collect => {
                let mut items = Vec::new();
                for input in inputs {
                    match &input.data {
                        NodeValue::Channel(channel) => items.extend(channel.items.iter().cloned()),
                        NodeValue::File(file) => items.push(file_item(file)?),
                        NodeValue::FileSet(files) => {
                            for file in files {
                                items.push(file_item(file)?);
                            }
                        }
                        NodeValue::DataFrame(_) => {
                            return Err(DagError::Schedule(
                                "channel.collect cannot serialize DataFrame outputs".into(),
                            ));
                        }
                    }
                }
                Ok(ChannelValue { items })
            }
            ChannelOperator::Combine => {
                let left = channel_items(inputs, 0)?;
                let right = channel_items(inputs, 1)?;
                let mut items = Vec::with_capacity(left.len().saturating_mul(right.len()));
                for left_item in left {
                    for right_item in &right {
                        let mut combined = Map::new();
                        combined.insert("left".into(), left_item.clone());
                        combined.insert("right".into(), right_item.clone());
                        items.push(Value::Object(combined));
                    }
                }
                Ok(ChannelValue { items })
            }
            ChannelOperator::Join {
                left_key,
                right_key,
            } => {
                let left = channel_items(inputs, 0)?;
                let right = channel_items(inputs, 1)?;
                let mut right_by_key = Vec::<(String, Value)>::new();
                for item in right {
                    if let Some(key) = item.pointer(&pointer_path(&right_key)) {
                        right_by_key.push((canonical_json(key), item));
                    }
                }
                let mut items = Vec::new();
                for left_item in left {
                    let Some(left_value) = left_item.pointer(&pointer_path(&left_key)) else {
                        continue;
                    };
                    let left_key = canonical_json(left_value);
                    for (right_value, right_item) in &right_by_key {
                        if *right_value == left_key {
                            let mut joined = Map::new();
                            if let (Value::Object(left), Value::Object(right)) =
                                (&left_item, right_item)
                            {
                                for (name, value) in left {
                                    joined.insert(format!("left_{name}"), value.clone());
                                }
                                for (name, value) in right {
                                    joined.insert(format!("right_{name}"), value.clone());
                                }
                            } else {
                                joined.insert("left".into(), left_item.clone());
                                joined.insert("right".into(), (*right_item).clone());
                            }
                            items.push(Value::Object(joined));
                        }
                    }
                }
                Ok(ChannelValue { items })
            }
            ChannelOperator::GroupTuple { key } => {
                let mut groups = Vec::<(String, Value, Vec<Value>)>::new();
                for item in channel_items(inputs, 0)? {
                    let Some(key_value) = item.pointer(&pointer_path(&key)) else {
                        continue;
                    };
                    let key_text = canonical_json(key_value);
                    match groups
                        .iter_mut()
                        .find(|(existing, _, _)| *existing == key_text)
                    {
                        Some((_, _, values)) => values.push(item),
                        None => groups.push((key_text, key_value.clone(), vec![item])),
                    }
                }
                Ok(ChannelValue {
                    items: groups
                        .into_iter()
                        .map(|(_, key, values)| Value::Array(vec![key, Value::Array(values)]))
                        .collect(),
                })
            }
            ChannelOperator::Branch { .. } => {
                unreachable!("branch outputs are materialized by `execute`")
            }
        }
    }
}

pub(crate) fn validate_branches(branches: &[ChannelBranch]) -> Result<()> {
    if branches.is_empty() {
        return Err(DagError::Schedule(
            "channel.branch requires at least one branch".into(),
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    for branch in branches {
        if branch.name.trim().is_empty() {
            return Err(DagError::Schedule(
                "channel.branch names cannot be empty".into(),
            ));
        }
        if !names.insert(branch.name.clone()) {
            return Err(DagError::Schedule(format!(
                "channel.branch contains duplicate output `{}`",
                branch.name
            )));
        }
        if branch.path.trim().is_empty() {
            return Err(DagError::Schedule(format!(
                "channel.branch `{}` has an empty item path",
                branch.name
            )));
        }
        let has_predicate = branch.equals.is_some()
            || branch.not_equals.is_some()
            || branch.exists.is_some()
            || branch.prefix.is_some()
            || branch.suffix.is_some()
            || branch.contains.is_some();
        if !has_predicate {
            return Err(DagError::Schedule(format!(
                "channel.branch `{}` requires at least one predicate",
                branch.name
            )));
        }
    }
    Ok(())
}

fn input_count(operator: &ChannelOperator) -> usize {
    match operator {
        ChannelOperator::OfItems { .. } => 0,
        ChannelOperator::Mix | ChannelOperator::Combine | ChannelOperator::Join { .. } => 2,
        ChannelOperator::Map { .. }
        | ChannelOperator::Filter { .. }
        | ChannelOperator::Flatten
        | ChannelOperator::Collect
        | ChannelOperator::GroupTuple { .. }
        | ChannelOperator::Branch { .. } => 1,
    }
}

fn input_type(operator: &ChannelOperator) -> PortType {
    match operator {
        ChannelOperator::Collect => PortType::Any,
        _ => PortType::Channel,
    }
}

fn channel_items(inputs: &[NodeInput], port: u8) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    for input in inputs.iter().filter(|input| input.port == port) {
        items.extend(input.data.as_channel()?.items.clone());
    }
    Ok(items)
}

fn file_item(file: &FileRef) -> Result<Value> {
    serde_json::to_value(file)
        .map_err(|error| DagError::Schedule(format!("cannot serialize file channel item: {error}")))
}

fn pointer_path(path: &str) -> String {
    format!("/{}", path.trim_start_matches('.').replace('.', "/"))
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn branch_matches(branch: &ChannelBranch, item: &Value) -> bool {
    let actual = item.pointer(&pointer_path(&branch.path));
    if actual.is_none() {
        return branch.exists == Some(false);
    }
    if let Some(exists) = branch.exists
        && actual.is_some() != exists
    {
        return false;
    }
    if let Some(expected) = &branch.equals
        && actual != Some(expected)
    {
        return false;
    }
    if let Some(unexpected) = &branch.not_equals
        && actual == Some(unexpected)
    {
        return false;
    }
    let text = actual.and_then(Value::as_str);
    if let Some(expected) = &branch.prefix
        && !text.is_some_and(|value| value.starts_with(expected))
    {
        return false;
    }
    if let Some(expected) = &branch.suffix
        && !text.is_some_and(|value| value.ends_with(expected))
    {
        return false;
    }
    if let Some(expected) = &branch.contains
        && !text.is_some_and(|value| value.contains(expected.as_str()))
    {
        return false;
    }
    true
}

#[async_trait::async_trait]
impl DagNode for ChannelNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let expected = if matches!(self.operator, ChannelOperator::Collect) {
            0
        } else {
            input_count(&self.operator)
        };
        let actual = if matches!(self.operator, ChannelOperator::Collect) {
            inputs.len()
        } else {
            expected
        };
        if actual < expected {
            return Err(DagError::Schedule(format!(
                "channel operator {:?} requires {expected} input(s), got {actual}",
                self.operator
            )));
        }
        let mut outputs = PortOutputs::new();
        if let ChannelOperator::OfItems { items } = &self.operator {
            for (sequence, item) in items.iter().enumerate() {
                reporter
                    .emit_channel_item(0, sequence as u64, item.clone())
                    .await;
                tokio::task::yield_now().await;
            }
            reporter.close_channel(0, items.len() as u64).await;
            outputs.insert(
                0,
                ChannelValue {
                    items: items.clone(),
                },
            );
        } else if let ChannelOperator::Branch { branches } = &self.operator {
            let mut branch_outputs = branches
                .iter()
                .map(|_| ChannelValue::default())
                .collect::<Vec<_>>();
            for item in channel_items(inputs, 0)? {
                if let Some((index, _)) = branches
                    .iter()
                    .enumerate()
                    .find(|(_, branch)| branch_matches(branch, &item))
                {
                    branch_outputs[index].items.push(item);
                }
            }
            for (port, channel) in branch_outputs.into_iter().enumerate() {
                outputs.insert(port as u8, channel.via_bounded_stream(128).await?);
            }
        } else {
            let channel = self.apply(inputs)?;
            outputs.insert(0, channel.via_bounded_stream(128).await?);
        }
        Ok(outputs)
    }

    fn kind(&self) -> &'static str {
        "channel"
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ChannelNode {
    pub async fn process_stream_item(
        &mut self,
        input_port: u8,
        item: serde_json::Value,
    ) -> Result<Vec<(u8, serde_json::Value)>> {
        if input_port != 0 {
            return Err(DagError::Schedule(format!(
                "streaming channel operator does not accept input port {input_port}"
            )));
        }

        let outputs = match self.operator.clone() {
            ChannelOperator::Map { template } => {
                vec![(0, render_spec(&template, &item)?)]
            }
            ChannelOperator::Filter { path, equals } => {
                if item.pointer(&pointer_path(&path)) == Some(&equals) {
                    vec![(0, item)]
                } else {
                    Vec::new()
                }
            }
            ChannelOperator::Flatten => match item {
                serde_json::Value::Array(values) => values
                    .into_iter()
                    .map(|value| Ok((0, value)))
                    .collect::<Result<Vec<_>>>()?,
                value => {
                    return Err(DagError::Schedule(format!(
                        "channel.flatten requires array items, got {value}"
                    )));
                }
            },
            ChannelOperator::Branch { branches } => {
                match branches
                    .iter()
                    .enumerate()
                    .find(|(_, branch)| branch_matches(branch, &item))
                {
                    Some((index, _)) => vec![(index as u8, item)],
                    None => Vec::new(),
                }
            }
            operator => {
                return Err(DagError::Schedule(format!(
                    "channel operator `{operator:?}` does not support streaming item execution"
                )));
            }
        };
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel_input(port: u8, items: Vec<Value>) -> NodeInput {
        NodeInput {
            port,
            data: NodeValue::Channel(ChannelValue { items }),
        }
    }

    async fn apply(operator: ChannelOperator, inputs: Vec<NodeInput>) -> ChannelValue {
        let outputs = execute_node(ChannelNode::new(operator), inputs).await;
        outputs[&0].as_channel().unwrap().clone()
    }

    async fn execute_node(
        mut node: ChannelNode,
        inputs: Vec<NodeInput>,
    ) -> crate::dag::graph::PortOutputs {
        let context = crate::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let reporter = NodeReporter::new("channel-test", sender);
        node.execute(&context, &inputs, &reporter).await.unwrap()
    }

    #[tokio::test]
    async fn stateless_operators_apply_stream_items_incrementally() {
        let mut map = ChannelNode::new(ChannelOperator::Map {
            template: serde_json::json!({"id": "{{item.id}}"}),
        });
        assert_eq!(
            map.process_stream_item(0, serde_json::json!({"id": "one", "kind": "keep"}))
                .await
                .unwrap(),
            vec![(0, serde_json::json!({"id": "one"}))]
        );

        let mut filter = ChannelNode::new(ChannelOperator::Filter {
            path: "kind".into(),
            equals: serde_json::json!("keep"),
        });
        assert_eq!(
            filter
                .process_stream_item(0, serde_json::json!({"kind": "keep"}))
                .await
                .unwrap(),
            vec![(0, serde_json::json!({"kind": "keep"}))]
        );
        assert!(
            filter
                .process_stream_item(0, serde_json::json!({"kind": "drop"}))
                .await
                .unwrap()
                .is_empty()
        );

        let mut flatten = ChannelNode::new(ChannelOperator::Flatten);
        assert_eq!(
            flatten
                .process_stream_item(0, serde_json::json!([1, 2]))
                .await
                .unwrap(),
            vec![(0, serde_json::json!(1)), (0, serde_json::json!(2))]
        );

        let mut branch = ChannelNode::new(ChannelOperator::Branch {
            branches: vec![
                crate::dag::ChannelBranch {
                    name: "keep".into(),
                    path: "kind".into(),
                    equals: Some(serde_json::json!("keep")),
                    not_equals: None,
                    exists: None,
                    prefix: None,
                    suffix: None,
                    contains: None,
                },
                crate::dag::ChannelBranch {
                    name: "other".into(),
                    path: "kind".into(),
                    equals: Some(serde_json::json!("other")),
                    not_equals: None,
                    exists: None,
                    prefix: None,
                    suffix: None,
                    contains: None,
                },
            ],
        });
        assert_eq!(
            branch
                .process_stream_item(0, serde_json::json!({"kind": "other"}))
                .await
                .unwrap(),
            vec![(1, serde_json::json!({"kind": "other"}))]
        );
    }

    #[tokio::test]
    async fn bounded_channel_preserves_item_order_under_capacity_one() {
        let items = (0..500)
            .map(|index| serde_json::json!(index))
            .collect::<Vec<_>>();
        let channel = ChannelValue { items }.via_bounded_stream(1).await.unwrap();
        assert_eq!(
            channel.items,
            (0..500)
                .map(|index| serde_json::json!(index))
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn branch_routes_items_to_labeled_output_ports() {
        let spec = serde_json::json!({
            "operator": "branch",
            "branches": [
                {"name": "high", "path": "score", "not_equals": 0},
                {"name": "zero", "path": "score", "equals": 0},
                {"name": "text", "path": "id", "prefix": "x"}
            ]
        });
        let node = ChannelNode::from_spec(&spec).unwrap();
        assert_eq!(
            node.ports().output_port(0).unwrap().label.as_deref(),
            Some("high")
        );
        assert_eq!(
            node.ports().output_port(1).unwrap().label.as_deref(),
            Some("zero")
        );
        assert_eq!(
            node.ports().output_port(2).unwrap().label.as_deref(),
            Some("text")
        );

        let inputs = vec![channel_input(
            0,
            vec![
                serde_json::json!({"id": "one", "score": 10}),
                serde_json::json!({"id": "two", "score": 0}),
                serde_json::json!({"id": "x-three", "score": 0}),
                serde_json::json!({"id": "unmatched"}),
            ],
        )];
        let outputs = execute_node(node, inputs).await;
        let high = outputs[&0].as_channel().unwrap();
        let zero = outputs[&1].as_channel().unwrap();
        let text = outputs[&2].as_channel().unwrap();
        assert_eq!(
            high.items,
            vec![serde_json::json!({"id": "one", "score": 10})]
        );
        assert_eq!(
            zero.items,
            vec![
                serde_json::json!({"id": "two", "score": 0}),
                serde_json::json!({"id": "x-three", "score": 0})
            ]
        );
        assert!(text.items.is_empty());
    }

    #[test]
    fn branch_specifications_require_predicates_and_unique_outputs() {
        let duplicate = serde_json::json!({
            "operator": "branch",
            "branches": [
                {"name": "same", "path": "id", "equals": "a"},
                {"name": "same", "path": "id", "equals": "b"}
            ]
        });
        let error = ChannelNode::from_spec(&duplicate).unwrap_err();
        assert!(error.to_string().contains("duplicate output"));

        let missing_predicate =
            serde_json::json!({"operator": "branch", "branches": [{"name": "out", "path": "id"}]});
        let error = ChannelNode::from_spec(&missing_predicate).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires at least one predicate")
        );
    }

    #[tokio::test]
    async fn maps_and_filters_json_items() {
        let items = vec![
            serde_json::json!({"id": "one", "type": "keep"}),
            serde_json::json!({"id": "two", "type": "drop"}),
        ];
        let mapped = apply(
            ChannelOperator::Map {
                template: serde_json::json!({"id": "{{item.id}}", "mapped": true}),
            },
            vec![channel_input(0, items)],
        )
        .await;
        assert_eq!(mapped.items.len(), 2);
        assert_eq!(mapped.items[0]["id"], "one");
        assert_eq!(mapped.items[1]["mapped"], true);

        let filtered = apply(
            ChannelOperator::Filter {
                path: "type".into(),
                equals: serde_json::json!("keep"),
            },
            vec![channel_input(
                0,
                vec![
                    serde_json::json!({"type": "keep"}),
                    serde_json::json!({"type": "drop"}),
                ],
            )],
        )
        .await;
        assert_eq!(filtered.items, vec![serde_json::json!({"type": "keep"})]);
    }

    #[tokio::test]
    async fn flattens_mixes_collects_and_combines() {
        let flattened = apply(
            ChannelOperator::Flatten,
            vec![channel_input(
                0,
                vec![serde_json::json!([1, 2]), serde_json::json!([3])],
            )],
        )
        .await;
        assert_eq!(
            flattened.items,
            vec![
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!(3)
            ]
        );

        let mixed = apply(
            ChannelOperator::Mix,
            vec![
                channel_input(0, vec![serde_json::json!("a")]),
                channel_input(1, vec![serde_json::json!("b")]),
            ],
        )
        .await;
        assert_eq!(mixed.items.len(), 2);

        let collected = apply(
            ChannelOperator::Collect,
            vec![NodeInput {
                port: 0,
                data: NodeValue::File(FileRef::new("/one.csv", Some("csv".into()))),
            }],
        )
        .await;
        assert_eq!(collected.items[0]["path"], "/one.csv");

        let combined = apply(
            ChannelOperator::Combine,
            vec![
                channel_input(0, vec![serde_json::json!(1), serde_json::json!(2)]),
                channel_input(1, vec![serde_json::json!("x"), serde_json::json!("y")]),
            ],
        )
        .await;
        assert_eq!(combined.items.len(), 4);
        assert_eq!(combined.items[0]["left"], 1);
        assert_eq!(combined.items[0]["right"], "x");
    }

    #[tokio::test]
    async fn joins_and_groups_keyed_items() {
        let joined = apply(
            ChannelOperator::Join {
                left_key: "id".into(),
                right_key: "sample".into(),
            },
            vec![
                channel_input(0, vec![serde_json::json!({"id": "a", "value": 1})]),
                channel_input(1, vec![serde_json::json!({"sample": "a", "score": 2})]),
            ],
        )
        .await;
        assert_eq!(joined.items.len(), 1);
        assert_eq!(joined.items[0]["left_id"], "a");
        assert_eq!(joined.items[0]["right_sample"], "a");
        assert_eq!(joined.items[0]["left_value"], 1);
        assert_eq!(joined.items[0]["right_score"], 2);

        let grouped = apply(
            ChannelOperator::GroupTuple { key: "type".into() },
            vec![channel_input(
                0,
                vec![
                    serde_json::json!({"type": "a", "id": 1}),
                    serde_json::json!({"type": "b", "id": 2}),
                    serde_json::json!({"type": "a", "id": 3}),
                ],
            )],
        )
        .await;
        assert_eq!(grouped.items.len(), 2);
        assert_eq!(grouped.items[0][0], serde_json::json!("a"));
        assert_eq!(grouped.items[0][1].as_array().unwrap().len(), 2);
    }
}
