//! Executor adapter for the run-local dataflow coordinator.
use crate::{
    cache::InstanceCache,
    dataflow::{Dataflow, Delivery},
    error::Error,
    instance::{InstanceId, render_spec},
    model::{ErrorMode, FlowGraph},
    plan::SourceResolution,
    scheduler::{FlowRunConfig, FlowRunReport, FlowScheduler, InstanceRecord, ProcessRunSummary},
    source::SourceStreams,
};
use dag_core::dag::execution::{
    TaskExecution, TaskInputBinding, TaskInputSource, TaskOutputBinding, TaskSpec, TaskSubmission,
};
use dag_core::dag::node_event::{JobResult, NodeEvent, NodeEventKind, NodeReporter};
use dag_core::dag::{NodeInput, RuntimeStatus};
use dag_core::value::{ChannelValue, FileRef, NodeValue};
use futures::FutureExt;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

struct SourceState<'a> {
    source: &'a crate::model::Source,
    reader: mpsc::Receiver<Result<serde_json::Value, Error>>,
    pending: Option<Option<Result<serde_json::Value, Error>>>,
    closed: bool,
}

async fn wait_source(
    states: &mut [SourceState<'_>],
    flow: &Dataflow<NodeValue>,
) -> (usize, Option<Result<serde_json::Value, Error>>) {
    let readers: Vec<_> = states
        .iter_mut()
        .enumerate()
        .filter(|(_, s)| !s.closed && s.pending.is_none() && flow.can_emit(&s.source.id, 0, 128))
        .map(|(index, state)| Box::pin(async move { (index, state.reader.recv().await) }))
        .collect();
    if readers.is_empty() {
        return std::future::pending().await;
    }
    futures::future::select_all(readers).await.0
}

fn json_value(value: &NodeValue) -> serde_json::Value {
    match value {
        NodeValue::File(f) => serde_json::json!(f.path),
        NodeValue::FileSet(files) => {
            serde_json::json!(files.iter().map(|f| &f.path).collect::<Vec<_>>())
        }
        NodeValue::Channel(c) if c.items.len() == 1 => c.items[0].clone(),
        NodeValue::Channel(c) => serde_json::json!(c.items),
        NodeValue::Structured(value) => {
            serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
        }
        NodeValue::DataFrame(_) => serde_json::Value::Null,
    }
}

fn matches_literal_criteria(value: &NodeValue, criteria: &serde_json::Value) -> bool {
    use dag_core::dag::execution::ChannelTransportValue as T;
    if let NodeValue::Structured(value) = value {
        let actual = group_key(value);
        return match criteria {
            serde_json::Value::Array(items) => items
                .iter()
                .any(|item| group_key(&T::Json(item.clone())) == actual),
            other => group_key(&T::Json(other.clone())) == actual,
        };
    }
    if matches!(
        value,
        NodeValue::File(_) | NodeValue::FileSet(_) | NodeValue::DataFrame(_)
    ) {
        return false;
    }
    let item = json_value(value);
    match criteria {
        serde_json::Value::Array(items) => items.contains(&item),
        other => other == &item,
    }
}

fn json_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|n| n != 0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
    }
}

fn node_truthy(value: &NodeValue) -> bool {
    use dag_core::dag::execution::ChannelTransportValue as T;
    match value {
        NodeValue::Channel(_) => json_truthy(&json_value(value)),
        NodeValue::FileSet(files) => !files.is_empty(),
        NodeValue::Structured(T::Json(value)) => json_truthy(value),
        NodeValue::Structured(T::List(items) | T::Tuple(items)) => !items.is_empty(),
        NodeValue::Structured(T::Map(items)) => !items.is_empty(),
        _ => true,
    }
}

fn source_value(value: &serde_json::Value) -> NodeValue {
    NodeValue::Channel(ChannelValue {
        items: vec![value.clone()],
    })
}
fn file_value(path: &str) -> NodeValue {
    NodeValue::File(FileRef {
        path: path.to_owned(),
        format: None,
        fingerprint: None,
    })
}
fn coerce_file_inputs(inputs: &mut [NodeInput], node: &dyn dag_core::node::DagNode) {
    use dag_core::value::PortType;
    for input in inputs {
        let Some(port) = node
            .ports()
            .input_ports()
            .iter()
            .find(|p| p.index == input.port)
        else {
            continue;
        };
        if port.data_type == PortType::Channel {
            match &input.data {
                NodeValue::File(file) => {
                    input.data = NodeValue::Channel(ChannelValue {
                        items: vec![serde_json::json!(file.path)],
                    })
                }
                NodeValue::FileSet(files) => {
                    input.data = NodeValue::Channel(ChannelValue {
                        items: files.iter().map(|f| serde_json::json!(f.path)).collect(),
                    })
                }
                _ => {}
            }
        }
        if let NodeValue::Channel(channel) = &input.data {
            if port.data_type == PortType::File && channel.items.len() == 1 {
                if let Some(path) = channel.items[0].as_str() {
                    input.data = file_value(path);
                }
            } else if port.data_type == PortType::FileSet {
                let paths: Option<Vec<_>> = channel
                    .items
                    .iter()
                    .map(|v| {
                        v.as_str().map(|path| FileRef {
                            path: path.to_owned(),
                            format: None,
                            fingerprint: None,
                        })
                    })
                    .collect();
                if let Some(paths) = paths {
                    input.data = NodeValue::FileSet(paths);
                }
            }
        }
    }
}

/// Natural ordering for JSON scalar values, with exact comparisons for integers.
fn sort_channel_values(items: &mut [serde_json::Value]) -> Result<(), Error> {
    use serde_json::Value;
    let category = |v: &Value| match v {
        Value::Null => Some(0),
        Value::Bool(_) => Some(1),
        Value::Number(_) => Some(2),
        Value::String(_) => Some(3),
        _ => None,
    };
    let mut kind = None;
    for item in items.iter().filter(|item| !item.is_null()) {
        let current = category(item)
            .ok_or_else(|| Error::Execution("natural sort supports scalar values only".into()))?;
        if kind.is_some_and(|prior| prior != current) {
            return Err(Error::Execution(
                "natural sort requires mutually comparable values".into(),
            ));
        }
        kind = Some(current);
    }
    let has_float = items
        .iter()
        .any(|item| item.as_number().is_some_and(|n| n.is_f64()));
    if has_float
        && items.iter().any(|item| {
            item.as_number().is_some_and(|n| {
                n.as_i64().is_some_and(|x| x.unsigned_abs() > (1_u64 << 53))
                    || n.as_u64().is_some_and(|x| x > (1_u64 << 53))
            })
        })
    {
        return Err(Error::Execution(
            "cannot exactly compare large integers with floating-point values".into(),
        ));
    }
    items.sort_by(compare_sorted_json);
    Ok(())
}

fn compare_sorted_json(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    use serde_json::Value;
    match (a, b) {
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Number(a), Value::Number(b)) => {
            let integer = |n: &serde_json::Number| {
                n.as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from))
            };
            match (integer(a), integer(b)) {
                (Some(a), Some(b)) => a.cmp(&b),
                _ => a.as_f64().unwrap().total_cmp(&b.as_f64().unwrap()),
            }
        }
        _ => unreachable!("values were checked before sorting"),
    }
}

/// Sort channel values without losing typed file references.
fn sort_channel_transport_values(
    items: &mut [dag_core::dag::execution::ChannelTransportValue],
) -> Result<(), Error> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    if items.iter().all(|item| matches!(item, T::File(_))) {
        items.sort_by(|a, b| match (a, b) {
            (T::File(a), T::File(b)) => a.path.cmp(&b.path),
            _ => unreachable!("file values checked before sorting"),
        });
        return Ok(());
    }
    let mut json = items
        .iter()
        .map(|item| match item {
            T::Json(value) => Some(value.clone()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            Error::Execution("natural sort requires mutually comparable values".into())
        })?;
    sort_channel_values(&mut json)?;
    for (item, value) in items.iter_mut().zip(json) {
        *item = T::Json(value);
    }
    Ok(())
}

fn sort_channel_values_by_function(
    values: Vec<dag_core::dag::execution::ChannelTransportValue>,
    function: &serde_json::Value,
    evaluator: &crate::ChannelFunction,
) -> Result<NodeValue, String> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    let mut keyed = Vec::with_capacity(values.len());
    for value in values {
        let input = match &value {
            T::Json(value) => source_value(value),
            T::File(file) => NodeValue::File(file.clone()),
            value => NodeValue::Structured(value.clone()),
        };
        let key =
            node_transport_value(evaluator(function, Some(&input))?).map_err(|e| e.to_string())?;
        keyed.push((key, value));
    }
    let mut keys = keyed.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>();
    sort_channel_transport_values(&mut keys).map_err(|e| e.to_string())?;
    keyed.sort_by(|(a, _), (b, _)| match (a, b) {
        (T::File(a), T::File(b)) => a.path.cmp(&b.path),
        (T::Json(a), T::Json(b)) => compare_sorted_json(a, b),
        _ => unreachable!("keys were validated before sorting"),
    });
    Ok(collection_value(
        keyed.into_iter().map(|(_, value)| value).collect(),
    ))
}

type JoinRows = (
    VecDeque<Vec<serde_json::Value>>,
    VecDeque<Vec<serde_json::Value>>,
);

struct PendingJoin {
    groups: BTreeMap<String, JoinRows>,
    singleton: [Option<bool>; 2],
    typed_keys: Option<BTreeMap<String, Vec<serde_json::Value>>>,
}
fn deliver_pending_join(
    flow: &mut Dataflow<NodeValue>,
    name: &str,
    pending: &mut PendingJoin,
    capacity: usize,
) -> Result<bool, Error> {
    while !pending.groups.is_empty() && flow.can_emit(name, 0, capacity) {
        let mut entry = pending.groups.first_entry().expect("nonempty join state");
        let mut row: Vec<serde_json::Value> = if let Some(keys) = &pending.typed_keys {
            keys.get(entry.key())
                .cloned()
                .ok_or_else(|| Error::Execution("missing join key".into()))?
        } else {
            serde_json::from_str(entry.key()).map_err(|e| Error::Execution(e.to_string()))?
        };
        let (left, right) = entry.get_mut();
        for (i, values) in [left, right].into_iter().enumerate() {
            if let Some(values) = values.pop_front() {
                row.extend(values);
            } else if !pending.singleton[i]
                .or(pending.singleton[1 - i])
                .unwrap_or(false)
            {
                row.push(if pending.typed_keys.is_some() {
                    serde_json::to_value(dag_core::dag::execution::ChannelTransportValue::Json(
                        serde_json::Value::Null,
                    ))
                    .map_err(|e| Error::Execution(e.to_string()))?
                } else {
                    serde_json::Value::Null
                });
            }
        }
        if entry.get().0.is_empty() && entry.get().1.is_empty() {
            entry.remove();
        }
        let output = if pending.typed_keys.is_some() {
            typed_flat_output(row)?
        } else if row.len() == 1 {
            source_value(&row.pop().unwrap())
        } else {
            source_value(&serde_json::Value::Array(row))
        };
        flow.emit(name, 0, output).map_err(Error::Execution)?;
    }
    Ok(pending.groups.is_empty())
}

type CombineRows = (Vec<Vec<serde_json::Value>>, Vec<Vec<serde_json::Value>>);
type CombineState = BTreeMap<String, BTreeMap<String, CombineRows>>;

struct PendingCombine {
    key: String,
    keys: Vec<serde_json::Value>,
    side: u8,
    own_index: usize,
    next: usize,
    count: usize,
    typed: bool,
}

fn deliver_pending_combine(
    flow: &mut Dataflow<NodeValue>,
    name: &str,
    pending: &mut PendingCombine,
    states: &CombineState,
    capacity: usize,
) -> Result<bool, Error> {
    let (left, right) = &states[name][&pending.key];
    while pending.next < pending.count && flow.can_emit(name, 0, capacity) {
        let (a, b) = if pending.side == 0 {
            (&left[pending.own_index], &right[pending.next])
        } else {
            (&left[pending.next], &right[pending.own_index])
        };
        let mut tuple = pending.keys.clone();
        tuple.extend(a.iter().cloned());
        tuple.extend(b.iter().cloned());
        let value = if pending.typed {
            typed_flat_output(tuple)?
        } else if tuple.len() == 1 {
            source_value(&tuple.pop().unwrap())
        } else {
            source_value(&serde_json::Value::Array(tuple))
        };
        flow.emit(name, 0, value).map_err(Error::Execution)?;
        pending.next += 1;
    }
    Ok(pending.next == pending.count)
}

#[derive(Default)]
struct GroupState {
    groups: std::collections::HashMap<String, GroupEntry>,
    order: BTreeMap<u64, String>,
    next_order: u64,
    typed: bool,
}
struct GroupEntry {
    order: u64,
    tuple: Vec<serde_json::Value>,
    count: usize,
}
impl GroupState {
    fn add(
        &mut self,
        key: String,
        tuple: Vec<serde_json::Value>,
        pivot: &[usize],
        size: Option<usize>,
    ) -> Result<Option<Vec<serde_json::Value>>, Error> {
        if !self.groups.contains_key(&key) {
            let order = self.next_order;
            self.next_order = self
                .next_order
                .checked_add(1)
                .ok_or_else(|| Error::Execution("group_tuple order overflow".into()))?;
            let grouped = tuple
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    if pivot.contains(&i) {
                        v.clone()
                    } else {
                        serde_json::json!([])
                    }
                })
                .collect();
            self.order.insert(order, key.clone());
            self.groups.insert(
                key.clone(),
                GroupEntry {
                    order,
                    tuple: grouped,
                    count: 0,
                },
            );
        }
        let group = self.groups.get_mut(&key).expect("group inserted");
        if group.tuple.len() != tuple.len() {
            return Err(Error::Execution(
                "group_tuple received inconsistent tuple lengths".into(),
            ));
        }
        for (i, value) in tuple.into_iter().enumerate() {
            if !pivot.contains(&i) {
                group.tuple[i]
                    .as_array_mut()
                    .expect("group list")
                    .push(value);
            }
        }
        group.count += 1;
        if size == Some(group.count) {
            let group = self.groups.remove(&key).expect("group present");
            self.order.remove(&group.order);
            return Ok(Some(group.tuple));
        }
        Ok(None)
    }
    fn pop_first(&mut self) -> Option<(Vec<serde_json::Value>, usize)> {
        let (_, key) = self.order.pop_first()?;
        let group = self.groups.remove(&key).expect("ordered group present");
        Some((group.tuple, group.count))
    }
}

struct PendingGroups {
    groups: GroupState,
    pivot: Vec<usize>,
    size: Option<usize>,
    remainder: bool,
    sort: bool,
}

fn deliver_pending_groups(
    flow: &mut Dataflow<NodeValue>,
    name: &str,
    pending: &mut PendingGroups,
    capacity: usize,
) -> Result<bool, Error> {
    while !pending.groups.order.is_empty() && flow.can_emit(name, 0, capacity) {
        let (mut tuple, count) = pending.groups.pop_first().expect("nonempty group state");
        if pending.size.is_none_or(|size| count == size) || pending.remainder {
            let output = if pending.groups.typed {
                typed_group_output(tuple, &pending.pivot, pending.sort)?
            } else {
                if pending.sort {
                    sort_group_tuple(&mut tuple, &pending.pivot)?;
                }
                source_value(&serde_json::Value::Array(tuple))
            };
            flow.emit(name, 0, output).map_err(Error::Execution)?;
        }
    }
    Ok(pending.groups.order.is_empty())
}

fn typed_group_output(
    tuple: Vec<serde_json::Value>,
    keys: &[usize],
    sort: bool,
) -> Result<NodeValue, Error> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    let mut result = Vec::with_capacity(tuple.len());
    for (index, column) in tuple.into_iter().enumerate() {
        if keys.contains(&index) {
            result.push(
                serde_json::from_value::<T>(column).map_err(|e| Error::Execution(e.to_string()))?,
            );
        } else {
            let values = column
                .as_array()
                .ok_or_else(|| Error::Execution("invalid grouped column".into()))?;
            let mut values = values
                .iter()
                .cloned()
                .map(serde_json::from_value::<T>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| Error::Execution(e.to_string()))?;
            if sort {
                sort_channel_transport_values(&mut values)?;
            }
            result.push(T::List(values));
        }
    }
    Ok(typed_node_value(T::Tuple(result)))
}

fn typed_node_value(value: dag_core::dag::execution::ChannelTransportValue) -> NodeValue {
    use dag_core::dag::execution::ChannelTransportValue as T;
    fn json(value: &T) -> Option<serde_json::Value> {
        match value {
            T::Json(value) => Some(value.clone()),
            T::List(values) | T::Tuple(values) => values
                .iter()
                .map(json)
                .collect::<Option<Vec<_>>>()
                .map(serde_json::Value::Array),
            T::Map(values) => values
                .iter()
                .map(|(k, v)| Some((k.clone(), json(v)?)))
                .collect::<Option<serde_json::Map<_, _>>>()
                .map(serde_json::Value::Object),
            T::File(_) | T::Arrow(_) => None,
        }
    }
    match value {
        T::File(file) => NodeValue::File(file),
        other => match json(&other) {
            Some(value) => source_value(&value),
            None => NodeValue::Structured(other),
        },
    }
}

fn typed_flat_output(values: Vec<serde_json::Value>) -> Result<NodeValue, Error> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    let mut values = values
        .into_iter()
        .map(serde_json::from_value::<T>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Error::Execution(e.to_string()))?;
    Ok(typed_node_value(if values.len() == 1 {
        values.pop().unwrap()
    } else {
        T::Tuple(values)
    }))
}

fn channel_fields(value: NodeValue) -> Vec<dag_core::dag::execution::ChannelTransportValue> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    match value {
        NodeValue::Structured(T::Tuple(values) | T::List(values)) => values,
        NodeValue::Structured(T::Json(serde_json::Value::Array(values))) => {
            values.into_iter().map(T::Json).collect()
        }
        NodeValue::Structured(value) => vec![value],
        NodeValue::File(file) => vec![T::File(file)],
        NodeValue::FileSet(files) => files.into_iter().map(T::File).collect(),
        other => match json_value(&other) {
            serde_json::Value::Array(values) => values.into_iter().map(T::Json).collect(),
            value => vec![T::Json(value)],
        },
    }
}

pub(crate) fn group_key(
    value: &dag_core::dag::execution::ChannelTransportValue,
) -> serde_json::Value {
    use dag_core::dag::execution::ChannelTransportValue as T;
    match value {
        T::File(file) => serde_json::json!(["file", file.path]),
        T::Arrow(file) => serde_json::json!(["arrow", file.path]),
        T::Json(serde_json::Value::Array(values)) => serde_json::json!([
            "list",
            values
                .iter()
                .cloned()
                .map(|value| group_key(&T::Json(value)))
                .collect::<Vec<_>>()
        ]),
        T::Json(serde_json::Value::Object(values)) => serde_json::json!([
            "map",
            values
                .iter()
                .map(|(key, value)| (key.clone(), group_key(&T::Json(value.clone()))))
                .collect::<BTreeMap<_, _>>()
        ]),
        T::Json(value) => serde_json::json!(["json", value]),
        T::List(values) | T::Tuple(values) => {
            serde_json::json!(["list", values.iter().map(group_key).collect::<Vec<_>>()])
        }
        T::Map(values) => serde_json::json!([
            "map",
            values
                .iter()
                .map(|(key, value)| (key.clone(), group_key(value)))
                .collect::<BTreeMap<_, _>>()
        ]),
    }
}

fn node_comparison_key(value: &NodeValue) -> Result<String, Error> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    let value = match value {
        NodeValue::File(file) => T::File(file.clone()),
        NodeValue::FileSet(files) => T::List(files.iter().cloned().map(T::File).collect()),
        NodeValue::Structured(value) => value.clone(),
        NodeValue::Channel(_) => T::Json(json_value(value)),
        NodeValue::DataFrame(_) => {
            return Err(Error::Execution("cannot compare DataFrame values".into()));
        }
    };
    serde_json::to_string(&group_key(&value)).map_err(|e| Error::Execution(e.to_string()))
}

fn node_is_null(value: &NodeValue) -> bool {
    match value {
        NodeValue::Structured(dag_core::dag::execution::ChannelTransportValue::Json(value)) => {
            value.is_null()
        }
        NodeValue::Channel(_) => json_value(value).is_null(),
        _ => false,
    }
}

pub(crate) fn node_transport_value(
    value: NodeValue,
) -> Result<dag_core::dag::execution::ChannelTransportValue, Error> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    Ok(match value {
        NodeValue::File(file) => T::File(file),
        NodeValue::FileSet(files) => T::List(files.into_iter().map(T::File).collect()),
        NodeValue::Structured(value) => value,
        NodeValue::Channel(channel) if channel.items.len() == 1 => {
            T::Json(channel.items.into_iter().next().unwrap())
        }
        NodeValue::Channel(channel) => T::Json(serde_json::Value::Array(channel.items)),
        NodeValue::DataFrame(_) => {
            return Err(Error::Execution(
                "operator does not support DataFrame values".into(),
            ));
        }
    })
}

/// A predicate sees file metadata while the original typed value continues downstream.
fn predicate_projection(
    value: &dag_core::dag::execution::ChannelTransportValue,
) -> Result<serde_json::Value, Error> {
    use dag_core::dag::execution::ChannelTransportValue as T;
    Ok(match value {
        T::Json(value) => value.clone(),
        T::File(file) | T::Arrow(file) => {
            serde_json::to_value(file).map_err(|e| Error::Execution(e.to_string()))?
        }
        T::List(values) | T::Tuple(values) => serde_json::Value::Array(
            values
                .iter()
                .map(predicate_projection)
                .collect::<Result<_, _>>()?,
        ),
        T::Map(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), predicate_projection(value)?)))
                .collect::<Result<_, Error>>()?,
        ),
    })
}

fn sort_group_tuple(tuple: &mut [serde_json::Value], keys: &[usize]) -> Result<(), Error> {
    for (index, column) in tuple.iter_mut().enumerate() {
        if !keys.contains(&index) {
            let items = column
                .as_array_mut()
                .ok_or_else(|| Error::Execution("invalid grouped column".into()))?;
            sort_channel_values(items)?;
        }
    }
    Ok(())
}

enum FlatMapRows {
    Array(std::vec::IntoIter<serde_json::Value>),
    Map(serde_json::map::IntoIter),
    One(Option<serde_json::Value>),
}

enum FunctionResult {
    Item(Option<NodeValue>),
    Branch(Option<(u8, NodeValue)>),
    Until {
        value: Option<NodeValue>,
        stop: bool,
    },
    Reduce(NodeValue),
    Collect {
        value: NodeValue,
        flatten: bool,
    },
    Dedup {
        key: NodeValue,
        value: NodeValue,
        distinct: bool,
    },
    FlatMap(serde_json::Value),
    FlatFiles(Vec<FileRef>),
    FlatStructured(dag_core::dag::execution::ChannelTransportValue),
    First(Option<NodeValue>),
    Count(bool),
}

enum PendingFlatMap {
    Json(FlatMapRows),
    Files(std::vec::IntoIter<FileRef>),
    Structured(std::vec::IntoIter<dag_core::dag::execution::ChannelTransportValue>),
    Flatten(Vec<dag_core::dag::execution::ChannelTransportValue>),
}
impl PendingFlatMap {
    fn next(&mut self) -> Option<NodeValue> {
        match self {
            Self::Json(rows) => rows.next().map(|value| source_value(&value)),
            Self::Files(files) => files.next().map(NodeValue::File),
            Self::Structured(items) => items.next().map(|item| match item {
                dag_core::dag::execution::ChannelTransportValue::Json(value) => {
                    source_value(&value)
                }
                dag_core::dag::execution::ChannelTransportValue::File(file) => {
                    NodeValue::File(file)
                }
                other => NodeValue::Structured(other),
            }),
            Self::Flatten(stack) => {
                use dag_core::dag::execution::ChannelTransportValue as T;
                while let Some(item) = stack.pop() {
                    match item {
                        T::List(items) | T::Tuple(items) => stack.extend(items.into_iter().rev()),
                        T::Json(serde_json::Value::Array(items)) => {
                            stack.extend(items.into_iter().rev().map(T::Json))
                        }
                        other => return Some(typed_node_value(other)),
                    }
                }
                None
            }
        }
    }
    fn done(&self) -> bool {
        match self {
            Self::Json(rows) => rows.done(),
            Self::Files(files) => files.len() == 0,
            Self::Structured(items) => items.len() == 0,
            Self::Flatten(stack) => stack.is_empty(),
        }
    }
}
impl FlatMapRows {
    fn new(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Array(items) => Self::Array(items.into_iter()),
            serde_json::Value::Object(map) => Self::Map(map.into_iter()),
            other => Self::One(Some(other)),
        }
    }
    fn done(&self) -> bool {
        match self {
            Self::Array(items) => items.len() == 0,
            Self::Map(items) => items.len() == 0,
            Self::One(item) => item.is_none(),
        }
    }
}
impl Iterator for FlatMapRows {
    type Item = serde_json::Value;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Array(items) => items.next(),
            Self::Map(items) => items
                .next()
                .map(|(key, value)| serde_json::json!([key, value])),
            Self::One(item) => item.take(),
        }
    }
}

struct TransposeRows {
    tuple: Vec<serde_json::Value>,
    indices: Vec<usize>,
    cursor: usize,
    count: usize,
}

struct TypedTransposeRows {
    tuple: Vec<dag_core::dag::execution::ChannelTransportValue>,
    indices: Vec<usize>,
    cursor: usize,
    count: usize,
}

impl TypedTransposeRows {
    fn new(
        tuple: Vec<dag_core::dag::execution::ChannelTransportValue>,
        by: Option<&serde_json::Value>,
        remainder: bool,
    ) -> Result<Self, Error> {
        use dag_core::dag::execution::ChannelTransportValue as T;
        fn len(value: &T) -> Option<usize> {
            match value {
                T::List(items) | T::Tuple(items) => Some(items.len()),
                T::Json(serde_json::Value::Array(items)) => Some(items.len()),
                _ => None,
            }
        }
        let mut indices: Vec<usize> = match by {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(values)) => values
                .iter()
                .map(|v| v.as_u64().expect("validated by") as usize)
                .collect(),
            Some(value) => vec![value.as_u64().expect("validated by") as usize],
        };
        if indices.is_empty() {
            indices = tuple
                .iter()
                .enumerate()
                .filter_map(|(i, value)| len(value).map(|_| i))
                .collect();
        }
        let count = if indices.is_empty() {
            1
        } else {
            let lengths = indices
                .iter()
                .map(|index| {
                    tuple.get(*index).and_then(len).ok_or_else(|| {
                        Error::Execution(format!(
                            "transpose element at index {index} is not a list"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if remainder {
                lengths.into_iter().max().unwrap()
            } else {
                lengths.into_iter().min().unwrap()
            }
        };
        Ok(Self {
            tuple,
            indices,
            cursor: 0,
            count,
        })
    }
    fn done(&self) -> bool {
        self.cursor == self.count
    }
    fn next(&mut self) -> Option<NodeValue> {
        use dag_core::dag::execution::ChannelTransportValue as T;
        if self.done() {
            return None;
        }
        let row = self.cursor;
        self.cursor += 1;
        let values = self
            .tuple
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if !self.indices.contains(&index) {
                    return value.clone();
                }
                match value {
                    T::List(items) | T::Tuple(items) => items.get(row).cloned(),
                    T::Json(serde_json::Value::Array(items)) => {
                        items.get(row).cloned().map(T::Json)
                    }
                    _ => unreachable!("transpose indices validated"),
                }
                .unwrap_or(T::Json(serde_json::Value::Null))
            })
            .collect();
        Some(typed_node_value(T::Tuple(values)))
    }
}

enum PendingTranspose {
    Json(TransposeRows),
    Typed(TypedTransposeRows),
}
impl PendingTranspose {
    fn next(&mut self) -> Option<NodeValue> {
        match self {
            Self::Json(rows) => rows.next().map(|value| source_value(&value)),
            Self::Typed(rows) => rows.next(),
        }
    }
    fn done(&self) -> bool {
        match self {
            Self::Json(rows) => rows.done(),
            Self::Typed(rows) => rows.done(),
        }
    }
}

impl TransposeRows {
    fn new(
        item: serde_json::Value,
        by: Option<&serde_json::Value>,
        remainder: bool,
    ) -> Result<Self, Error> {
        let serde_json::Value::Array(tuple) = item else {
            unreachable!("caller checks array");
        };
        let mut indices: Vec<usize> = match by {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(values)) => values
                .iter()
                .map(|v| v.as_u64().expect("validated by") as usize)
                .collect(),
            Some(value) => vec![value.as_u64().expect("validated by") as usize],
        };
        if indices.is_empty() {
            indices = tuple
                .iter()
                .enumerate()
                .filter_map(|(i, v)| v.is_array().then_some(i))
                .collect();
        }
        let count = if indices.is_empty() {
            1
        } else {
            let mut lengths = Vec::new();
            for &index in &indices {
                lengths.push(
                    tuple
                        .get(index)
                        .and_then(|v| v.as_array())
                        .ok_or_else(|| {
                            Error::Execution(format!(
                                "transpose element at index {index} is not a list"
                            ))
                        })?
                        .len(),
                );
            }
            if remainder {
                lengths.into_iter().max().unwrap()
            } else {
                lengths.into_iter().min().unwrap()
            }
        };
        Ok(Self {
            tuple,
            indices,
            cursor: 0,
            count,
        })
    }
    fn done(&self) -> bool {
        self.cursor == self.count
    }
}
impl Iterator for TransposeRows {
    type Item = serde_json::Value;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done() {
            return None;
        }
        let row = self.cursor;
        self.cursor += 1;
        Some(serde_json::Value::Array(
            self.tuple
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    if self.indices.contains(&index) {
                        value
                            .as_array()
                            .unwrap()
                            .get(row)
                            .cloned()
                            .unwrap_or(serde_json::Value::Null)
                    } else {
                        value.clone()
                    }
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
fn transpose_tuple(
    item: serde_json::Value,
    by: Option<&serde_json::Value>,
    remainder: bool,
) -> Result<Vec<serde_json::Value>, Error> {
    if !item.is_array() {
        return Ok(vec![item]);
    }
    Ok(TransposeRows::new(item, by, remainder)?.collect())
}

#[derive(Default)]
struct BufferState {
    skipped: usize,
    items: Vec<serde_json::Value>,
}

impl BufferState {
    fn push(
        &mut self,
        item: serde_json::Value,
        size: usize,
        skip: usize,
    ) -> Option<Vec<serde_json::Value>> {
        if self.skipped < skip {
            self.skipped += 1;
            return None;
        }
        self.items.push(item);
        if self.items.len() == size {
            self.skipped = 0;
            return Some(std::mem::take(&mut self.items));
        }
        None
    }

    fn finish(&mut self, remainder: bool) -> Option<Vec<serde_json::Value>> {
        if remainder && !self.items.is_empty() {
            Some(std::mem::take(&mut self.items))
        } else {
            None
        }
    }
}

fn collect(values: Vec<NodeValue>) -> NodeValue {
    if values
        .iter()
        .all(|v| matches!(v, NodeValue::File(_) | NodeValue::FileSet(_)))
    {
        NodeValue::FileSet(
            values
                .into_iter()
                .flat_map(|v| match v {
                    NodeValue::File(f) => vec![f],
                    NodeValue::FileSet(f) => f,
                    _ => vec![],
                })
                .collect(),
        )
    } else if values.iter().any(|v| {
        matches!(
            v,
            NodeValue::File(_) | NodeValue::FileSet(_) | NodeValue::Structured(_)
        )
    }) {
        let mut items = Vec::new();
        for value in &values {
            append_collection(&mut items, value, true);
        }
        collection_value(items)
    } else {
        NodeValue::Channel(ChannelValue {
            items: values
                .iter()
                .flat_map(|value| match json_value(value) {
                    serde_json::Value::Array(items) => items,
                    item => vec![item],
                })
                .collect(),
        })
    }
}

fn append_collection(
    items: &mut Vec<dag_core::dag::execution::ChannelTransportValue>,
    value: &NodeValue,
    flatten: bool,
) {
    use dag_core::dag::execution::ChannelTransportValue as T;
    let item = match value {
        NodeValue::File(file) => T::File(file.clone()),
        NodeValue::FileSet(files) => T::List(files.iter().cloned().map(T::File).collect()),
        NodeValue::Structured(value) => value.clone(),
        NodeValue::Channel(_) => T::Json(json_value(value)),
        NodeValue::DataFrame(_) => unreachable!("dataframes are rejected before collection"),
    };
    match item {
        T::List(values) | T::Tuple(values) if flatten => items.extend(values),
        T::Json(serde_json::Value::Array(values)) if flatten => {
            items.extend(values.into_iter().map(T::Json))
        }
        other => items.push(other),
    }
}

fn collection_value(items: Vec<dag_core::dag::execution::ChannelTransportValue>) -> NodeValue {
    use dag_core::dag::execution::ChannelTransportValue as T;
    if !items.is_empty() && items.iter().all(|item| matches!(item, T::File(_))) {
        NodeValue::FileSet(
            items
                .into_iter()
                .map(|item| match item {
                    T::File(file) => file,
                    _ => unreachable!(),
                })
                .collect(),
        )
    } else if items.iter().all(|item| matches!(item, T::Json(_))) {
        let values = items
            .into_iter()
            .map(|item| match item {
                T::Json(value) => value,
                _ => unreachable!(),
            })
            .collect();
        source_value(&serde_json::Value::Array(values))
    } else {
        NodeValue::Structured(T::List(items))
    }
}

fn collect_item_is_empty(value: &NodeValue) -> bool {
    match value {
        NodeValue::FileSet(files) => files.is_empty(),
        NodeValue::Channel(channel) => match channel.items.as_slice() {
            [] => true,
            [serde_json::Value::Array(items)] => items.is_empty(),
            _ => false,
        },
        NodeValue::Structured(
            dag_core::dag::execution::ChannelTransportValue::List(items)
            | dag_core::dag::execution::ChannelTransportValue::Tuple(items),
        ) => items.is_empty(),
        NodeValue::Structured(dag_core::dag::execution::ChannelTransportValue::Json(
            serde_json::Value::Array(items),
        )) => items.is_empty(),
        _ => false,
    }
}
fn deliver_event(
    flow: &mut Dataflow<NodeValue>,
    streamed: &mut BTreeSet<(String, u8)>,
    event: NodeEvent,
) -> Result<(), Error> {
    match event.kind {
        NodeEventKind::ChannelItem { port, item, .. } => {
            streamed.insert((event.node_id.clone(), port));
            let process = event
                .node_id
                .split_once('#')
                .map(|(name, _)| name)
                .ok_or_else(|| Error::Execution("invalid streaming task id".into()))?;
            flow.emit(process, port, source_value(&item))
                .map_err(Error::Execution)?;
        }
        NodeEventKind::ChannelClosed { port, .. } => {
            streamed.insert((event.node_id, port));
        }
        _ => {}
    }
    Ok(())
}

fn event_can_deliver(flow: &Dataflow<NodeValue>, event: &NodeEvent) -> bool {
    match &event.kind {
        NodeEventKind::ChannelItem { port, .. } => {
            event.node_id.split_once('#').is_none_or(|(process, _)| {
                flow.output_kind(process) == Some(crate::channel::ChannelKind::Value)
                    || flow.can_emit(process, *port, 128)
            })
        }
        _ => true,
    }
}

// Return no cache identity when a value cannot be validated safely.
fn input_fingerprint(
    process: &crate::model::Process,
    inputs: &[NodeInput],
    node: &dyn dag_core::node::DagNode,
    dependency_state: Option<serde_json::Value>,
    has_vfs: bool,
) -> Option<String> {
    fn file(f: &FileRef) -> Option<serde_json::Value> {
        if f.path.starts_with("vfs://")
            || f.fingerprint.as_ref().is_some_and(|fp| fp.immutable_remote)
        {
            return None;
        }
        let path = f.path.strip_prefix("file://").unwrap_or(&f.path);
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
            .to_string();
        Some(serde_json::json!([f.path, metadata.len(), modified]))
    }
    fn channel_item(value: &serde_json::Value, has_vfs: bool) -> Option<serde_json::Value> {
        use serde_json::Value;
        match value {
            Value::Null => Some(serde_json::json!(["null"])),
            Value::Bool(value) => Some(serde_json::json!(["bool", value])),
            Value::Number(value) => Some(serde_json::json!(["number", value])),
            Value::String(value) => {
                // Untyped channel strings may name a virtual file even when
                // an identically named local file also exists.
                if has_vfs && !value.starts_with("file://") {
                    return None;
                }
                if value.contains("://") && !value.starts_with("file://") {
                    return None;
                }
                let path = value.strip_prefix("file://").unwrap_or(value);
                if std::path::Path::new(path).is_file() {
                    let reference = FileRef {
                        path: value.clone(),
                        format: None,
                        fingerprint: None,
                    };
                    Some(serde_json::json!(["file", file(&reference)?]))
                } else {
                    Some(serde_json::json!(["string", value]))
                }
            }
            Value::Array(values) => Some(serde_json::json!([
                "array",
                values
                    .iter()
                    .map(|value| channel_item(value, has_vfs))
                    .collect::<Option<Vec<_>>>()?
            ])),
            Value::Object(values) => {
                let mapped = values
                    .iter()
                    .map(|(key, value)| Some((key.clone(), channel_item(value, has_vfs)?)))
                    .collect::<Option<serde_json::Map<_, _>>>()?;
                Some(serde_json::json!(["object", mapped]))
            }
        }
    }
    let mut values = Vec::new();
    for input in inputs {
        let value = match &input.data {
            NodeValue::File(f) => {
                if has_vfs && f.fingerprint.is_none() && !f.path.starts_with("file://") {
                    return None;
                }
                serde_json::json!(["file", file(f)?])
            }
            NodeValue::FileSet(files) => {
                serde_json::json!([
                    "files",
                    files
                        .iter()
                        .map(|f| {
                            if has_vfs && f.fingerprint.is_none() && !f.path.starts_with("file://")
                            {
                                None
                            } else {
                                file(f)
                            }
                        })
                        .collect::<Option<Vec<_>>>()?
                ])
            }
            NodeValue::Channel(c) => {
                serde_json::json!([
                    "channel",
                    c.items
                        .iter()
                        .map(|value| channel_item(value, has_vfs))
                        .collect::<Option<Vec<_>>>()?
                ])
            }
            NodeValue::DataFrame(_) | NodeValue::Structured(_) => return None,
        };
        values.push(serde_json::json!([input.port, value]));
    }
    let paths = node.referenced_file_paths();
    // A bare path may resolve through the VFS before the local filesystem.
    // Its current metadata cannot safely be inferred from a host stat.
    if has_vfs && paths.iter().any(|path| !path.starts_with("file://")) {
        return None;
    }
    let referenced = paths
        .into_iter()
        .map(|path| {
            file(&FileRef {
                path,
                format: None,
                fingerprint: None,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let bytes = serde_json::to_vec(&serde_json::json!([
        dag_core::engine_version(),
        dag_core::source_revision(),
        process.id,
        process.kind,
        process.spec,
        values,
        referenced,
        dependency_state,
        node.plugin_identity()
    ]))
    .ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

fn cacheable_outputs(outputs: &dag_core::dag::graph::PortOutputs) -> bool {
    !outputs.is_empty()
        && outputs.iter().all(|(_, value)| match value {
            NodeValue::File(f) => {
                f.fingerprint
                    .as_ref()
                    .is_some_and(|fp| !fp.immutable_remote)
                    && !f.path.starts_with("vfs://")
            }
            NodeValue::FileSet(files) => {
                !files.is_empty()
                    && files.iter().all(|f| {
                        f.fingerprint
                            .as_ref()
                            .is_some_and(|fp| !fp.immutable_remote)
                            && !f.path.starts_with("vfs://")
                    })
            }
            _ => false,
        })
}

fn outputs_unchanged(outputs: &dag_core::dag::graph::PortOutputs) -> bool {
    fn unchanged(file: &FileRef) -> bool {
        let Some(expected) = &file.fingerprint else {
            return false;
        };
        let path = file.path.strip_prefix("file://").unwrap_or(&file.path);
        let Some(actual) = dag_core::value::FileFingerprint::from_path(path) else {
            return false;
        };
        actual.size == expected.size && actual.mtime_ns == expected.mtime_ns
    }
    outputs.iter().all(|(_, value)| match value {
        NodeValue::File(f) => unchanged(f),
        NodeValue::FileSet(files) => files.iter().all(unchanged),
        _ => false,
    })
}

impl FlowScheduler {
    /// Run tasks on actual input arrivals instead of a pre-expanded plan.
    pub async fn run_dataflow(
        &self,
        graph: &FlowGraph,
        resolution: &SourceResolution,
        cfg: &FlowRunConfig,
        cancel: Option<CancellationToken>,
    ) -> Result<FlowRunReport, Error> {
        self.run_dataflow_cached(graph, resolution, cfg, None, cancel)
            .await
    }

    pub async fn run_dataflow_cached(
        &self,
        graph: &FlowGraph,
        resolution: &SourceResolution,
        cfg: &FlowRunConfig,
        cache: Option<&mut InstanceCache>,
        cancel: Option<CancellationToken>,
    ) -> Result<FlowRunReport, Error> {
        let streams = SourceStreams::from_resolution(graph, resolution)?;
        self.run_dataflow_streamed_cached(graph, streams, cfg, cache, cancel)
            .await
    }

    pub async fn run_dataflow_streamed_cached(
        &self,
        graph: &FlowGraph,
        mut streams: SourceStreams,
        cfg: &FlowRunConfig,
        mut cache: Option<&mut InstanceCache>,
        cancel: Option<CancellationToken>,
    ) -> Result<FlowRunReport, Error> {
        if cfg.resume && cache.is_none() {
            return Err(Error::Execution("resume requires an instance cache".into()));
        }
        let mut flow = Dataflow::<NodeValue>::new(graph).map_err(Error::Execution)?;
        flow.set_collect_empty(collect_item_is_empty);
        for process in graph.processes() {
            if process.kind == "channel"
                && process.spec.get("operator").and_then(|v| v.as_str()) == Some("take")
                && process.spec.get("count").and_then(|v| v.as_i64()) == Some(0)
            {
                flow.stop_process(&process.id).map_err(Error::Execution)?;
            }
        }
        let executor = self
            .executor
            .bind_run()
            .await
            .map_err(|e| Error::Execution(e.to_string()))?
            .unwrap_or_else(|| Arc::clone(&self.executor));
        let mut sources = Vec::new();
        for source in graph.sources() {
            let reader = streams
                .readers
                .remove(&source.id)
                .ok_or_else(|| Error::Execution(format!("source `{}` is unresolved", source.id)))?;
            sources.push(SourceState {
                source,
                reader,
                pending: None,
                closed: false,
            });
        }
        // Keep cancellation local: a failed run must not cancel the caller's token.
        let caller_token = cancel.unwrap_or_default();
        let token = caller_token.child_token();
        let _cancel_on_drop = token.clone().drop_guard();
        let mut tasks = JoinSet::new();
        let mut function_tasks = JoinSet::new();
        let mut function_pending = VecDeque::<(String, u8, Option<NodeValue>, bool)>::new();
        let mut sequences = BTreeMap::<String, usize>::new();
        let mut combine_state = CombineState::new();
        let mut combine_pending = BTreeMap::<String, PendingCombine>::new();
        let mut join_state = BTreeMap::<String, BTreeMap<String, JoinRows>>::new();
        let mut join_pending = BTreeMap::<String, PendingJoin>::new();
        let mut join_matched = BTreeMap::<String, BTreeSet<String>>::new();
        let mut join_singleton = BTreeMap::<String, [Option<bool>; 2]>::new();
        let mut join_keys = BTreeMap::<String, BTreeMap<String, Vec<serde_json::Value>>>::new();
        let mut group_state = BTreeMap::<String, GroupState>::new();
        let mut group_pending = BTreeMap::<String, PendingGroups>::new();
        let mut list_state =
            BTreeMap::<String, Vec<dag_core::dag::execution::ChannelTransportValue>>::new();
        let mut buffer_state = BTreeMap::<String, BufferState>::new();
        let mut transpose_pending = BTreeMap::<String, PendingTranspose>::new();
        let mut flat_map_pending = BTreeMap::<String, PendingFlatMap>::new();
        let mut if_empty_seen = BTreeSet::<String>::new();
        let mut unique_state = BTreeMap::<String, BTreeSet<String>>::new();
        let mut distinct_state = BTreeMap::<String, String>::new();
        let mut take_state = BTreeMap::<String, u64>::new();
        let mut last_state = BTreeMap::<String, NodeValue>::new();
        let mut reduce_state = BTreeMap::<String, Option<NodeValue>>::new();
        let mut count_state = BTreeMap::<String, u64>::new();
        let mut records = Vec::new();
        // Keep identity outside spawned tasks so cancelled/aborted tasks remain reportable.
        let mut running = BTreeMap::<
            InstanceId,
            (
                String,
                String,
                std::time::Instant,
                Arc<std::sync::atomic::AtomicUsize>,
            ),
        >::new();
        let (event_tx, mut event_rx) = mpsc::channel::<(NodeEvent, oneshot::Sender<()>)>(64);
        let mut pending_events = VecDeque::new();
        let mut streamed = BTreeSet::new();
        let mut failed = false;
        let limit = if cfg.max_concurrency == 0 {
            usize::MAX
        } else {
            cfg.max_concurrency
        };
        let loop_result: Result<(), Error> = async {
        loop {
            if !token.is_cancelled() {
                let names: Vec<_> = join_pending.keys().cloned().collect();
                for name in names {
                    if flow.output_abandoned(&name, 0) {
                        join_pending.remove(&name);
                        flow.finish(&name).map_err(Error::Execution)?;
                        flow.stop_process(&name).map_err(Error::Execution)?;
                        continue;
                    }
                    let done = deliver_pending_join(&mut flow, &name, join_pending.get_mut(&name).expect("pending join"), 128)?;
                    if done {
                        join_pending.remove(&name);
                        flow.finish(&name).map_err(Error::Execution)?;
                    }
                }
                let names: Vec<_> = combine_pending.keys().cloned().collect();
                for name in names {
                    if flow.output_abandoned(&name, 0) {
                        combine_pending.remove(&name);
                        combine_state.remove(&name);
                        flow.finish(&name).map_err(Error::Execution)?;
                        flow.stop_process(&name).map_err(Error::Execution)?;
                        continue;
                    }
                    let done = deliver_pending_combine(&mut flow, &name, combine_pending.get_mut(&name).expect("pending combine"), &combine_state, 128)?;
                    if done {
                        combine_pending.remove(&name);
                        flow.finish(&name).map_err(Error::Execution)?;
                    }
                }
                let names: Vec<_> = group_pending.keys().cloned().collect();
                for name in names {
                    if flow.output_abandoned(&name, 0) {
                        group_pending.remove(&name);
                        flow.finish(&name).map_err(Error::Execution)?;
                        flow.stop_process(&name).map_err(Error::Execution)?;
                        continue;
                    }
                    let done = deliver_pending_groups(&mut flow, &name, group_pending.get_mut(&name).expect("pending groups"), 128)?;
                    if done {
                        group_pending.remove(&name);
                        flow.finish(&name).map_err(Error::Execution)?;
                    }
                }
                let names: Vec<_> = flat_map_pending.keys().cloned().collect();
                for name in names {
                    if flow.output_abandoned(&name, 0) {
                        flat_map_pending.remove(&name);
                        flow.finish(&name).map_err(Error::Execution)?;
                        flow.stop_process(&name).map_err(Error::Execution)?;
                        continue;
                    }
                    while flow.can_emit(&name, 0, 128) {
                        let rows = flat_map_pending.get_mut(&name).expect("pending flat_map");
                        if let Some(output) = rows.next() { flow.emit(&name, 0, output).map_err(Error::Execution)?; }
                        if rows.done() {
                            flat_map_pending.remove(&name);
                            flow.finish(&name).map_err(Error::Execution)?;
                            break;
                        }
                    }
                }
                let names: Vec<_> = transpose_pending.keys().cloned().collect();
                for name in names {
                    if flow.output_abandoned(&name, 0) {
                        transpose_pending.remove(&name);
                        flow.finish(&name).map_err(Error::Execution)?;
                        flow.stop_process(&name).map_err(Error::Execution)?;
                        continue;
                    }
                    while flow.can_emit(&name, 0, 128) {
                        let rows = transpose_pending.get_mut(&name).expect("pending transpose");
                        if let Some(output) = rows.next() {
                            flow.emit(&name, 0, output)
                                .map_err(Error::Execution)?;
                        }
                        if rows.done() {
                            transpose_pending.remove(&name);
                            flow.finish(&name).map_err(Error::Execution)?;
                            break;
                        }
                    }
                }
            }
            while let Some(index) = function_pending.iter().position(|(name, port, value, _)| {
                value.is_none() || flow.output_abandoned(name, *port) || flow.can_emit(name, *port, 128)
            }) {
                let (name, port, value, stop) = function_pending.remove(index).expect("position exists");
                if let Some(value) = value.filter(|_| !flow.output_abandoned(&name, port)) {
                    flow.emit(&name, port, value).map_err(Error::Execution)?;
                }
                flow.finish(&name).map_err(Error::Execution)?;
                if stop { flow.stop_process(&name).map_err(Error::Execution)?; }
            }
            while let Some(index) = pending_events
                .iter()
                .position(|(event, _)| event_can_deliver(&flow, event))
            {
                let (event, ack): (NodeEvent, oneshot::Sender<()>) =
                    pending_events.remove(index).expect("position exists");
                deliver_event(&mut flow, &mut streamed, event)?;
                let _ = ack.send(());
            }
            // A running streaming task occupies its execution slot while it
            // waits for its output acknowledgement. If every slot is occupied
            // by such tasks, no downstream task can start to drain the queues.
            // Admit one pending event in that case: strict concurrency remains
            // intact, at the cost of temporarily exceeding the queue target.
            if !token.is_cancelled()
                && !pending_events.is_empty()
                && tasks.len() >= limit
                && pending_events.len() >= tasks.len()
            {
                let (event, ack): (NodeEvent, oneshot::Sender<()>) =
                    pending_events.pop_front().expect("not empty");
                deliver_event(&mut flow, &mut streamed, event)?;
                let _ = ack.send(());
            }
            if tasks.len() + function_tasks.len() < limit && !token.is_cancelled() {
                for state in &mut sources {
                    if !state.closed && flow.output_abandoned(&state.source.id, 0) {
                        state.reader.close();
                        flow.close_source(&state.source.id)
                            .map_err(Error::Execution)?;
                        state.closed = true;
                        continue;
                    }
                    if state.closed || !flow.can_emit(&state.source.id, 0, 128) {
                        continue;
                    }
                    let message = if let Some(pending) = state.pending.take() {
                        pending
                    } else {
                        match state.reader.try_recv() {
                            Ok(value) => Some(value),
                            Err(mpsc::error::TryRecvError::Empty) => continue,
                            Err(mpsc::error::TryRecvError::Disconnected) => None,
                        }
                    };
                    if let Some(value) = message {
                        let value = value?;
                        let payload = match &state.source.spec {
                            crate::model::SourceSpec::Glob { .. } => value
                                .as_str()
                                .map(file_value)
                                .unwrap_or_else(|| source_value(&value)),
                            crate::model::SourceSpec::FilePairs { flat, .. } => {
                                use dag_core::dag::execution::ChannelTransportValue as T;
                                let items = value.as_array().ok_or_else(|| Error::Execution("file pair source produced an invalid tuple".into()))?;
                                let key = items.first().ok_or_else(|| Error::Execution("file pair source produced an invalid key".into()))?;
                                let key = serde_json::from_value::<T>(key.clone()).unwrap_or_else(|_| T::Json(key.clone()));
                                let paths = if *flat { &items[1..] } else { items.get(1).and_then(|value| value.as_array()).ok_or_else(|| Error::Execution("file pair source produced invalid paths".into()))?.as_slice() };
                                let files = paths.iter().map(|value| value.as_str().map(|path| T::File(FileRef::new(path, None))).ok_or_else(|| Error::Execution("file pair source produced a non-path".into()))).collect::<Result<Vec<_>, _>>()?;
                                let mut tuple = vec![key];
                                if *flat { tuple.extend(files); } else { tuple.push(T::List(files)); }
                                NodeValue::Structured(T::Tuple(tuple))
                            }
                            _ => source_value(&value),
                        };
                        flow.emit(&state.source.id, 0, payload)
                            .map_err(Error::Execution)?;
                    } else {
                        flow.close_source(&state.source.id)
                            .map_err(Error::Execution)?;
                        state.closed = true;
                    }
                }
            }
            while tasks.len() + function_tasks.len() < limit && !token.is_cancelled() {
                let Some(activation) = flow
                    .next_activation_bounded(128)
                    .map_err(Error::Execution)?
                else {
                    break;
                };
                let process = graph
                    .process(&activation.process)
                    .expect("validated process");
                if activation.inputs.iter().any(|(_, input)| matches!(input, Delivery::Collected(values) if values.iter().any(|v| matches!(v, NodeValue::DataFrame(_))))) {
                    return Err(Error::Execution("collecting DataFrame values is not yet supported".into()));
                }
                let structured_operator = match process.spec.get("operator").and_then(|v| v.as_str()) {
                    Some("map" | "filter") => process.spec.get("function").is_some(),
                    Some("flat_map") => process.spec.get("function").is_some() || process.spec.get("template").is_none(),
                    Some("if_empty" | "mix" | "concat" | "first" | "take" | "last" | "count" | "unique" | "distinct" | "flatten" | "buffer" | "transpose" | "branch" | "until" | "reduce") => true,
                    Some("collect" | "to_list" | "to_sorted_list" | "group_tuple" | "join" | "combine") => true,
                    _ => false,
                };
                if process.kind == "channel" && !structured_operator && activation.inputs.iter().any(|(_, input)| match input {
                    Delivery::Value(NodeValue::Structured(_)) => true,
                    Delivery::Collected(values) => values.iter().any(|v| matches!(v, NodeValue::Structured(_))),
                    _ => false,
                }) {
                    return Err(Error::Execution("structured channel operators are not yet supported".into()));
                }
                if process.kind == "channel" && !activation.closing
                    && matches!(process.spec.get("operator").and_then(|v| v.as_str()), Some("first" | "count"))
                    && process.spec.get("criteria").is_some_and(|v| v.is_object())
                {
                    let evaluator = self.channel_function.clone().ok_or_else(|| Error::Execution("no channel function evaluator is installed".into()))?;
                    let (_, delivery) = activation.inputs.into_iter().next().expect("validated predicate input");
                    let value = match delivery { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                    if matches!(value, NodeValue::DataFrame(_)) {
                        return Err(Error::Execution("channel functions do not yet support DataFrame values".into()));
                    }
                    let function = process.spec["criteria"].clone();
                    let name = process.id.clone();
                    let is_first = process.spec["operator"] == "first";
                    function_tasks.spawn_blocking(move || {
                        let result = evaluator(&function, Some(&value)).map(|result| {
                            let matched = node_truthy(&result);
                            if is_first { FunctionResult::First(matched.then_some(value)) }
                            else { FunctionResult::Count(matched) }
                        });
                        (name, result)
                    });
                    continue;
                }
                if process.kind == "channel"
                    && matches!(
                        process.spec.get("operator").and_then(|v| v.as_str()),
                        Some("last" | "count")
                    )
                {
                    let operator = process.spec["operator"].as_str().expect("matched operator");
                    if activation.closing {
                        let value = if operator == "last" {
                            last_state
                                .remove(&process.id)
                                .unwrap_or_else(|| source_value(&serde_json::Value::Null))
                        } else {
                            source_value(&serde_json::json!(
                                count_state.remove(&process.id).unwrap_or(0)
                            ))
                        };
                        flow.emit(&process.id, 0, value).map_err(Error::Execution)?;
                    } else {
                        for (_, delivery) in activation.inputs {
                            let value = match delivery {
                                Delivery::Value(value) => value,
                                Delivery::Collected(values) => collect(values),
                            };
                            if operator == "last" {
                                last_state.insert(process.id.clone(), value);
                            } else if process.spec.get("criteria").is_none_or(|criteria| {
                                criteria.is_null() || matches_literal_criteria(&value, criteria)
                            }) {
                                let count = count_state.entry(process.id.clone()).or_default();
                                *count = count
                                    .checked_add(1)
                                    .ok_or_else(|| Error::Execution("count overflow".into()))?;
                            }
                        }
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel" && process.spec.get("operator").and_then(|v| v.as_str()) == Some("reduce") {
                    let state = reduce_state.entry(process.id.clone()).or_insert_with(|| process.spec.get("seed").filter(|value| !value.is_null()).map(source_value));
                    if activation.closing {
                        let output = state.take().unwrap_or_else(|| source_value(&serde_json::Value::Null));
                        flow.emit(&process.id, 0, output).map_err(Error::Execution)?;
                        reduce_state.remove(&process.id);
                        flow.finish(&process.id).map_err(Error::Execution)?;
                        continue;
                    }
                    let (_, delivery) = activation.inputs.into_iter().next().expect("reduce has one input");
                    let value = match delivery { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                    if let Some(accumulator) = state.take() {
                        let evaluator = self.channel_binary_function.clone().ok_or_else(|| Error::Execution("no binary channel function evaluator is installed".into()))?;
                        let function = process.spec["function"].clone();
                        let name = process.id.clone();
                        function_tasks.spawn_blocking(move || (name, evaluator(&function, &accumulator, &value).map(FunctionResult::Reduce)));
                    } else {
                        *state = (!node_is_null(&value)).then_some(value);
                        flow.finish(&process.id).map_err(Error::Execution)?;
                    }
                    continue;
                }
                if process.kind == "channel"
                    && matches!(
                        process.spec.get("operator").and_then(|v| v.as_str()),
                        Some("first" | "take")
                    )
                {
                    let operator = process.spec["operator"].as_str().expect("matched operator");
                    let (_, delivery) = activation.inputs.into_iter().next().expect("first/take has one input");
                    let value = match delivery { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                    if operator == "first" {
                        if let Some(criteria) = process.spec.get("criteria") {
                            let matched = matches_literal_criteria(&value, criteria);
                            if !matched { flow.finish(&process.id).map_err(Error::Execution)?; continue; }
                        }
                    }
                    flow.emit(&process.id, 0, value).map_err(Error::Execution)?;
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    let seen = take_state.entry(process.id.clone()).or_default();
                    *seen += 1;
                    let limit = if operator == "first" {
                        1
                    } else {
                        process.spec["count"]
                            .as_i64()
                            .expect("validated take count")
                    };
                    if limit > 0 && *seen >= limit as u64 {
                        flow.stop_process(&process.id).map_err(Error::Execution)?;
                    }
                    continue;
                }
                if process.kind == "channel" && process.spec.get("operator").and_then(|v| v.as_str()) == Some("until") {
                    let evaluator = self.channel_function.clone().ok_or_else(|| Error::Execution("no channel function evaluator is installed".into()))?;
                    let (_, delivery) = activation.inputs.into_iter().next().expect("until has one input");
                    let value = match delivery { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                    if matches!(value, NodeValue::DataFrame(_)) { return Err(Error::Execution("channel functions do not yet support DataFrame values".into())); }
                    let function = process.spec["function"].clone();
                    let name = process.id.clone();
                    function_tasks.spawn_blocking(move || {
                        let result = evaluator(&function, Some(&value)).map(|result| {
                            let stop = node_truthy(&result);
                            FunctionResult::Until { value: (!stop).then_some(value), stop }
                        });
                        (name, result)
                    });
                    continue;
                }
                if process.kind == "channel"
                    && matches!(
                        process.spec.get("operator").and_then(|v| v.as_str()),
                        Some("unique" | "distinct")
                    )
                {
                    let operator = process.spec["operator"].as_str().expect("matched operator");
                    if let Some(function) = process.spec.get("function") {
                        let evaluator = self.channel_function.clone().ok_or_else(|| Error::Execution("no channel function evaluator is installed".into()))?;
                        let (_, delivery) = activation.inputs.into_iter().next().expect("dedup has one input");
                        let value = match delivery { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                        if matches!(value, NodeValue::DataFrame(_)) { return Err(Error::Execution("channel functions do not yet support DataFrame values".into())); }
                        let function = function.clone();
                        let name = process.id.clone();
                        let distinct = operator == "distinct";
                        function_tasks.spawn_blocking(move || {
                            let result = evaluator(&function, Some(&value)).map(|key| FunctionResult::Dedup { key, value, distinct });
                            (name, result)
                        });
                        continue;
                    }
                    for (_, delivery) in activation.inputs {
                        let value = match delivery {
                            Delivery::Value(value) => value,
                            Delivery::Collected(values) => collect(values),
                        };
                        if matches!(value, NodeValue::DataFrame(_)) {
                            return Err(Error::Execution(format!(
                                "{operator} does not support DataFrame values"
                            )));
                        }
                        let is_null = node_is_null(&value);
                        let key = node_comparison_key(&value)?;
                        let emit = if operator == "unique" {
                            unique_state
                                .entry(process.id.clone())
                                .or_default()
                                .insert(key)
                        } else {
                            let previous = distinct_state.insert(process.id.clone(), key.clone());
                            // Nextflow initializes its previous comparison value to null.
                            previous.map_or(!is_null, |previous| previous != key)
                        };
                        if emit {
                            flow.emit(&process.id, 0, value).map_err(Error::Execution)?;
                        }
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel" && process.spec.get("operator").and_then(|v| v.as_str()) == Some("flat_map") && process.spec.get("function").is_none() {
                    let (_, delivery) = activation.inputs.into_iter().next().expect("flat_map has one input");
                    let value = match delivery { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                    if matches!(value, NodeValue::DataFrame(_)) { return Err(Error::Execution("flat_map does not support DataFrame values".into())); }
                    if process.spec.get("template").is_none() {
                        if let NodeValue::Structured(value) = value {
                            use dag_core::dag::execution::ChannelTransportValue as T;
                            let items = match value {
                                T::List(items) | T::Tuple(items) => items,
                                T::Map(items) => items.into_iter().map(|(key, value)| T::Tuple(vec![T::Json(serde_json::Value::String(key)), value])).collect(),
                                T::Json(serde_json::Value::Array(items)) => items.into_iter().map(T::Json).collect(),
                                T::Json(serde_json::Value::Object(items)) => items.into_iter().map(|(key, value)| T::Tuple(vec![T::Json(serde_json::Value::String(key)), T::Json(value)])).collect(),
                                other => vec![other],
                            };
                            if items.is_empty() { flow.finish(&process.id).map_err(Error::Execution)?; }
                            else { flat_map_pending.insert(process.id.clone(), PendingFlatMap::Structured(items.into_iter())); }
                            continue;
                        }
                        if let NodeValue::FileSet(files) = value {
                            if files.is_empty() {
                                flow.finish(&process.id).map_err(Error::Execution)?;
                            } else {
                                flat_map_pending.insert(process.id.clone(), PendingFlatMap::Files(files.into_iter()));
                            }
                            continue;
                        }
                    }
                    let item = json_value(&value);
                    if process.spec.get("template").is_none() && !item.is_array() && !item.is_object() {
                        flow.emit(&process.id, 0, value).map_err(Error::Execution)?;
                        flow.finish(&process.id).map_err(Error::Execution)?;
                        continue;
                    }
                    let mapped = match process.spec.get("template") { Some(template) => render_spec(template, &item)?, None => item };
                    let rows = FlatMapRows::new(mapped);
                    if rows.done() { flow.finish(&process.id).map_err(Error::Execution)?; continue; }
                    flat_map_pending.insert(process.id.clone(), PendingFlatMap::Json(rows));
                    break;
                }
                if process.kind == "channel" && process.spec.get("operator").and_then(|v| v.as_str()) == Some("flatten")
                    && activation.inputs.iter().any(|(_, input)| match input {
                        Delivery::Value(value) => matches!(value, NodeValue::File(_) | NodeValue::FileSet(_) | NodeValue::Structured(_)),
                        Delivery::Collected(values) => values.iter().any(|value| matches!(value, NodeValue::File(_) | NodeValue::FileSet(_) | NodeValue::Structured(_))),
                    })
                {
                    let (_, delivery) = activation.inputs.into_iter().next().expect("flatten has one input");
                    let value = match delivery { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                    let value = node_transport_value(value)?;
                    flat_map_pending.insert(process.id.clone(), PendingFlatMap::Flatten(vec![value]));
                    continue;
                }
                if process.kind == "channel"
                    && process.spec.get("operator").and_then(|v| v.as_str()) == Some("transpose")
                {
                    let (_, delivery) = activation
                        .inputs
                        .into_iter()
                        .next()
                        .expect("transpose has one input");
                    let value = match delivery {
                        Delivery::Value(value) => value,
                        Delivery::Collected(values) => collect(values),
                    };
                    if matches!(value, NodeValue::DataFrame(_)) {
                        return Err(Error::Execution(
                            "transpose does not support DataFrame values".into(),
                        ));
                    }
                    if matches!(&value, NodeValue::Structured(dag_core::dag::execution::ChannelTransportValue::Tuple(_) | dag_core::dag::execution::ChannelTransportValue::List(_) | dag_core::dag::execution::ChannelTransportValue::Json(serde_json::Value::Array(_))) | NodeValue::FileSet(_)) {
                        let tuple = channel_fields(value);
                        let rows = TypedTransposeRows::new(tuple, process.spec.get("by"), process.spec.get("remainder").and_then(|v| v.as_bool()).unwrap_or(false))?;
                        if rows.done() { flow.finish(&process.id).map_err(Error::Execution)?; continue; }
                        transpose_pending.insert(process.id.clone(), PendingTranspose::Typed(rows));
                        break;
                    }
                    if matches!(&value, NodeValue::Structured(_)) {
                        flow.emit(&process.id, 0, value).map_err(Error::Execution)?;
                        flow.finish(&process.id).map_err(Error::Execution)?;
                        continue;
                    }
                    let item = json_value(&value);
                    if !item.is_array() {
                        flow.emit(&process.id, 0, value).map_err(Error::Execution)?;
                        flow.finish(&process.id).map_err(Error::Execution)?;
                        continue;
                    }
                    let rows = TransposeRows::new(
                        item,
                        process.spec.get("by"),
                        process
                            .spec
                            .get("remainder")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    )?;
                    if rows.done() {
                        flow.finish(&process.id).map_err(Error::Execution)?;
                        continue;
                    }
                    transpose_pending.insert(process.id.clone(), PendingTranspose::Json(rows));
                    break;
                }
                if process.kind == "channel"
                    && process.spec.get("operator").and_then(|v| v.as_str()) == Some("if_empty")
                {
                    if activation.closing {
                        if !if_empty_seen.contains(&process.id) {
                            if process.spec["value"].get("__dag_shell_function__").is_some() {
                                let evaluator = self.channel_function.clone().ok_or_else(|| Error::Execution("no channel function evaluator is installed".into()))?;
                                let function = process.spec["value"].clone();
                                let name = process.id.clone();
                                function_tasks.spawn_blocking(move || (name, evaluator(&function, None).map(|value| FunctionResult::Item(Some(value)))));
                                continue;
                            }
                            flow.emit(&process.id, 0, source_value(&process.spec["value"]))
                                .map_err(Error::Execution)?;
                        }
                    } else {
                        let (_, delivery) = activation
                            .inputs
                            .into_iter()
                            .next()
                            .expect("if_empty has one input");
                        let value = match delivery {
                            Delivery::Value(value) => value,
                            Delivery::Collected(values) => collect(values),
                        };
                        if_empty_seen.insert(process.id.clone());
                        flow.emit(&process.id, 0, value).map_err(Error::Execution)?;
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel"
                    && process.spec.get("operator").and_then(|v| v.as_str()) == Some("buffer")
                {
                    let state = buffer_state.entry(process.id.clone()).or_default();
                    let batch = if activation.closing {
                        state.finish(
                            process
                                .spec
                                .get("remainder")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
                        )
                    } else {
                        let (_, delivery) = activation
                            .inputs
                            .into_iter()
                            .next()
                            .expect("buffer has one input");
                        let value = match delivery {
                            Delivery::Value(value) => value,
                            Delivery::Collected(values) => collect(values),
                        };
                        if matches!(value, NodeValue::DataFrame(_)) {
                            return Err(Error::Execution(
                                "buffer does not support DataFrame values".into(),
                            ));
                        }
                        state.push(
                            serde_json::to_value(node_transport_value(value)?).map_err(|e| Error::Execution(e.to_string()))?,
                            process.spec["size"].as_u64().unwrap() as usize,
                            process
                                .spec
                                .get("skip")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize,
                        )
                    };
                    if let Some(batch) = batch {
                        flow.emit(
                            &process.id,
                            0,
                            collection_value(batch.into_iter().map(serde_json::from_value).collect::<Result<Vec<_>, _>>().map_err(|e| Error::Execution(e.to_string()))?),
                        )
                        .map_err(Error::Execution)?;
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel"
                    && matches!(
                        process.spec.get("operator").and_then(|v| v.as_str()),
                        Some("collect" | "to_list" | "to_sorted_list")
                    )
                {
                    let is_collect =
                        process.spec.get("operator").and_then(|v| v.as_str()) == Some("collect");
                    let flatten = is_collect
                        && process
                            .spec
                            .get("flat")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                    if is_collect && !activation.closing && process.spec.get("function").is_some() {
                        let evaluator = self.channel_function.clone().ok_or_else(|| Error::Execution("no channel function evaluator is installed".into()))?;
                        let (_, delivery) = activation.inputs.into_iter().next().expect("collect has one input");
                        let value = match delivery { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                        if matches!(value, NodeValue::DataFrame(_)) {
                            return Err(Error::Execution("collect functions do not yet support DataFrame values".into()));
                        }
                        let function = process.spec["function"].clone();
                        let name = process.id.clone();
                        function_tasks.spawn_blocking(move || {
                            (name, evaluator(&function, Some(&value)).map(|value| FunctionResult::Collect { value, flatten }))
                        });
                        continue;
                    }
                    let items = list_state.entry(process.id.clone()).or_default();
                    if activation.closing {
                        if !is_collect || !items.is_empty() {
                            if is_collect && process.spec.get("sort").is_some_and(|v| v.is_object()) {
                                let evaluator = self.channel_function.clone().ok_or_else(|| Error::Execution("no channel function evaluator is installed".into()))?;
                                let function = process.spec["sort"].clone();
                                let values = std::mem::take(items);
                                let name = process.id.clone();
                                function_tasks.spawn_blocking(move || {
                                    (name, sort_channel_values_by_function(values, &function, &evaluator).map(|value| FunctionResult::Item(Some(value))))
                                });
                                continue;
                            }
                            let sorted = process.spec.get("operator").and_then(|v| v.as_str())
                                == Some("to_sorted_list")
                                || (is_collect
                                    && process.spec.get("sort").and_then(|v| v.as_bool())
                                        == Some(true));
                            if sorted {
                                sort_channel_transport_values(items)?;
                            }
                            flow.emit(
                                &process.id,
                                0,
                                collection_value(std::mem::take(items)),
                            )
                            .map_err(Error::Execution)?;
                        }
                    } else {
                        for (_, delivery) in activation.inputs {
                            let value = match delivery {
                                Delivery::Value(value) => value,
                                Delivery::Collected(values) => collect(values),
                            };
                            if matches!(value, NodeValue::DataFrame(_)) {
                                return Err(Error::Execution(
                                    "collect/to_list does not support DataFrame values".into(),
                                ));
                            }
                            append_collection(items, &value, flatten);
                        }
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel"
                    && process.spec.get("operator").and_then(|v| v.as_str()) == Some("group_tuple")
                {
                    if process.spec.as_object().is_some_and(|fields| {
                        fields.keys().any(|key| {
                            !["operator", "by", "size", "remainder", "sort"].contains(&key.as_str())
                        })
                    }) {
                        return Err(Error::Execution(
                            "group_tuple supports by, size, remainder and natural sort options".into(),
                        ));
                    }
                    let pivot: Vec<usize> = match process.spec.get("by") {
                        None | Some(serde_json::Value::Null) => vec![0],
                        Some(serde_json::Value::Number(n)) => vec![n.as_u64().ok_or_else(|| {
                            Error::Execution("group_tuple `by` must be nonnegative".into())
                        })?
                            as usize],
                        Some(serde_json::Value::Array(values)) => values
                            .iter()
                            .map(|v| {
                                v.as_u64().map(|i| i as usize).ok_or_else(|| {
                                    Error::Execution("invalid group_tuple `by`".into())
                                })
                            })
                            .collect::<Result<_, _>>()?,
                        _ => return Err(Error::Execution("invalid group_tuple `by`".into())),
                    };
                    let size = match process.spec.get("size") {
                        None | Some(serde_json::Value::Null) => None,
                        Some(v) => Some(v.as_u64().filter(|n| *n > 0).ok_or_else(|| {
                            Error::Execution("group_tuple size must be positive".into())
                        })? as usize),
                    };
                    let remainder = match process.spec.get("remainder") {
                        None => false,
                        Some(v) => v.as_bool().ok_or_else(|| {
                            Error::Execution("group_tuple remainder must be boolean".into())
                        })?,
                    };
                    let sort = process.spec.get("sort").is_some_and(|v| !v.is_null());
                    if activation.closing {
                        let groups = group_state.remove(&process.id).unwrap_or_default();
                        if groups.order.is_empty() {
                            flow.finish(&process.id).map_err(Error::Execution)?;
                            continue;
                        }
                        group_pending.insert(process.id.clone(), PendingGroups { groups, pivot, size, remainder, sort });
                        break;
                    } else {
                        let groups = group_state.entry(process.id.clone()).or_default();
                        groups.typed = true;
                        for (_, delivery) in activation.inputs {
                            let value = match delivery {
                                Delivery::Value(v) => v,
                                Delivery::Collected(v) => collect(v),
                            };
                            if matches!(value, NodeValue::DataFrame(_)) {
                                return Err(Error::Execution(
                                    "group_tuple does not support DataFrame values".into(),
                                ));
                            }
                            use dag_core::dag::execution::ChannelTransportValue as T;
                            let tuple = match value {
                                NodeValue::Structured(T::Tuple(items) | T::List(items)) => items,
                                NodeValue::Structured(T::Json(serde_json::Value::Array(items))) => items.into_iter().map(T::Json).collect(),
                                other => match json_value(&other) {
                                    serde_json::Value::Array(items) => items.into_iter().map(T::Json).collect(),
                                    _ => return Err(Error::Execution("group_tuple requires tuple values".into())),
                                },
                            };
                            if pivot.iter().any(|i| *i >= tuple.len()) {
                                return Err(Error::Execution(
                                    "group_tuple key index out of range".into(),
                                ));
                            }
                            let key = serde_json::to_string(&pivot.iter().map(|i| group_key(&tuple[*i])).collect::<Vec<_>>())
                            .map_err(|e| Error::Execution(e.to_string()))?;
                            let encoded = tuple.into_iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>().map_err(|e| Error::Execution(e.to_string()))?;
                            if let Some(output) = groups.add(key, encoded, &pivot, size)? {
                                flow.emit(&process.id, 0, typed_group_output(output, &pivot, sort)?)
                                    .map_err(Error::Execution)?;
                            }
                        }
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel"
                    && process.spec.get("operator").and_then(|v| v.as_str()) == Some("join")
                {
                    if process.spec.as_object().is_some_and(|fields| {
                        fields.keys().any(|key| {
                            ![
                                "operator",
                                "by",
                                "remainder",
                                "failOnMismatch",
                                "failOnDuplicate",
                            ]
                            .contains(&key.as_str())
                        })
                    }) {
                        return Err(Error::Execution("unsupported join option".into()));
                    }
                    let pivot: Vec<usize> = match process.spec.get("by") {
                        None | Some(serde_json::Value::Null) => vec![0],
                        Some(serde_json::Value::Number(n)) => vec![n.as_u64().ok_or_else(|| {
                            Error::Execution("join `by` must be a nonnegative index".into())
                        })?
                            as usize],
                        Some(serde_json::Value::Array(values)) => values
                            .iter()
                            .map(|v| {
                                v.as_u64().map(|i| i as usize).ok_or_else(|| {
                                    Error::Execution(
                                        "join `by` must contain nonnegative indices".into(),
                                    )
                                })
                            })
                            .collect::<Result<_, _>>()?,
                        _ => {
                            return Err(Error::Execution(
                                "join `by` must be an index or list of indices".into(),
                            ));
                        }
                    };
                    let option = |name: &str| -> Result<bool, Error> {
                        match process.spec.get(name) {
                            None => Ok(false),
                            Some(value) => value.as_bool().ok_or_else(|| {
                                Error::Execution(format!("join `{name}` must be boolean"))
                            }),
                        }
                    };
                    let remainder = option("remainder")?;
                    let fail_mismatch = option("failOnMismatch")?;
                    let fail_duplicate = option("failOnDuplicate")?;
                    if remainder && fail_mismatch {
                        return Err(Error::Execution(
                            "join remainder conflicts with failOnMismatch".into(),
                        ));
                    }
                    if activation.closing {
                        let matched = join_matched.remove(&process.id).unwrap_or_default();
                        let singleton = join_singleton.remove(&process.id).unwrap_or_default();
                        let mut groups = join_state.remove(&process.id).unwrap_or_default();
                        let typed_keys = join_keys.remove(&process.id).unwrap_or_default();
                        groups.retain(|_, (left, right)| !left.is_empty() || !right.is_empty());
                        for key in groups.keys() {
                            if fail_mismatch { return Err(Error::Execution(format!("join mismatch for key {key}"))); }
                            if fail_duplicate && matched.contains(key) { return Err(Error::Execution(format!("join duplicate for key {key}"))); }
                        }
                        if remainder && !groups.is_empty() {
                            join_pending.insert(process.id.clone(), PendingJoin { groups, singleton, typed_keys: Some(typed_keys) });
                            break;
                        }
                        flow.finish(&process.id).map_err(Error::Execution)?;
                        continue;
                    }
                    for (port, delivery) in activation.inputs {
                        let value = match delivery {
                            Delivery::Value(v) => v,
                            Delivery::Collected(v) => collect(v),
                        };
                        if matches!(value, NodeValue::DataFrame(_)) {
                            return Err(Error::Execution(
                                "join does not support DataFrame values".into(),
                            ));
                        }
                        let fields = channel_fields(value);
                        if pivot.iter().any(|index| *index >= fields.len()) {
                            return Err(Error::Execution("join key index out of range".into()));
                        }
                        let mut keys = Vec::new();
                        let mut rest = Vec::new();
                        for (i, value) in fields.into_iter().enumerate() {
                            if pivot.contains(&i) {
                                keys.push(value);
                            } else {
                                rest.push(value);
                            }
                        }
                        let key = serde_json::to_string(&keys.iter().map(group_key).collect::<Vec<_>>())
                            .map_err(|e| Error::Execution(e.to_string()))?;
                        let mut keys = keys.into_iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>().map_err(|e| Error::Execution(e.to_string()))?;
                        let rest = rest.into_iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>().map_err(|e| Error::Execution(e.to_string()))?;
                        join_keys.entry(process.id.clone()).or_default().entry(key.clone()).or_insert_with(|| keys.clone());
                        if fail_duplicate
                            && join_matched
                                .entry(process.id.clone())
                                .or_default()
                                .contains(&key)
                        {
                            return Err(Error::Execution(format!("join duplicate for key {key}")));
                        }
                        if port > 1 {
                            return Err(Error::Execution(format!("invalid join port {port}")));
                        }
                        let singleton = join_singleton.entry(process.id.clone()).or_default();
                        singleton[port as usize] =
                            Some(singleton[port as usize].unwrap_or(false) || rest.is_empty());
                        let state = join_state
                            .entry(process.id.clone())
                            .or_default()
                            .entry(key.clone())
                            .or_default();
                        match port {
                            0 => state.0.push_back(rest),
                            1 => state.1.push_back(rest),
                            _ => return Err(Error::Execution(format!("invalid join port {port}"))),
                        }
                        if !state.0.is_empty() && !state.1.is_empty() {
                            if fail_duplicate
                                && !join_matched
                                    .entry(process.id.clone())
                                    .or_default()
                                    .insert(key.clone())
                            {
                                return Err(Error::Execution(format!(
                                    "join duplicate for key {key}"
                                )));
                            }
                            keys.extend(state.0.pop_front().expect("nonempty"));
                            keys.extend(state.1.pop_front().expect("nonempty"));
                            flow.emit(&process.id, 0, typed_flat_output(keys)?)
                                .map_err(Error::Execution)?;
                            if state.0.is_empty() && state.1.is_empty() {
                                join_keys.get_mut(&process.id).expect("join keys exist").remove(&key);
                            }
                        }
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel"
                    && process.spec.get("operator").and_then(|v| v.as_str()) == Some("combine")
                {
                    if process.spec.as_object().is_some_and(|fields| {
                        fields.keys().any(|key| key != "operator" && key != "by")
                    }) {
                        return Err(Error::Execution("unsupported combine option".into()));
                    }
                    let pivot: Vec<usize> = match process.spec.get("by") {
                        None | Some(serde_json::Value::Null) => vec![],
                        Some(serde_json::Value::Number(n)) => vec![n.as_u64().ok_or_else(|| {
                            Error::Execution("combine `by` must be nonnegative".into())
                        })?
                            as usize],
                        Some(serde_json::Value::Array(values)) => values
                            .iter()
                            .map(|v| {
                                v.as_u64()
                                    .map(|i| i as usize)
                                    .ok_or_else(|| Error::Execution("invalid combine `by`".into()))
                            })
                            .collect::<Result<_, _>>()?,
                        _ => return Err(Error::Execution("invalid combine `by`".into())),
                    };
                    for (port, delivery) in activation.inputs {
                        let value = match delivery {
                            Delivery::Value(v) => v,
                            Delivery::Collected(v) => collect(v),
                        };
                        if matches!(value, NodeValue::DataFrame(_)) {
                            return Err(Error::Execution(
                                "combine does not support DataFrame values".into(),
                            ));
                        }
                        let fields = channel_fields(value);
                        if pivot.iter().any(|index| *index >= fields.len()) {
                            return Err(Error::Execution("combine key index out of range".into()));
                        }
                        let mut keys = Vec::new();
                        let mut rest = Vec::new();
                        for (index, value) in fields.into_iter().enumerate() {
                            if pivot.contains(&index) {
                                keys.push(value);
                            } else {
                                rest.push(value);
                            }
                        }
                        let key = serde_json::to_string(&keys.iter().map(group_key).collect::<Vec<_>>())
                            .map_err(|e| Error::Execution(e.to_string()))?;
                        let keys = keys.into_iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>().map_err(|e| Error::Execution(e.to_string()))?;
                        let rest = rest.into_iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>().map_err(|e| Error::Execution(e.to_string()))?;
                        let state = combine_state.entry(process.id.clone()).or_default().entry(key.clone()).or_default();
                        let (own_index, count) = match port {
                            0 => { let index = state.0.len(); state.0.push(rest); (index, state.1.len()) },
                            1 => { let index = state.1.len(); state.1.push(rest); (index, state.0.len()) },
                            _ => return Err(Error::Execution(format!("combine does not support input port {port}"))),
                        };
                        if count > 0 {
                            combine_pending.insert(process.id.clone(), PendingCombine { key, keys, side: port, own_index, next: 0, count, typed: true });
                            break;
                        }
                    }
                    if combine_pending.contains_key(&process.id) { break; }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel"
                    && matches!(
                        process.spec.get("operator").and_then(|v| v.as_str()),
                        Some("mix" | "concat")
                    )
                {
                    for (_, value) in activation.inputs {
                        match value {
                            Delivery::Value(value) => {
                                flow.emit(&process.id, 0, value).map_err(Error::Execution)?
                            }
                            Delivery::Collected(values) => flow
                                .emit(&process.id, 0, collect(values))
                                .map_err(Error::Execution)?,
                        }
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel" && process.spec.get("operator").and_then(|v| v.as_str()) == Some("branch")
                    && process.spec.get("branches").and_then(|v| v.as_array()).is_some_and(|branches| branches.iter().any(|branch| branch.get("function").is_some()))
                {
                    let evaluator = self.channel_function.clone().ok_or_else(|| Error::Execution("no channel function evaluator is installed".into()))?;
                    let (_, input) = activation.inputs.into_iter().next().expect("branch has one input");
                    let value = match input { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                    if matches!(value, NodeValue::DataFrame(_)) { return Err(Error::Execution("channel functions do not yet support DataFrame values".into())); }
                    let branches = process.spec["branches"].as_array().expect("validated branches").clone();
                    let name = process.id.clone();
                    function_tasks.spawn_blocking(move || {
                        let result = (|| {
                            for (index, branch) in branches.iter().enumerate() {
                                let condition = evaluator(&branch["function"], Some(&value))?;
                                if node_truthy(&condition) {
                                    let output = match branch.get("transform") {
                                        Some(transform) => evaluator(transform, Some(&value))?,
                                        None => value,
                                    };
                                    return Ok(FunctionResult::Branch(Some((index as u8, output))));
                                }
                            }
                            Ok(FunctionResult::Branch(None))
                        })();
                        (name, result)
                    });
                    continue;
                }
                if process.kind == "channel" && process.spec.get("operator").and_then(|v| v.as_str()) == Some("branch")
                    && activation.inputs.iter().any(|(_, input)| match input {
                        Delivery::Value(value) => matches!(value, NodeValue::File(_) | NodeValue::FileSet(_) | NodeValue::Structured(_)),
                        Delivery::Collected(values) => values.iter().any(|value| matches!(value, NodeValue::File(_) | NodeValue::FileSet(_) | NodeValue::Structured(_))),
                    })
                {
                    let (_, input) = activation.inputs.into_iter().next().expect("branch has one input");
                    let value = match input { Delivery::Value(value) => value, Delivery::Collected(values) => collect(values) };
                    let projection = predicate_projection(&node_transport_value(value.clone())?)?;
                    let operator: dag_core::dag::ChannelOperator = serde_json::from_value(process.spec.clone()).map_err(|e| Error::Execution(e.to_string()))?;
                    let dag_core::dag::ChannelOperator::Branch { branches } = operator else { unreachable!() };
                    if let Some((index, _)) = branches.iter().enumerate().find(|(_, branch)| dag_core::dag::channel::branch_matches(branch, &projection)) {
                        flow.emit(&process.id, index as u8, value).map_err(Error::Execution)?;
                    }
                    flow.finish(&process.id).map_err(Error::Execution)?;
                    continue;
                }
                if process.kind == "channel"
                    && matches!(process.spec.get("operator").and_then(|v| v.as_str()), Some("map" | "filter" | "flat_map"))
                    && process.spec.get("function").is_some()
                {
                    let evaluator = self.channel_function.clone().ok_or_else(|| {
                        Error::Execution("no channel function evaluator is installed".into())
                    })?;
                    let (_, delivery) = activation.inputs.into_iter().next().expect("validated channel function input");
                    let value = match delivery {
                        Delivery::Value(value) => value,
                        Delivery::Collected(values) => collect(values),
                    };
                    if !matches!(value, NodeValue::Channel(_) | NodeValue::File(_) | NodeValue::FileSet(_) | NodeValue::Structured(_)) {
                        return Err(Error::Execution("channel functions do not yet support DataFrame values".into()));
                    }
                    let function = process.spec["function"].clone();
                    let name = process.id.clone();
                    let is_filter = process.spec["operator"] == "filter";
                    let is_flat_map = process.spec["operator"] == "flat_map";
                    function_tasks.spawn_blocking(move || {
                        let result = evaluator(&function, Some(&value)).map(|result| {
                            let json = matches!(result, NodeValue::Channel(_)).then(|| json_value(&result));
                            if is_filter {
                                let truth = node_truthy(&result);
                                FunctionResult::Item(truth.then_some(value))
                            } else if let Some(json) = json {
                                if is_flat_map { FunctionResult::FlatMap(json) }
                                else if json.is_null() { FunctionResult::Item(None) }
                                else { FunctionResult::Item(Some(result)) }
                            } else if is_flat_map && matches!(result, NodeValue::FileSet(_)) {
                                let NodeValue::FileSet(files) = result else { unreachable!() };
                                FunctionResult::FlatFiles(files)
                            } else if is_flat_map && matches!(result, NodeValue::Structured(_)) {
                                let NodeValue::Structured(value) = result else { unreachable!() };
                                FunctionResult::FlatStructured(value)
                            } else {
                                FunctionResult::Item(Some(result))
                            }
                        });
                        (name, result)
                    });
                    continue;
                }
                let seq = sequences.entry(process.id.clone()).or_default();
                *seq += 1;
                let key = seq.to_string();
                let id = InstanceId::new(&process.id, Some(&key));
                let mut inputs: Vec<NodeInput> = activation
                    .inputs
                    .into_iter()
                    .map(|(port, v)| NodeInput {
                        port,
                        data: match v {
                            Delivery::Value(v) => v,
                            Delivery::Collected(v) => collect(v),
                        },
                    })
                    .collect();
                let item = inputs.first().map(|i| json_value(&i.data));
                let spec = if let Some(item) = &item {
                    render_spec(&process.spec, item)?
                } else {
                    process.spec.clone()
                };
                let node = (self.node_builder)(&process.kind, spec.clone())?;
                coerce_file_inputs(&mut inputs, node.as_ref());
                node.prepare_task_inputs(&mut inputs)
                    .map_err(|e| Error::Execution(format!("input binding for `{}`: {e}", process.id)))?;
                let fingerprint = if cache.is_some() && node.allows_cache_reuse() {
                    let dependency = node
                        .cache_dependency_state(&self.engine_ctx, &inputs)
                        .await
                        .map_err(|e| {
                            Error::Execution(format!("cache dependency for `{}`: {e}", process.id))
                        })?;
                    input_fingerprint(
                        process,
                        &inputs,
                        node.as_ref(),
                        dependency,
                        self.engine_ctx.opendal.is_some(),
                    )
                } else {
                    None
                };
                if cfg.resume {
                    if let (Some(key), Some(cache)) = (fingerprint.as_ref(), cache.as_deref_mut()) {
                        if let Some(cached) = cache.lookup(&process.id, Some(key)) {
                            if cached.still_valid(self.engine_ctx.opendal.as_deref()).await {
                                if let Some(outputs) =
                                    cached.to_port_outputs().filter(outputs_unchanged)
                                {
                                    for (port, value) in outputs.iter() {
                                        flow.emit(&process.id, *port, value.clone())
                                            .map_err(Error::Execution)?;
                                    }
                                    flow.finish(&process.id).map_err(Error::Execution)?;
                                    records.push(InstanceRecord {
                                        id,
                                        process: process.id.clone(),
                                        item_key: Some(key.clone()),
                                        status: RuntimeStatus::Success,
                                        reused: true,
                                        ignored: false,
                                        attempts: 0,
                                        outputs: outputs
                                            .iter()
                                            .map(|(p, v)| (*p, v.clone()))
                                            .collect(),
                                        error: None,
                                        elapsed_ms: 0,
                                    });
                                    continue;
                                }
                            }
                        }
                    }
                }
                executor
                    .preflight(&[(id.as_str(), node.as_ref())])
                    .map_err(|e| Error::Execution(e.to_string()))?;
                let bindings = inputs
                    .iter()
                    .map(|i| TaskInputBinding {
                        name: node
                            .ports()
                            .input_port(i.port)
                            .and_then(|port| port.label.clone())
                            .unwrap_or_else(|| format!("port_{}", i.port)),
                        port: i.port,
                        source: {
                            let wire = graph
                                .incoming(&process.id)
                                .find(|wire| wire.to_port == i.port)
                                .expect("validated input has an authored channel");
                            TaskInputSource::Channel {
                                from: wire.from.clone(),
                                from_port: wire.from_port,
                                collected: wire.op == crate::model::WireOp::Collect,
                            }
                        },
                        payload: i.data.data_type().to_string(),
                        path: match &i.data {
                            NodeValue::File(f) => Some(f.path.clone()),
                            NodeValue::FileSet(f) => f.first().map(|x| x.path.clone()),
                            _ => None,
                        },
                        fingerprint: match &i.data {
                            NodeValue::File(f) => f.fingerprint.clone(),
                            _ => None,
                        },
                        staged_paths: vec![],
                    })
                    .collect();
                let output_bindings = node
                    .ports()
                    .output_ports()
                    .iter()
                    .map(|p| TaskOutputBinding {
                        name: p
                            .label
                            .clone()
                            .unwrap_or_else(|| format!("port_{}", p.index)),
                        port: p.index,
                        payload: p.data_type.to_string(),
                        artifact_paths: vec![],
                    })
                    .collect();
                let submission = TaskSubmission {
                    spec: TaskSpec {
                        id: id.as_str().into(),
                        logical_node: Some(process.id.clone()),
                        kind: process.kind.clone(),
                        spec,
                        axis: None,
                        item_key: Some(key.clone()),
                        item,
                        inputs: bindings,
                        outputs: output_bindings,
                    },
                    inputs,
                    resources: node.task_resources().unwrap_or_default(),
                };
                let ctx = Arc::clone(&self.engine_ctx);
                let executor = Arc::clone(&executor);
                let task_cancel = token.child_token();
                let strategy = process.error_strategy.clone();
                let process_name = process.id.clone();
                let tx = event_tx.clone();
                let attempt_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                running.insert(id.clone(), (process_name.clone(), key.clone(), std::time::Instant::now(), Arc::clone(&attempt_count)));
                tasks.spawn(async move {
                    let mut attempts = 0;
                    let result = loop {
                        attempts += 1;
                        attempt_count.store(attempts, std::sync::atomic::Ordering::SeqCst);
                        let (local_tx, mut local_rx) = mpsc::channel::<NodeEvent>(64);
                        let forward_tx = tx.clone();
                        let bridge = tokio::spawn(async move {
                            let mut emitted = false;
                            while let Some(event) = local_rx.recv().await {
                                if matches!(event.kind, NodeEventKind::ChannelItem { .. } | NodeEventKind::ChannelClosed { .. }) {
                                    emitted = true;
                                }
                                let (ack_tx, ack_rx) = oneshot::channel();
                                if forward_tx.send((event, ack_tx)).await.is_err() || ack_rx.await.is_err() { break; }
                            }
                            emitted
                        });
                        let execution = TaskExecution {
                            submission: submission.clone(), node: node.clone_box(),
                            engine_ctx: Arc::clone(&ctx),
                            reporter: NodeReporter::new(id.as_str().to_owned(), local_tx.clone()),
                            cancellation: task_cancel.clone(),
                        };
                        let result = std::panic::AssertUnwindSafe(executor.run(execution))
                            .catch_unwind().await.unwrap_or_else(|_| JobResult::Failed {
                                id: id.as_str().into(),
                                error: dag_core::dag::DagError::Schedule("task executor panicked".into()),
                                duration: Duration::ZERO, details: None,
                            });
                        drop(local_tx);
                        let emitted = bridge.await.unwrap_or(true);
                        if matches!(result, JobResult::Failed { .. })
                            && strategy.mode == ErrorMode::Retry && attempts <= strategy.max_retries
                            && !task_cancel.is_cancelled() {
                            if emitted {
                                break JobResult::Failed {
                                    id: id.as_str().into(),
                                    error: dag_core::dag::DagError::Schedule(
                                        "cannot retry a task after it emitted channel events; retry would duplicate delivered items".into()),
                                    duration: result.duration(), details: None,
                                };
                            }
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_millis(strategy.retry_delay_ms)) => {},
                                _ = task_cancel.cancelled() => break result,
                            }
                            continue;
                        }
                        break result;
                    };
                    (id, process_name, key, fingerprint, strategy.mode, attempts, result)
                });
            }
            if join_pending.keys().any(|name| flow.can_emit(name, 0, 128)) { continue; }
            if combine_pending.keys().any(|name| flow.can_emit(name, 0, 128)) { continue; }
            if group_pending.keys().any(|name| flow.can_emit(name, 0, 128)) { continue; }
            if flat_map_pending.keys().any(|name| flow.can_emit(name, 0, 128)) { continue; }
            if transpose_pending
                .keys()
                .any(|name| flow.can_emit(name, 0, 128))
            {
                continue;
            }
            if pending_events
                .iter()
                .any(|(event, _)| event_can_deliver(&flow, event))
            {
                continue;
            }
            if tasks.is_empty() && function_tasks.is_empty() {
                if flow.is_closed() || token.is_cancelled() {
                    break;
                }
                if sources
                    .iter()
                    .any(|state| !state.closed && flow.can_emit(&state.source.id, 0, 128))
                {
                    tokio::select! {
                        _ = token.cancelled() => { failed = true; break; }
                        (index, value) = wait_source(&mut sources, &flow) => { sources[index].pending = Some(value); }
                    }
                    continue;
                }
                return Err(Error::Execution(
                    "dataflow stalled with open channels".into(),
                ));
            }
            tokio::select! {
                _ = token.cancelled() => { failed = true; break; }
                (index, value) = wait_source(&mut sources, &flow), if tasks.len() + function_tasks.len() < limit => { sources[index].pending = Some(value); }
                Some((event, ack)) = event_rx.recv() => {
                    if event_can_deliver(&flow, &event) {
                        deliver_event(&mut flow, &mut streamed, event)?;
                        let _ = ack.send(());
                    } else {
                        pending_events.push_back((event, ack));
                    }
                }
                joined = function_tasks.join_next(), if !function_tasks.is_empty() => {
                    let (name, result) = joined.ok_or_else(|| Error::Execution("missing channel function task".into()))?
                        .map_err(|e| Error::Execution(format!("channel function failed: {e}")))?;
                    match result.map_err(Error::Execution)? {
                        FunctionResult::Item(value) => function_pending.push_back((name, 0, value, false)),
                        FunctionResult::Branch(result) => match result {
                            Some((port, value)) => function_pending.push_back((name, port, Some(value), false)),
                            None => function_pending.push_back((name, 0, None, false)),
                        },
                        FunctionResult::Until { value, stop } => function_pending.push_back((name, 0, value, stop)),
                        FunctionResult::Reduce(value) => {
                            reduce_state.insert(name.clone(), (!node_is_null(&value)).then_some(value));
                            flow.finish(&name).map_err(Error::Execution)?;
                        },
                        FunctionResult::Collect { value, flatten } => {
                            if matches!(value, NodeValue::DataFrame(_)) {
                                return Err(Error::Execution("collect functions do not yet support DataFrame values".into()));
                            }
                            append_collection(list_state.entry(name.clone()).or_default(), &value, flatten);
                            flow.finish(&name).map_err(Error::Execution)?;
                        },
                        FunctionResult::Dedup { key, value, distinct } => {
                            let is_null = node_is_null(&key);
                            let key = node_comparison_key(&key)?;
                            let emit = if distinct {
                                distinct_state.insert(name.clone(), key.clone()).map_or(!is_null, |previous| previous != key)
                            } else {
                                unique_state.entry(name.clone()).or_default().insert(key)
                            };
                            function_pending.push_back((name, 0, emit.then_some(value), false));
                        },
                        FunctionResult::First(value) => {
                            let stop = value.is_some();
                            function_pending.push_back((name, 0, value, stop));
                        }
                        FunctionResult::Count(matched) => {
                            if matched {
                                let count = count_state.entry(name.clone()).or_default();
                                *count = count.checked_add(1).ok_or_else(|| Error::Execution("count overflow".into()))?;
                            }
                            flow.finish(&name).map_err(Error::Execution)?;
                        }
                        FunctionResult::FlatMap(value) if value.is_null() => {
                            flow.finish(&name).map_err(Error::Execution)?;
                        }
                        FunctionResult::FlatMap(value) => {
                            flat_map_pending.insert(name, PendingFlatMap::Json(FlatMapRows::new(value)));
                        }
                        FunctionResult::FlatFiles(files) => {
                            flat_map_pending.insert(name, PendingFlatMap::Files(files.into_iter()));
                        }
                        FunctionResult::FlatStructured(value) => {
                            use dag_core::dag::execution::ChannelTransportValue as T;
                            let items = match value {
                                T::List(items) | T::Tuple(items) => items,
                                T::Map(items) => items.into_iter().map(|(key, value)| T::Tuple(vec![T::Json(serde_json::Value::String(key)), value])).collect(),
                                other => vec![other],
                            };
                            flat_map_pending.insert(name, PendingFlatMap::Structured(items.into_iter()));
                        }
                    }
                }
                joined = tasks.join_next(), if !tasks.is_empty() => {
                    let (id, process, key, fingerprint, mode, attempts, result) = joined.ok_or_else(|| Error::Execution("missing task".into()))?.map_err(|e| Error::Execution(e.to_string()))?;
                    running.remove(&id);
                    let (status, outputs, error, duration, ignored) = match result {
                        JobResult::Success { outputs, duration, .. } => {
                            let has_stream = streamed.iter().any(|(task, _)| task == id.as_str());
                            if !has_stream && cacheable_outputs(&outputs) {
                                if let (Some(fingerprint), Some(cache)) = (fingerprint.as_ref(), cache.as_deref_mut()) {
                                    cache.record(&process, Some(fingerprint), fingerprint.clone(), &outputs);
                                }
                            }
                            for (port, value) in outputs.iter() {
                                if streamed.contains(&(id.as_str().to_owned(), *port)) { continue; }
                                match value {
                                    NodeValue::Channel(channel) if flow.output_kind(&process) == Some(crate::channel::ChannelKind::Queue) => {
                                        for item in &channel.items {
                                            flow.emit(&process, *port, source_value(item)).map_err(Error::Execution)?;
                                        }
                                    }
                                    _ => flow.emit(&process, *port, value.clone()).map_err(Error::Execution)?,
                                }
                            }
                            (RuntimeStatus::Success, outputs.iter().map(|(p,v)| (*p,v.clone())).collect(), None, duration, false)
                        }
                        JobResult::Failed { error, duration, .. } => {
                            if let (Some(fingerprint), Some(cache)) = (fingerprint.as_ref(), cache.as_deref_mut()) {
                                cache.invalidate(&process, Some(fingerprint));
                            }
                            let ignored = mode == ErrorMode::Ignore;
                            if !ignored { failed = true; token.cancel(); }
                            (RuntimeStatus::Failed, BTreeMap::new(), Some(error.to_string()), duration, ignored)
                        }
                    };
                    flow.finish(&process).map_err(Error::Execution)?;
                    records.push(InstanceRecord { id, process, item_key: Some(key), status, reused: false, ignored, attempts, outputs, error, elapsed_ms: duration.as_millis() as u64 });
                }
            }
        }
        Ok(())
        }.await;
        let mut cleanup_errors = Vec::new();
        if loop_result.is_err() || token.is_cancelled() {
            // Closing the receiver releases task event acknowledgements. Remote
            // executors must finish their cancellation/reconciliation before
            // their futures are dropped; local executors retain the bounded wait.
            token.cancel();
            drop(pending_events);
            drop(event_rx);
            let mut completed = Vec::new();
            let drain = async {
                while let Some(result) = tasks.join_next().await {
                    completed.push(result);
                }
            };
            let expired = if executor.requires_cancel_join() {
                drain.await;
                false
            } else {
                tokio::time::timeout(Duration::from_secs(1), drain)
                    .await
                    .is_err()
            };
            if expired {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
            }
            for (id, process, key, _, _, attempts, result) in completed.into_iter().flatten() {
                running.remove(&id);
                let error = match &result {
                    JobResult::Failed { error, .. } => Some(error.to_string()),
                    JobResult::Success { .. } => None,
                };
                if let Some(error) = error
                    .as_ref()
                    .filter(|error| error.contains("remote_stop_unconfirmed"))
                {
                    cleanup_errors.push(error.clone());
                }
                let status = if error
                    .as_ref()
                    .is_some_and(|error| error.contains("remote_stop_unconfirmed"))
                {
                    RuntimeStatus::Failed
                } else {
                    RuntimeStatus::Cancelled
                };
                records.push(InstanceRecord {
                    id,
                    process,
                    item_key: Some(key),
                    status,
                    reused: false,
                    ignored: false,
                    attempts,
                    outputs: BTreeMap::new(),
                    error,
                    elapsed_ms: result.duration().as_millis().min(u64::MAX as u128) as u64,
                });
            }
        }
        if let Err(error) = loop_result {
            if !cleanup_errors.is_empty() {
                return Err(Error::Execution(format!(
                    "{error}; {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(error);
        }
        if token.is_cancelled() {
            for (id, (process, key, started, attempts)) in running {
                records.push(InstanceRecord {
                    id,
                    process,
                    item_key: Some(key),
                    status: RuntimeStatus::Cancelled,
                    reused: false,
                    ignored: false,
                    attempts: attempts.load(std::sync::atomic::Ordering::SeqCst),
                    outputs: BTreeMap::new(),
                    error: None,
                    elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                });
            }
        }
        let mut processes: BTreeMap<_, _> = graph
            .processes()
            .iter()
            .map(|p| (p.id.clone(), ProcessRunSummary::default()))
            .collect();
        for r in &records {
            let p = processes.get_mut(&r.process).unwrap();
            p.total += 1;
            p.succeeded += usize::from(r.status == RuntimeStatus::Success);
            p.reused += usize::from(r.reused);
            p.failed += usize::from(r.status == RuntimeStatus::Failed);
            p.cancelled += usize::from(r.status == RuntimeStatus::Cancelled);
            p.ignored += usize::from(r.ignored);
            p.summed_elapsed_ms += r.elapsed_ms;
        }
        Ok(FlowRunReport {
            ok: !failed && !token.is_cancelled(),
            cancelled: caller_token.is_cancelled(),
            execution_binding: executor.execution_binding(),
            processes,
            instances: records,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_remainder_respects_downstream_capacity_and_order() {
        use serde_json::json;
        let graph = crate::model::FlowGraphBuilder::default()
            .source(crate::model::Source::items("a", vec![]))
            .source(crate::model::Source::items("b", vec![]))
            .process(crate::model::Process::new(
                "join",
                "channel",
                json!({"operator":"join", "remainder":true}),
            ))
            .process(crate::model::Process::new(
                "sink",
                "channel",
                json!({"operator":"take", "count":10}),
            ))
            .wire(crate::model::Wire::direct("a", 0, "join", 0))
            .wire(crate::model::Wire::direct("b", 0, "join", 1))
            .wire(crate::model::Wire::direct("join", 0, "sink", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::<NodeValue>::new(&graph).unwrap();
        let mut groups = BTreeMap::new();
        for n in 0..5 {
            groups.insert(
                json!([n]).to_string(),
                (VecDeque::from([vec![json!(n)]]), VecDeque::new()),
            );
        }
        let mut pending = PendingJoin {
            groups,
            singleton: [None, None],
            typed_keys: None,
        };
        for start in [0, 2, 4] {
            let done = deliver_pending_join(&mut flow, "join", &mut pending, 2).unwrap();
            let count = if start == 4 { 1 } else { 2 };
            assert_eq!(flow.buffered_for("sink"), count);
            assert_eq!(done, start == 4);
            for n in start..start + count {
                let activation = flow.next_activation().unwrap().unwrap();
                assert_eq!(activation.process, "sink");
                let Delivery::Value(value) = &activation.inputs[0].1 else {
                    panic!("expected value")
                };
                assert_eq!(json_value(value), json!([n, n, null]));
                flow.finish("sink").unwrap();
            }
        }
    }

    #[test]
    fn combine_fanout_respects_downstream_capacity_and_order() {
        use serde_json::json;
        let graph = crate::model::FlowGraphBuilder::default()
            .source(crate::model::Source::items("a", vec![]))
            .source(crate::model::Source::items("b", vec![]))
            .process(crate::model::Process::new(
                "combine",
                "channel",
                json!({"operator":"combine"}),
            ))
            .process(crate::model::Process::new(
                "sink",
                "channel",
                json!({"operator":"take", "count":10}),
            ))
            .wire(crate::model::Wire::direct("a", 0, "combine", 0))
            .wire(crate::model::Wire::direct("b", 0, "combine", 1))
            .wire(crate::model::Wire::direct("combine", 0, "sink", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::<NodeValue>::new(&graph).unwrap();
        let mut states = CombineState::new();
        states.entry("combine".into()).or_default().insert(
            "[]".into(),
            (
                vec![vec![json!("left")]],
                (0..5).map(|n| vec![json!(n)]).collect(),
            ),
        );
        let mut pending = PendingCombine {
            key: "[]".into(),
            keys: vec![],
            side: 0,
            own_index: 0,
            next: 0,
            count: 5,
            typed: false,
        };
        for start in [0, 2, 4] {
            let done =
                deliver_pending_combine(&mut flow, "combine", &mut pending, &states, 2).unwrap();
            let count = if start == 4 { 1 } else { 2 };
            assert_eq!(flow.buffered_for("sink"), count);
            assert_eq!(done, start == 4);
            for n in start..start + count {
                let activation = flow.next_activation().unwrap().unwrap();
                assert_eq!(activation.process, "sink");
                let Delivery::Value(value) = &activation.inputs[0].1 else {
                    panic!("expected value")
                };
                assert_eq!(json_value(value), json!(["left", n]));
                flow.finish("sink").unwrap();
            }
        }
    }

    #[test]
    fn group_finalization_respects_downstream_capacity_and_order() {
        use serde_json::json;
        let graph = crate::model::FlowGraphBuilder::default()
            .source(crate::model::Source::items("s", vec![]))
            .process(crate::model::Process::new(
                "group",
                "channel",
                json!({"operator":"group_tuple"}),
            ))
            .process(crate::model::Process::new(
                "sink",
                "channel",
                json!({"operator":"take", "count":10}),
            ))
            .wire(crate::model::Wire::direct("s", 0, "group", 0))
            .wire(crate::model::Wire::direct("group", 0, "sink", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::<NodeValue>::new(&graph).unwrap();
        let mut groups = GroupState::default();
        for n in 0..5 {
            groups
                .add(n.to_string(), vec![json!(n), json!(n)], &[0], None)
                .unwrap();
        }
        let mut pending = PendingGroups {
            groups,
            pivot: vec![0],
            size: None,
            remainder: false,
            sort: false,
        };
        for start in [0, 2, 4] {
            let done = deliver_pending_groups(&mut flow, "group", &mut pending, 2).unwrap();
            let expected_count = if start == 4 { 1 } else { 2 };
            assert_eq!(flow.buffered_for("sink"), expected_count);
            assert_eq!(done, start == 4);
            for n in start..start + expected_count {
                let activation = flow.next_activation().unwrap().unwrap();
                assert_eq!(activation.process, "sink");
                let Delivery::Value(value) = &activation.inputs[0].1 else {
                    panic!("expected value")
                };
                assert_eq!(json_value(value), json!([n, [n]]));
                flow.finish("sink").unwrap();
            }
        }
    }

    #[test]
    fn grouped_many_distinct_keys_keep_first_arrival_order() {
        use serde_json::json;
        let mut state = GroupState::default();
        for key in (0..10_000).rev() {
            assert!(
                state
                    .add(key.to_string(), vec![json!(key), json!(key)], &[0], None)
                    .unwrap()
                    .is_none()
            );
        }
        let groups: Vec<_> = std::iter::from_fn(|| state.pop_first()).collect();
        assert_eq!(groups.len(), 10_000);
        for (index, (tuple, count)) in groups.into_iter().enumerate() {
            assert_eq!(tuple[0], json!(9_999 - index));
            assert_eq!(count, 1);
        }
    }

    #[test]
    fn grouped_keys_keep_arrival_order_after_completed_groups_are_reinserted() {
        use serde_json::json;
        let mut state = GroupState::default();
        let add = |state: &mut GroupState, key: &str, value: i32| {
            state
                .add(key.into(), vec![json!(key), json!(value)], &[0], Some(2))
                .unwrap()
        };
        assert!(add(&mut state, "z", 1).is_none());
        assert!(add(&mut state, "a", 2).is_none());
        assert_eq!(
            add(&mut state, "z", 3),
            Some(vec![json!("z"), json!([1, 3])])
        );
        assert!(add(&mut state, "z", 4).is_none());
        assert_eq!(
            std::iter::from_fn(|| state.pop_first()).collect::<Vec<_>>(),
            vec![
                (vec![json!("a"), json!([2])], 1),
                (vec![json!("z"), json!([4])], 1)
            ]
        );
    }

    #[test]
    fn group_tuple_natural_sort_sorts_non_key_columns_independently() {
        use serde_json::json;
        let mut tuple = vec![json!("group"), json!([3, 1, 2]), json!(["z", "a", "m"])];
        sort_group_tuple(&mut tuple, &[0]).unwrap();
        assert_eq!(
            tuple,
            vec![json!("group"), json!([1, 2, 3]), json!(["a", "m", "z"])]
        );
    }

    #[test]
    fn collect_preserves_typed_files_in_mixed_tuples() {
        use dag_core::dag::execution::ChannelTransportValue as T;
        let fingerprint = dag_core::value::FileFingerprint {
            size: 7,
            mtime_ns: 9,
            content_hash: Some("digest".into()),
            immutable_remote: false,
        };
        let file = FileRef {
            path: "/tmp/reads.fastq".into(),
            format: Some("fastq".into()),
            fingerprint: Some(fingerprint.clone()),
        };
        let value = collect(vec![
            NodeValue::Structured(T::Tuple(vec![
                T::Json(serde_json::json!("sample")),
                T::File(file),
            ])),
            source_value(&serde_json::json!(2)),
        ]);
        let NodeValue::Structured(T::List(items)) = value else {
            panic!("structured list expected")
        };
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0], T::Json(value) if value == "sample"));
        assert!(
            matches!(&items[1], T::File(file) if file.fingerprint.as_ref() == Some(&fingerprint))
        );
        assert!(matches!(&items[2], T::Json(value) if value == 2));
    }

    #[test]
    fn collected_file_values_are_a_file_set_with_original_metadata() {
        use dag_core::dag::execution::ChannelTransportValue as T;
        let file = FileRef {
            path: "/tmp/sample.fastq".into(),
            format: Some("fastq".into()),
            fingerprint: Some(dag_core::value::FileFingerprint {
                size: 7,
                mtime_ns: 9,
                content_hash: Some("digest".into()),
                immutable_remote: false,
            }),
        };
        let value = collection_value(vec![T::File(file.clone()), T::File(file.clone())]);
        let NodeValue::FileSet(files) = value else {
            panic!("file set expected")
        };
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, file.path);
        assert_eq!(files[0].fingerprint, file.fingerprint);
        assert_eq!(files[1].path, file.path);
    }

    #[test]
    fn flat_map_file_rows_preserve_file_metadata() {
        use dag_core::value::FileFingerprint;
        let fingerprint = FileFingerprint {
            size: 7,
            mtime_ns: 10,
            content_hash: Some("hash".into()),
            immutable_remote: false,
        };
        let file = FileRef {
            path: "/tmp/a.fastq".into(),
            format: Some("fastq".into()),
            fingerprint: Some(fingerprint.clone()),
        };
        let mut rows = PendingFlatMap::Files(vec![file].into_iter());
        let Some(NodeValue::File(output)) = rows.next() else {
            panic!("expected a file")
        };
        assert_eq!(output.format.as_deref(), Some("fastq"));
        assert_eq!(output.fingerprint.as_ref(), Some(&fingerprint));
        assert!(rows.done());
    }

    #[test]
    fn flat_map_expands_one_level_and_map_entries() {
        use serde_json::json;
        assert_eq!(
            FlatMapRows::new(json!([1, [2, 3]])).collect::<Vec<_>>(),
            vec![json!(1), json!([2, 3])]
        );
        assert_eq!(
            FlatMapRows::new(json!({"a":1,"b":2})).collect::<Vec<_>>(),
            vec![json!(["a", 1]), json!(["b", 2])]
        );
        assert_eq!(
            FlatMapRows::new(json!(null)).collect::<Vec<_>>(),
            vec![json!(null)]
        );
        assert!(FlatMapRows::new(json!([])).done());
    }

    #[test]
    fn transpose_matches_nextflow_tuple_examples() {
        use serde_json::json;
        let tuple = json!(["a", [1, 2, 3], ["p", "q"]]);
        assert_eq!(
            transpose_tuple(tuple.clone(), None, false).unwrap(),
            vec![json!(["a", 1, "p"]), json!(["a", 2, "q"])]
        );
        assert_eq!(
            transpose_tuple(tuple.clone(), None, true).unwrap(),
            vec![
                json!(["a", 1, "p"]),
                json!(["a", 2, "q"]),
                json!(["a", 3, null])
            ]
        );
        assert_eq!(
            transpose_tuple(tuple, Some(&json!(1)), false).unwrap(),
            vec![
                json!(["a", 1, ["p", "q"]]),
                json!(["a", 2, ["p", "q"]]),
                json!(["a", 3, ["p", "q"]])
            ]
        );
        assert!(transpose_tuple(json!(["x", 1]), Some(&json!(1)), false).is_err());
        assert_eq!(
            transpose_tuple(json!("scalar"), None, false).unwrap(),
            vec![json!("scalar")]
        );
    }

    #[test]
    fn buffer_matches_nextflow_skip_size_and_remainder() {
        let mut state = BufferState::default();
        let items = [1, 2, 3, 4, 5, 1, 2, 3, 4, 5, 1, 2, 9];
        let mut batches: Vec<_> = items
            .into_iter()
            .filter_map(|v| state.push(serde_json::json!(v), 3, 2))
            .collect();
        batches.extend(state.finish(true));
        assert_eq!(
            batches,
            vec![
                vec![
                    serde_json::json!(3),
                    serde_json::json!(4),
                    serde_json::json!(5)
                ],
                vec![
                    serde_json::json!(3),
                    serde_json::json!(4),
                    serde_json::json!(5)
                ],
                vec![serde_json::json!(9)]
            ]
        );
        let mut no_remainder = BufferState::default();
        assert!(no_remainder.push(serde_json::json!(1), 2, 0).is_none());
        assert!(no_remainder.finish(false).is_none());
    }

    #[test]
    fn natural_sort_orders_scalars_and_rejects_incomparable_values() {
        use serde_json::json;
        let mut numbers = vec![json!(3), json!(1), json!(4), json!(2)];
        sort_channel_values(&mut numbers).unwrap();
        assert_eq!(numbers, vec![json!(1), json!(2), json!(3), json!(4)]);
        let mut large = vec![json!(9007199254740993_u64), json!(9007199254740992_u64)];
        sort_channel_values(&mut large).unwrap();
        assert_eq!(large[0], json!(9007199254740992_u64));
        let mut strings = vec![json!("z"), json!("a")];
        sort_channel_values(&mut strings).unwrap();
        assert_eq!(strings, vec![json!("a"), json!("z")]);
        assert!(sort_channel_values(&mut [json!("a"), json!(1)]).is_err());
        assert!(sort_channel_values(&mut [json!(9007199254740993_u64), json!(1.5)]).is_err());
    }

    #[test]
    fn natural_sort_preserves_file_references_and_rejects_mixed_types() {
        use dag_core::dag::execution::ChannelTransportValue as T;
        let mut files = vec![
            T::File(FileRef::new("/reads/z.fastq", Some("fastq".into()))),
            T::File(FileRef::new("/reads/a.fastq", Some("fastq".into()))),
        ];
        sort_channel_transport_values(&mut files).unwrap();
        assert!(
            matches!(&files[0], T::File(file) if file.path == "/reads/a.fastq" && file.format.as_deref() == Some("fastq"))
        );
        assert!(matches!(&files[1], T::File(file) if file.path == "/reads/z.fastq"));
        let mut mixed = vec![files[0].clone(), T::Json(serde_json::json!("a"))];
        assert!(sort_channel_transport_values(&mut mixed).is_err());
    }

    #[test]
    fn group_output_preserves_and_sorts_file_references() {
        use dag_core::dag::execution::ChannelTransportValue as T;
        let file = |path: &str| T::File(FileRef::new(path, Some("fastq".into())));
        let encode = |value: T| serde_json::to_value(value).unwrap();
        let tuple = vec![
            encode(T::Json(serde_json::json!("sample"))),
            serde_json::json!([
                encode(file("/reads/z.fastq")),
                encode(file("/reads/a.fastq"))
            ]),
        ];
        let NodeValue::Structured(T::Tuple(items)) = typed_group_output(tuple, &[0], true).unwrap()
        else {
            panic!("typed grouped tuple expected");
        };
        assert!(
            matches!(&items[1], T::List(files) if matches!(&files[0], T::File(f) if f.path == "/reads/a.fastq" && f.format.as_deref() == Some("fastq")))
        );
    }

    #[test]
    fn structured_dedup_compares_file_identity_without_metadata() {
        use dag_core::dag::execution::ChannelTransportValue as T;
        let first = FileRef::new("/reads/a.fastq", Some("fastq".into()));
        let second = FileRef::new("/reads/a.fastq", None);
        let tuple = |value| {
            NodeValue::Structured(T::Tuple(vec![T::Json(serde_json::json!("sample")), value]))
        };
        assert_eq!(
            node_comparison_key(&tuple(T::File(first))).unwrap(),
            node_comparison_key(&tuple(T::File(second))).unwrap()
        );
        assert_ne!(
            node_comparison_key(&tuple(T::File(FileRef::new("/reads/a.fastq", None)))).unwrap(),
            node_comparison_key(&tuple(T::Json(serde_json::json!("/reads/a.fastq")))).unwrap()
        );
    }

    #[test]
    fn flatten_nested_values_preserves_files_and_order() {
        use dag_core::dag::execution::ChannelTransportValue as T;
        let file = FileRef::new("/reads/a.fastq", Some("fastq".into()));
        let mut rows = PendingFlatMap::Flatten(vec![T::Tuple(vec![
            T::List(vec![
                T::Json(serde_json::json!(1)),
                T::List(vec![T::File(file)]),
            ]),
            T::Json(serde_json::json!([2, []])),
        ])]);
        assert_eq!(json_value(&rows.next().unwrap()), serde_json::json!(1));
        assert!(
            matches!(rows.next(), Some(NodeValue::File(file)) if file.path == "/reads/a.fastq")
        );
        assert_eq!(json_value(&rows.next().unwrap()), serde_json::json!(2));
        assert!(rows.next().is_none());
        assert!(rows.done());
    }

    #[test]
    fn transpose_file_lists_preserves_types_and_remainder() {
        use dag_core::dag::execution::ChannelTransportValue as T;
        let tuple = vec![
            T::Json(serde_json::json!("sample")),
            T::List(vec![
                T::File(FileRef::new("a.fastq", Some("fastq".into()))),
                T::File(FileRef::new("b.fastq", Some("fastq".into()))),
            ]),
            T::Json(serde_json::json!([1])),
        ];
        let mut rows = TypedTransposeRows::new(tuple, None, true).unwrap();
        assert!(
            matches!(rows.next(), Some(NodeValue::Structured(T::Tuple(items))) if matches!(&items[1], T::File(file) if file.path == "a.fastq") && matches!(&items[2], T::Json(value) if value == 1))
        );
        assert!(
            matches!(rows.next(), Some(NodeValue::Structured(T::Tuple(items))) if matches!(&items[1], T::File(file) if file.path == "b.fastq") && matches!(&items[2], T::Json(value) if value.is_null()))
        );
        assert!(rows.done());
    }

    #[test]
    fn branch_projection_exposes_nested_file_metadata() {
        use dag_core::dag::execution::ChannelTransportValue as T;
        let value = T::Tuple(vec![
            T::Json(serde_json::json!("sample")),
            T::File(FileRef::new("/data/a.fastq", Some("fastq".into()))),
        ]);
        let projection = predicate_projection(&value).unwrap();
        assert_eq!(projection.pointer("/0"), Some(&serde_json::json!("sample")));
        assert_eq!(
            projection.pointer("/1/path"),
            Some(&serde_json::json!("/data/a.fastq"))
        );
    }

    #[test]
    fn collect_flattens_list_values_one_level() {
        let result = collect(vec![
            source_value(&serde_json::json!([1, [2, 3]])),
            source_value(&serde_json::json!(4)),
        ]);
        let NodeValue::Channel(result) = result else {
            panic!("expected channel value")
        };
        assert_eq!(
            result.items,
            vec![
                serde_json::json!(1),
                serde_json::json!([2, 3]),
                serde_json::json!(4)
            ]
        );
    }

    #[test]
    fn remote_file_metadata_is_not_cached_as_local() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("same-name");
        std::fs::write(&path, "payload").unwrap();
        let mut reference = FileRef::local(&path, None).unwrap();
        reference.fingerprint.as_mut().unwrap().immutable_remote = true;
        let mut outputs = dag_core::dag::graph::PortOutputs::new();
        outputs.insert(0, NodeValue::File(reference));
        assert!(!cacheable_outputs(&outputs));
    }

    #[test]
    fn bare_channel_strings_with_vfs_are_not_assumed_to_be_local() {
        let process = crate::model::Process::new(
            "test",
            "channel",
            serde_json::json!({"operator":"flatten"}),
        );
        let node = dag_core::dag::ChannelNode::from_spec(&process.spec).unwrap();
        let inputs = vec![NodeInput {
            port: 0,
            data: source_value(&serde_json::json!({"reads":["data/a.fastq"]})),
        }];
        assert!(input_fingerprint(&process, &inputs, &node, None, true).is_none());
        assert!(input_fingerprint(&process, &inputs, &node, None, false).is_some());
    }
}
