//! Run-local dataflow coordinator. Tasks are created from delivered input tuples;
//! authored graphs are never expanded into a precomputed instance graph.

use std::collections::{BTreeMap, BTreeSet};

use crate::workflow::channel::{AnyFiring, ChannelKind, Collect, Firing, InputSet};
use crate::workflow::model::{FlowGraph, WireOp};

/// A direct input carries one value; collect carries the values seen before
/// upstream closure. The executor adapter converts these into typed node inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery<T> {
    Value(T),
    Collected(Vec<T>),
}

#[derive(Debug)]
pub struct Activation<T> {
    pub process: String,
    pub inputs: Vec<(u8, Delivery<T>)>,
    pub closing: bool,
}

struct ProcessState<T> {
    ports: Vec<u8>,
    inputs: InputSet<Delivery<T>>,
    active: usize,
    closed: bool,
    output_kind: ChannelKind,
    is_channel: bool,
    serial: bool,
    mix: bool,
    concat_order: Option<Vec<usize>>,
    finalize: bool,
    finalized: bool,
}

struct Connection<T> {
    from: String,
    from_port: u8,
    to: String,
    input: usize,
    collect: Option<Collect<T>>,
    closed: bool,
    abandoned: bool,
}

/// JSON Schema for the channel operators understood by this runtime.
/// Operators are scheduler primitives, rather than registry node factories.
pub fn channel_operator_schema() -> serde_json::Value {
    use serde_json::json;
    let index = json!({"oneOf":[{"type":"integer","minimum":0},{"type":"array","items":{"type":"integer","minimum":0}},{"type":"null"}]});
    let mut variants = Vec::new();
    let mut add = |name: &str,
                   mut properties: serde_json::Map<String, serde_json::Value>,
                   required: &[&str]| {
        properties.insert("operator".into(), json!({"const":name}));
        let mut fields = vec!["operator"];
        fields.extend_from_slice(required);
        variants.push(json!({"type":"object", "properties":properties, "required":fields, "additionalProperties":false}));
    };
    let props = |pairs: Vec<(&str, serde_json::Value)>| {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    };
    add(
        "of_items",
        props(vec![("items", json!({"type":"array"}))]),
        &["items"],
    );
    add(
        "flat_map",
        props(vec![
            ("template", json!({})),
            ("function", json!({"type":"object"})),
        ]),
        &[],
    );
    add(
        "map",
        props(vec![
            ("template", json!({})),
            ("function", json!({"type":"object"})),
        ]),
        &[],
    );
    add(
        "filter",
        props(vec![
            ("path", json!({"type":"string"})),
            ("equals", json!({})),
            ("function", json!({"type":"object"})),
        ]),
        &[],
    );
    add(
        "branch",
        props(vec![(
            "branches",
            json!({"type":"array","items":{"oneOf":[{"type":"object","properties":{"name":{"type":"string"},"path":{"type":"string"},"equals":{},"not_equals":{},"exists":{"type":"boolean"},"prefix":{"type":"string"},"suffix":{"type":"string"},"contains":{"type":"string"}},"required":["name","path"]},{"type":"object","properties":{"name":{"type":"string"},"function":{"type":"object"},"transform":{"type":"object"}},"required":["name","function"],"additionalProperties":false}]}}),
        )]),
        &["branches"],
    );
    for name in [
        "flatten",
        "mix",
        "concat",
        "to_list",
        "to_sorted_list",
        "last",
    ] {
        add(name, props(vec![]), &[]);
    }
    for name in ["unique", "distinct"] {
        add(
            name,
            props(vec![("function", json!({"type":"object"}))]),
            &[],
        );
    }
    add(
        "until",
        props(vec![("function", json!({"type":"object"}))]),
        &["function"],
    );
    add(
        "reduce",
        props(vec![
            ("function", json!({"type":"object"})),
            ("seed", json!({})),
        ]),
        &["function"],
    );
    for name in ["first", "count"] {
        add(
            name,
            props(vec![(
                "criteria",
                json!({"type":["null","boolean","number","string","array","object"]}),
            )]),
            &[],
        );
    }
    add(
        "take",
        props(vec![("count", json!({"type":"integer"}))]),
        &["count"],
    );
    add("if_empty", props(vec![("value", json!({}))]), &["value"]);
    add(
        "buffer",
        props(vec![
            ("size", json!({"type":"integer","minimum":1})),
            ("skip", json!({"type":"integer","minimum":0})),
            ("remainder", json!({"type":"boolean"})),
        ]),
        &["size"],
    );
    add(
        "collect",
        props(vec![
            ("flat", json!({"type":"boolean"})),
            ("sort", json!({"type":"boolean"})),
        ]),
        &[],
    );
    add(
        "transpose",
        props(vec![
            ("by", index.clone()),
            ("remainder", json!({"type":"boolean"})),
        ]),
        &[],
    );
    add("combine", props(vec![("by", index.clone())]), &[]);
    add(
        "join",
        props(vec![
            ("by", index.clone()),
            ("remainder", json!({"type":"boolean"})),
            ("failOnMismatch", json!({"type":"boolean"})),
            ("failOnDuplicate", json!({"type":"boolean"})),
        ]),
        &[],
    );
    add(
        "group_tuple",
        props(vec![
            ("by", index),
            ("size", json!({"type":["integer","null"],"minimum":1})),
            ("remainder", json!({"type":"boolean"})),
            ("sort", json!({"enum":[true,"true","natural",null]})),
        ]),
        &[],
    );
    json!({"title":"Channel operator", "description":"A built-in dataflow operator used as kind 'channel' in dag_shell; wire ports according to the operator.", "oneOf":variants})
}

fn validate_channel_spec(
    process: &crate::workflow::model::Process,
    ports: &BTreeSet<u8>,
) -> Result<(), String> {
    let op = process
        .spec
        .get("operator")
        .and_then(|v| v.as_str())
        .ok_or("channel operator is missing")?;
    match op {
        "of_items" | "map" | "filter" | "flatten" | "branch" => {
            if op == "branch"
                && process
                    .spec
                    .get("branches")
                    .and_then(|v| v.as_array())
                    .is_some_and(|branches| branches.iter().any(|b| b.get("function").is_some()))
            {
                let object = process
                    .spec
                    .as_object()
                    .ok_or("invalid branch specification")?;
                let branches = object
                    .get("branches")
                    .and_then(|v| v.as_array())
                    .ok_or("branch requires branches")?;
                if object.len() != 2 || branches.is_empty() || branches.len() > 256 {
                    return Err("invalid branch specification".into());
                }
                let mut names = BTreeSet::new();
                for branch in branches {
                    let branch = branch.as_object().ok_or("invalid branch")?;
                    if branch
                        .keys()
                        .any(|key| !["name", "function", "transform"].contains(&key.as_str()))
                    {
                        return Err("unsupported branch option".into());
                    }
                    let name = branch
                        .get("name")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .ok_or("branch requires name")?;
                    if !names.insert(name) {
                        return Err(format!("duplicate branch name `{name}`"));
                    }
                    for function in
                        std::iter::once(branch.get("function").ok_or("branch requires function")?)
                            .chain(branch.get("transform"))
                    {
                        let function = function
                            .get("__dag_shell_function__")
                            .and_then(|v| v.as_object())
                            .ok_or("invalid channel function")?;
                        if !(function.len() == 2
                            || (function.len() == 3
                                && function.get("functions").is_some_and(|v| v.is_array())))
                            || !function.get("source").is_some_and(|v| v.is_string())
                            || !function.get("captures").is_some_and(|v| v.is_object())
                        {
                            return Err("invalid channel function source or captures".into());
                        }
                    }
                }
            } else if matches!(op, "map" | "filter") && process.spec.get("function").is_some() {
                let object = process
                    .spec
                    .as_object()
                    .ok_or("map specification must be an object")?;
                if object.len() != 2 || !object.contains_key("operator") {
                    return Err(format!(
                        "{op} function cannot be combined with other options"
                    ));
                }
                let function = process.spec["function"]
                    .get("__dag_shell_function__")
                    .and_then(|v| v.as_object())
                    .ok_or("invalid channel function")?;
                if !(function.len() == 2
                    || (function.len() == 3
                        && function.get("functions").is_some_and(|v| v.is_array())))
                    || !function.get("source").is_some_and(|v| v.is_string())
                    || !function.get("captures").is_some_and(|v| v.is_object())
                {
                    return Err("invalid channel function source or captures".into());
                }
            } else {
                dag_core::dag::ChannelNode::from_spec(&process.spec).map_err(|e| e.to_string())?;
            }
            if (op == "of_items" && !ports.is_empty())
                || (op != "of_items" && *ports != BTreeSet::from([0]))
            {
                return Err(format!("channel operator `{op}` has invalid input ports"));
            }
        }
        "to_list" | "to_sorted_list" | "unique" | "distinct" | "until" | "reduce" | "first"
        | "take" | "last" | "count" => {
            if *ports != BTreeSet::from([0]) {
                return Err(format!("{op} requires input port 0"));
            }
            if process.spec.as_object().is_some_and(|fields| {
                fields.keys().any(|key| {
                    key != "operator"
                        && !(matches!(op, "unique" | "distinct" | "until" | "reduce")
                            && key == "function")
                        && !(op == "reduce" && key == "seed")
                        && !(op == "take" && key == "count")
                        && !(matches!(op, "first" | "count") && key == "criteria")
                })
            }) {
                return Err(format!("unsupported {op} option"));
            }
            if matches!(op, "until" | "reduce") && process.spec.get("function").is_none() {
                return Err(format!("{op} requires a function"));
            }
            if let Some(function) = process.spec.get("function") {
                if !matches!(op, "unique" | "distinct" | "until" | "reduce") {
                    return Err(format!("unsupported {op} function"));
                }
                let function = function
                    .get("__dag_shell_function__")
                    .and_then(|v| v.as_object())
                    .ok_or("invalid channel function")?;
                if !(function.len() == 2
                    || (function.len() == 3
                        && function.get("functions").is_some_and(|v| v.is_array())))
                    || !function.get("source").is_some_and(|v| v.is_string())
                    || !function.get("captures").is_some_and(|v| v.is_object())
                {
                    return Err("invalid channel function source or captures".into());
                }
            }
            if let Some(criteria) = process.spec.get("criteria").filter(|v| v.is_object()) {
                let function = criteria
                    .get("__dag_shell_function__")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| {
                        format!("{op} criteria must be a literal or channel function")
                    })?;
                if !(function.len() == 2
                    || (function.len() == 3
                        && function.get("functions").is_some_and(|v| v.is_array())))
                    || !function.get("source").is_some_and(|v| v.is_string())
                    || !function.get("captures").is_some_and(|v| v.is_object())
                {
                    return Err("invalid channel function source or captures".into());
                }
            }
            if op == "take" && process.spec.get("count").and_then(|v| v.as_i64()).is_none() {
                return Err("take requires an integer count".into());
            }
        }
        "flat_map" => {
            if *ports != BTreeSet::from([0]) {
                return Err("flat_map requires input port 0".into());
            }
            if process.spec.as_object().is_some_and(|fields| {
                fields
                    .keys()
                    .any(|key| !["operator", "template", "function"].contains(&key.as_str()))
            }) {
                return Err("unsupported flat_map option".into());
            }
            if let Some(function) = process.spec.get("function") {
                if process.spec.get("template").is_some() {
                    return Err("flat_map function cannot be combined with template".into());
                }
                let function = function
                    .get("__dag_shell_function__")
                    .and_then(|v| v.as_object())
                    .ok_or("invalid channel function")?;
                if !(function.len() == 2
                    || (function.len() == 3
                        && function.get("functions").is_some_and(|v| v.is_array())))
                    || !function.get("source").is_some_and(|v| v.is_string())
                    || !function.get("captures").is_some_and(|v| v.is_object())
                {
                    return Err("invalid channel function source or captures".into());
                }
            }
        }
        "transpose" => {
            if *ports != BTreeSet::from([0]) {
                return Err("transpose requires input port 0".into());
            }
            if process.spec.as_object().is_some_and(|fields| {
                fields
                    .keys()
                    .any(|key| !["operator", "by", "remainder"].contains(&key.as_str()))
            }) {
                return Err("unsupported transpose option".into());
            }
            if let Some(by) = process.spec.get("by").filter(|v| !v.is_null()) {
                let valid_index =
                    |v: &serde_json::Value| v.as_u64().is_some_and(|n| usize::try_from(n).is_ok());
                if !valid_index(by)
                    && !by
                        .as_array()
                        .is_some_and(|indices| indices.iter().all(valid_index))
                {
                    return Err(
                        "transpose by must be a nonnegative index or list of indices".into(),
                    );
                }
            }
            if process
                .spec
                .get("remainder")
                .is_some_and(|v| !v.is_boolean())
            {
                return Err("transpose remainder must be boolean".into());
            }
        }
        "if_empty" => {
            if *ports != BTreeSet::from([0]) {
                return Err("if_empty requires input port 0".into());
            }
            if !process.spec.as_object().is_some_and(|fields| {
                fields.contains_key("value")
                    && fields
                        .keys()
                        .all(|key| ["operator", "value"].contains(&key.as_str()))
            }) {
                return Err("if_empty requires a value and no other options".into());
            }
            if let Some(function) = process.spec["value"].get("__dag_shell_function__") {
                let function = function.as_object().ok_or("invalid channel function")?;
                if !(function.len() == 2
                    || (function.len() == 3
                        && function.get("functions").is_some_and(|v| v.is_array())))
                    || !function.get("source").is_some_and(|v| v.is_string())
                    || !function.get("captures").is_some_and(|v| v.is_object())
                {
                    return Err("invalid channel function source or captures".into());
                }
            }
        }
        "buffer" => {
            if *ports != BTreeSet::from([0]) {
                return Err("buffer requires input port 0".into());
            }
            if process.spec.as_object().is_some_and(|fields| {
                fields
                    .keys()
                    .any(|key| !["operator", "size", "skip", "remainder"].contains(&key.as_str()))
            }) {
                return Err("unsupported buffer option".into());
            }
            if process
                .spec
                .get("size")
                .and_then(|value| value.as_u64())
                .is_none_or(|size| size == 0 || usize::try_from(size).is_err())
            {
                return Err("buffer size must be a positive integer".into());
            }
            if process.spec.get("skip").is_some_and(|value| {
                value
                    .as_u64()
                    .is_none_or(|skip| usize::try_from(skip).is_err())
            }) {
                return Err("buffer skip must be a nonnegative integer".into());
            }
            if process
                .spec
                .get("remainder")
                .is_some_and(|value| !value.is_boolean())
            {
                return Err("buffer remainder must be boolean".into());
            }
        }
        "collect" => {
            if *ports != BTreeSet::from([0]) {
                return Err("collect requires input port 0".into());
            }
            if process.spec.as_object().is_some_and(|fields| {
                fields
                    .keys()
                    .any(|key| !["operator", "flat", "sort", "function"].contains(&key.as_str()))
            }) {
                return Err("unsupported collect option".into());
            }
            if process
                .spec
                .get("flat")
                .is_some_and(|value| !value.is_boolean())
            {
                return Err("collect flat must be boolean".into());
            }
            if let Some(function) = process.spec.get("function") {
                let function = function
                    .get("__dag_shell_function__")
                    .and_then(|v| v.as_object())
                    .ok_or("invalid channel function")?;
                if !(function.len() == 2
                    || (function.len() == 3
                        && function.get("functions").is_some_and(|v| v.is_array())))
                    || !function.get("source").is_some_and(|v| v.is_string())
                    || !function.get("captures").is_some_and(|v| v.is_object())
                {
                    return Err("invalid channel function source or captures".into());
                }
            }
            if let Some(sort) = process.spec.get("sort").filter(|v| !v.is_boolean()) {
                let function = sort
                    .get("__dag_shell_function__")
                    .and_then(|v| v.as_object())
                    .ok_or("collect sort must be boolean or a channel function")?;
                if !(function.len() == 2
                    || (function.len() == 3
                        && function.get("functions").is_some_and(|v| v.is_array())))
                    || !function.get("source").is_some_and(|v| v.is_string())
                    || !function.get("captures").is_some_and(|v| v.is_object())
                {
                    return Err("invalid collect sort function source or captures".into());
                }
            }
        }
        "mix" | "concat" => {
            if ports.len() < 2 {
                return Err(format!("{op} requires at least two inputs"));
            }
            if process
                .spec
                .as_object()
                .is_some_and(|fields| fields.keys().any(|key| key != "operator"))
            {
                return Err(format!("unsupported {op} option"));
            }
        }
        "combine" | "join" | "group_tuple" => {
            let expected = if op == "group_tuple" {
                BTreeSet::from([0])
            } else {
                BTreeSet::from([0, 1])
            };
            if *ports != expected {
                return Err(format!("{op} has invalid input ports"));
            }
            let allowed: &[&str] = match op {
                "join" => &[
                    "operator",
                    "by",
                    "remainder",
                    "failOnMismatch",
                    "failOnDuplicate",
                ],
                "group_tuple" => &["operator", "by", "size", "remainder", "sort"],
                _ => &["operator", "by"],
            };
            if process
                .spec
                .as_object()
                .is_some_and(|fields| fields.keys().any(|key| !allowed.contains(&key.as_str())))
            {
                return Err(format!("unsupported {op} option"));
            }
            if let Some(by) = process.spec.get("by").filter(|v| !v.is_null()) {
                let valid = by.as_u64().is_some()
                    || by
                        .as_array()
                        .is_some_and(|items| items.iter().all(|v| v.as_u64().is_some()));
                if !valid {
                    return Err(format!("invalid {op} by index"));
                }
            }
            for key in ["remainder", "failOnMismatch", "failOnDuplicate"] {
                if process.spec.get(key).is_some_and(|v| !v.is_boolean()) {
                    return Err(format!("{op} {key} must be boolean"));
                }
            }
            if op == "join"
                && process.spec.get("remainder").and_then(|v| v.as_bool()) == Some(true)
                && process.spec.get("failOnMismatch").and_then(|v| v.as_bool()) == Some(true)
            {
                return Err("join remainder conflicts with failOnMismatch".into());
            }
            if op == "group_tuple"
                && process.spec.get("sort").is_some_and(|sort| {
                    !sort.is_null()
                        && sort != &serde_json::json!(true)
                        && sort != "natural"
                        && sort != "true"
                })
            {
                return Err("group_tuple sort supports natural ordering only".into());
            }
            if let Some(size) = process.spec.get("size").filter(|v| !v.is_null())
                && size.as_u64().is_none_or(|n| n == 0)
            {
                return Err("group_tuple size must be positive".into());
            }
        }
        _ => {
            return Err(format!(
                "channel operator `{op}` is not implemented by the streaming runtime"
            ));
        }
    }
    Ok(())
}

/// Runs one copy per workflow invocation. Multiple wires from one output are
/// independent subscriptions; the user explicitly chose multi-reader edges.
pub struct Dataflow<T> {
    processes: BTreeMap<String, ProcessState<T>>,
    connections: Vec<Connection<T>>,
    sources: BTreeSet<String>,
    closed_sources: BTreeSet<String>,
    collect_empty: fn(&T) -> bool,
}

impl<T: Clone> Dataflow<T> {
    pub fn new(graph: &FlowGraph) -> Result<Self, String> {
        graph.validate().map_err(|e| e.to_string())?;
        let sources: BTreeSet<_> = graph.sources().iter().map(|s| s.id.clone()).collect();
        let names: BTreeSet<_> = graph.processes().iter().map(|p| p.id.clone()).collect();
        if sources.len() != graph.sources().len()
            || names.len() != graph.processes().len()
            || !sources.is_disjoint(&names)
        {
            return Err("duplicate graph identifier".into());
        }
        let mut pending = names.clone();
        let mut kinds: BTreeMap<String, ChannelKind> = graph
            .sources()
            .iter()
            .map(|source| {
                (
                    source.id.clone(),
                    match &source.spec {
                        crate::workflow::model::SourceSpec::Value { .. } => ChannelKind::Value,
                        _ => ChannelKind::Queue,
                    },
                )
            })
            .collect();
        let mut processes = BTreeMap::new();
        while !pending.is_empty() {
            let ready: Vec<_> = pending
                .iter()
                .filter(|name| graph.incoming(name).all(|w| kinds.contains_key(&w.from)))
                .cloned()
                .collect();
            if ready.is_empty() {
                return Err("cycle or unknown source in flow graph".into());
            }
            for name in ready {
                let incoming: Vec<_> = graph.incoming(&name).collect();
                let mut seen = BTreeSet::new();
                if incoming.iter().any(|w| !seen.insert(w.to_port)) {
                    return Err(format!(
                        "process `{name}` has multiple wires on one input port"
                    ));
                }
                let input_kinds: Vec<_> = incoming
                    .iter()
                    .map(|w| {
                        if w.op == WireOp::Collect {
                            ChannelKind::Value
                        } else {
                            kinds[&w.from]
                        }
                    })
                    .collect();
                let process = graph.process(&name).expect("process exists");
                if process.kind == "channel" {
                    validate_channel_spec(process, &seen)?;
                    if process.spec.get("operator").and_then(|v| v.as_str()) == Some("take")
                        && input_kinds.first() == Some(&ChannelKind::Value)
                    {
                        return Err("take cannot consume a value channel".into());
                    }
                }
                if process.kind == "channel"
                    && process
                        .spec
                        .get("operator")
                        .and_then(|v| v.as_str())
                        .is_some_and(|op| matches!(op, "combine" | "join"))
                    && seen != BTreeSet::from([0, 1])
                {
                    return Err("operator requires input ports 0 and 1".into());
                }
                if process.kind == "channel"
                    && process.spec.get("operator").and_then(|v| v.as_str()) == Some("group_tuple")
                    && seen != BTreeSet::from([0])
                {
                    return Err("group_tuple requires input port 0".into());
                }
                let output_kind = if process.kind == "channel" {
                    match process.spec.get("operator").and_then(|v| v.as_str()) {
                        Some(
                            "of_items" | "mix" | "concat" | "combine" | "join" | "group_tuple"
                            | "flatten" | "take" | "buffer" | "transpose" | "flat_map",
                        ) => ChannelKind::Queue,
                        Some(
                            "collect" | "to_list" | "to_sorted_list" | "first" | "last" | "count"
                            | "reduce",
                        ) => ChannelKind::Value,
                        Some(
                            "map" | "filter" | "branch" | "unique" | "distinct" | "if_empty"
                            | "until",
                        ) => {
                            if input_kinds.contains(&ChannelKind::Queue) {
                                ChannelKind::Queue
                            } else {
                                ChannelKind::Value
                            }
                        }
                        Some(op) => {
                            return Err(format!(
                                "channel operator `{op}` is not implemented by the streaming runtime"
                            ));
                        }
                        None => return Err("channel operator is missing".into()),
                    }
                } else if input_kinds.contains(&ChannelKind::Queue) {
                    ChannelKind::Queue
                } else {
                    ChannelKind::Value
                };
                kinds.insert(name.clone(), output_kind);
                processes.insert(
                    name.clone(),
                    ProcessState {
                        ports: incoming.iter().map(|w| w.to_port).collect(),
                        inputs: InputSet::new(input_kinds),
                        active: 0,
                        closed: false,
                        output_kind,
                        is_channel: process.kind == "channel",
                        serial: process.kind == "channel"
                            && matches!(
                                process.spec.get("operator").and_then(|v| v.as_str()),
                                Some(
                                    "transpose"
                                        | "flat_map"
                                        | "combine"
                                        | "map"
                                        | "filter"
                                        | "branch"
                                        | "until"
                                        | "reduce"
                                )
                            )
                            || (process.kind == "channel"
                                && process.spec.get("operator").and_then(|v| v.as_str())
                                    == Some("collect")
                                && process.spec.get("function").is_some())
                            || (process.kind == "channel"
                                && matches!(
                                    process.spec.get("operator").and_then(|v| v.as_str()),
                                    Some("unique" | "distinct")
                                )
                                && process.spec.get("function").is_some())
                            || (process.kind == "channel"
                                && matches!(
                                    process.spec.get("operator").and_then(|v| v.as_str()),
                                    Some("first" | "count")
                                )
                                && process.spec.get("criteria").is_some_and(|v| v.is_object())),
                        finalize: process.kind == "channel"
                            && matches!(
                                process.spec.get("operator").and_then(|v| v.as_str()),
                                Some(
                                    "group_tuple"
                                        | "join"
                                        | "collect"
                                        | "to_list"
                                        | "to_sorted_list"
                                        | "buffer"
                                        | "if_empty"
                                        | "last"
                                        | "count"
                                        | "reduce"
                                )
                            ),
                        finalized: false,
                        concat_order: if process.kind == "channel"
                            && process.spec.get("operator").and_then(|v| v.as_str())
                                == Some("concat")
                        {
                            let mut indices: Vec<_> = (0..incoming.len()).collect();
                            indices.sort_by_key(|&index| incoming[index].to_port);
                            Some(indices)
                        } else {
                            None
                        },
                        mix: process.kind == "channel"
                            && matches!(
                                process.spec.get("operator").and_then(|v| v.as_str()),
                                Some(
                                    "mix"
                                        | "combine"
                                        | "join"
                                        | "group_tuple"
                                        | "collect"
                                        | "to_list"
                                        | "to_sorted_list"
                                        | "buffer"
                                        | "if_empty"
                                        | "last"
                                        | "count"
                                        | "reduce"
                                )
                            ),
                    },
                );
                pending.remove(&name);
            }
        }
        let mut input_indices = BTreeMap::<String, usize>::new();
        let mut connections = Vec::new();
        for w in graph.wires() {
            if !kinds.contains_key(&w.from) || !names.contains(&w.to) {
                return Err("unknown wire endpoint".into());
            }
            let input = input_indices.entry(w.to.clone()).or_default();
            connections.push(Connection {
                from: w.from.clone(),
                from_port: w.from_port,
                to: w.to.clone(),
                input: *input,
                collect: (w.op == WireOp::Collect).then(Collect::new),
                closed: false,
                abandoned: false,
            });
            *input += 1;
        }
        Ok(Self {
            processes,
            connections,
            sources,
            closed_sources: BTreeSet::new(),
            collect_empty: |_| false,
        })
    }

    /// Identify items that contribute no elements to a flattened collect.
    pub(crate) fn set_collect_empty(&mut self, is_empty: fn(&T) -> bool) {
        self.collect_empty = is_empty;
    }

    /// Whether every live direct subscriber has space. Collect buffers until
    /// close. A tuple input waiting on another input may exceed the limit so
    /// that sequential outputs from one producer can supply the missing input.
    pub fn can_emit(&self, producer: &str, port: u8, capacity: usize) -> bool {
        self.connections
            .iter()
            .filter(|c| c.from == producer && c.from_port == port && !c.abandoned)
            .all(|c| {
                !c.closed
                    && (c.collect.is_some()
                        || self.processes[&c.to].inputs.can_accept(c.input, capacity)
                        || (!self.processes[&c.to].mix
                            && self.processes[&c.to].inputs.waiting_on_other(c.input)))
            })
    }

    /// True only when an output had subscribers and all have detached.
    /// An output that never had subscribers is not considered abandoned.
    pub(crate) fn output_abandoned(&self, producer: &str, port: u8) -> bool {
        let mut subscriptions = self
            .connections
            .iter()
            .filter(|c| c.from == producer && c.from_port == port)
            .peekable();
        subscriptions.peek().is_some() && subscriptions.all(|c| c.abandoned)
    }

    /// Deliver a single output item. Nothing is delivered for absent outputs.
    pub fn emit(&mut self, producer: &str, port: u8, value: T) -> Result<(), String> {
        for c in self
            .connections
            .iter_mut()
            .filter(|c| c.from == producer && c.from_port == port)
        {
            if c.abandoned {
                continue;
            }
            if c.closed {
                return Err(format!("output `{producer}:{port}` is closed"));
            }
            if let Some(collect) = &mut c.collect {
                if !(self.collect_empty)(&value) {
                    collect.send(value.clone()).map_err(|e| format!("{e:?}"))?;
                }
            } else {
                self.processes
                    .get_mut(&c.to)
                    .expect("validated target")
                    .inputs
                    .send(c.input, Delivery::Value(value.clone()))
                    .map_err(|e| format!("{e:?}"))?;
            }
        }
        Ok(())
    }

    fn close_output(&mut self, producer: &str) -> Result<(), String> {
        for c in self
            .connections
            .iter_mut()
            .filter(|c| c.from == producer && !c.closed && !c.abandoned)
        {
            let target = self.processes.get_mut(&c.to).expect("validated target");
            if let Some(collect) = &mut c.collect
                && let Some(values) = collect.close()
            {
                target
                    .inputs
                    .send(c.input, Delivery::Collected(values))
                    .map_err(|e| format!("{e:?}"))?;
            }
            target.inputs.close(c.input).map_err(|e| format!("{e:?}"))?;
            c.closed = true;
        }
        Ok(())
    }

    pub fn close_source(&mut self, name: &str) -> Result<(), String> {
        if !self.sources.contains(name) {
            return Err(format!("unknown source `{name}`"));
        }
        if self.closed_sources.insert(name.into()) {
            self.close_output(name)?;
        }
        Ok(())
    }

    /// Return one runnable tuple, propagating closure through processes that
    /// have no further possible firings and no tasks still in flight.
    pub fn next_activation(&mut self) -> Result<Option<Activation<T>>, String> {
        self.next_activation_with_capacity(None)
    }

    /// Do not consume another synchronous operator input while any of its
    /// output subscribers is full. This preserves backpressure across chains.
    pub(crate) fn next_activation_bounded(
        &mut self,
        capacity: usize,
    ) -> Result<Option<Activation<T>>, String> {
        self.next_activation_with_capacity(Some(capacity))
    }

    fn next_activation_with_capacity(
        &mut self,
        capacity: Option<usize>,
    ) -> Result<Option<Activation<T>>, String> {
        loop {
            let blocked: BTreeSet<String> = capacity
                .map(|capacity| {
                    self.processes
                        .iter()
                        .filter(|(name, state)| {
                            state.is_channel
                                && self
                                    .connections
                                    .iter()
                                    .filter(|c| &c.from == *name && !c.abandoned)
                                    .any(|c| !self.can_emit(name, c.from_port, capacity))
                        })
                        .map(|(name, _)| name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let orphaned: BTreeSet<String> = self
                .processes
                .iter()
                .filter_map(|(name, state)| {
                    if state.closed || !state.is_channel || state.active != 0 {
                        return None;
                    }
                    let mut outputs = self
                        .connections
                        .iter()
                        .filter(|connection| &connection.from == name)
                        .peekable();
                    (outputs.peek().is_some() && outputs.all(|connection| connection.abandoned))
                        .then(|| name.clone())
                })
                .collect();
            let mut close = None;
            for (name, p) in &mut self.processes {
                if orphaned.contains(name) {
                    p.closed = true;
                    close = Some(name.clone());
                    break;
                }
                if p.closed || blocked.contains(name) || (p.serial && p.active != 0) {
                    continue;
                }
                if p.mix || p.concat_order.is_some() {
                    let firing = match &p.concat_order {
                        Some(order) => p.inputs.next_ordered(order),
                        None => p.inputs.next_any(),
                    };
                    match firing {
                        AnyFiring::Ready(index, value) => {
                            p.active += 1;
                            return Ok(Some(Activation {
                                process: name.clone(),
                                inputs: vec![(p.ports[index], value)],
                                closing: false,
                            }));
                        }
                        AnyFiring::Closed if p.active == 0 && p.finalize && !p.finalized => {
                            p.finalized = true;
                            p.active += 1;
                            return Ok(Some(Activation {
                                process: name.clone(),
                                inputs: vec![],
                                closing: true,
                            }));
                        }
                        AnyFiring::Closed if p.active == 0 => {
                            p.closed = true;
                            close = Some(name.clone());
                            break;
                        }
                        AnyFiring::Pending | AnyFiring::Closed => {}
                    }
                    continue;
                }
                match p.inputs.next() {
                    Firing::Ready(values) => {
                        p.active += 1;
                        return Ok(Some(Activation {
                            process: name.clone(),
                            inputs: p.ports.iter().copied().zip(values).collect(),
                            closing: false,
                        }));
                    }
                    Firing::Closed if p.active == 0 => {
                        p.closed = true;
                        close = Some(name.clone());
                        break;
                    }
                    Firing::Pending | Firing::Closed => {}
                }
            }
            if let Some(name) = close {
                // Other producers can outlive this consumer. Detach only its
                // subscriptions, keeping sibling subscribers intact.
                for connection in self.connections.iter_mut().filter(|c| c.to == name) {
                    connection.abandoned = true;
                    connection.collect = None;
                }
                self.processes
                    .get_mut(&name)
                    .expect("known process")
                    .inputs
                    .discard();
                self.close_output(&name)?;
            } else {
                return Ok(None);
            }
        }
    }

    pub fn finish(&mut self, process: &str) -> Result<(), String> {
        let p = self
            .processes
            .get_mut(process)
            .ok_or_else(|| format!("unknown process `{process}`"))?;
        if p.active == 0 {
            return Err(format!("process `{process}` has no active task"));
        }
        p.active -= 1;
        Ok(())
    }

    /// End a synchronous operator early without affecting sibling subscribers.
    pub(crate) fn stop_process(&mut self, process: &str) -> Result<(), String> {
        let state = self
            .processes
            .get_mut(process)
            .ok_or_else(|| format!("unknown process `{process}`"))?;
        if state.active != 0 {
            return Err(format!("process `{process}` still has active tasks"));
        }
        if state.closed {
            return Ok(());
        }
        state.closed = true;
        state.inputs.discard();
        for connection in self.connections.iter_mut().filter(|c| c.to == process) {
            connection.abandoned = true;
            connection.collect = None;
        }
        self.close_output(process)
    }

    #[cfg(test)]
    pub(crate) fn buffered_for(&self, process: &str) -> usize {
        self.processes[process].inputs.buffered_len()
    }
    pub fn is_closed(&self) -> bool {
        self.processes.values().all(|p| p.closed)
    }
    pub fn output_kind(&self, process: &str) -> Option<ChannelKind> {
        self.processes.get(process).map(|p| p.output_kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::model::{FlowGraphBuilder, Process, Source, Wire};
    use serde_json::json;

    #[test]
    fn take_rejects_a_value_channel() {
        let graph = FlowGraphBuilder::default()
            .source(Source::value("s", json!(1)))
            .process(Process::new(
                "take",
                "channel",
                json!({"operator":"take", "count":1}),
            ))
            .wire(Wire::direct("s", 0, "take", 0))
            .build()
            .unwrap();
        assert!(
            Dataflow::<i32>::new(&graph)
                .err()
                .unwrap()
                .contains("value channel")
        );
    }

    #[test]
    fn reductions_wait_until_input_closes() {
        for operator in ["last", "count"] {
            let graph = FlowGraphBuilder::default()
                .source(Source::items("s", vec![]))
                .process(Process::new(
                    "reduce",
                    "channel",
                    json!({"operator":operator}),
                ))
                .wire(Wire::direct("s", 0, "reduce", 0))
                .build()
                .unwrap();
            let mut flow = Dataflow::<i32>::new(&graph).unwrap();
            flow.emit("s", 0, 7).unwrap();
            let activation = flow.next_activation().unwrap().unwrap();
            assert!(!activation.closing);
            flow.finish("reduce").unwrap();
            assert!(flow.next_activation().unwrap().is_none());
            flow.close_source("s").unwrap();
            assert!(flow.next_activation().unwrap().unwrap().closing);
        }
    }

    #[test]
    fn actual_outputs_trigger_downstream_and_close_after_inflight_finishes() {
        let g = FlowGraphBuilder::default()
            .source(Source::items("reads", vec![]))
            .process(Process::new("a", "x", json!({})))
            .process(Process::new("b", "x", json!({})))
            .wire(Wire::direct("reads", 0, "a", 0))
            .wire(Wire::direct("a", 0, "b", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::new(&g).unwrap();
        flow.emit("reads", 0, 1).unwrap();
        flow.emit("reads", 0, 2).unwrap();
        flow.close_source("reads").unwrap();
        assert_eq!(flow.next_activation().unwrap().unwrap().process, "a");
        assert_eq!(flow.next_activation().unwrap().unwrap().process, "a");
        assert!(flow.next_activation().unwrap().is_none());
        flow.emit("a", 0, 10).unwrap();
        assert_eq!(
            flow.next_activation().unwrap().unwrap().inputs,
            vec![(0, Delivery::Value(10))]
        );
        flow.finish("b").unwrap();
        flow.finish("a").unwrap();
        flow.finish("a").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        assert!(flow.is_closed());
    }

    #[test]
    fn collect_emits_only_when_source_closes_and_nothing_when_empty() {
        let g = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new("sink", "x", json!({})))
            .wire(Wire::collect("s", 0, "sink", 0))
            .build()
            .unwrap();
        let mut f = Dataflow::new(&g).unwrap();
        f.emit("s", 0, 1).unwrap();
        assert!(f.next_activation().unwrap().is_none());
        f.close_source("s").unwrap();
        assert_eq!(
            f.next_activation().unwrap().unwrap().inputs,
            vec![(0, Delivery::Collected(vec![1]))]
        );
        f.finish("sink").unwrap();
        f.next_activation().unwrap();
        assert!(f.is_closed());
        let mut empty = Dataflow::<i32>::new(&g).unwrap();
        empty.close_source("s").unwrap();
        assert!(empty.next_activation().unwrap().is_none());
        assert!(empty.is_closed());
    }

    #[test]
    fn two_queues_pair_and_value_process_broadcasts_to_each_firing() {
        // Deserialize rather than using the legacy builder, whose old
        // validation incorrectly rejects multiple queue inputs.
        let g: FlowGraph = serde_json::from_value(json!({
            "sources": [
                {"id":"a", "spec":{"type":"items", "values":[]}},
                {"id":"b", "spec":{"type":"items", "values":[]}}
            ],
            "processes": [
                {"id":"constant", "kind":"x", "spec":{}},
                {"id":"join", "kind":"x", "spec":{}}
            ],
            "wires": [
                {"from":"a", "from_port":0, "to":"join", "to_port":0, "op":"direct"},
                {"from":"b", "from_port":0, "to":"join", "to_port":1, "op":"direct"},
                {"from":"constant", "from_port":0, "to":"join", "to_port":2, "op":"direct"}
            ]
        }))
        .unwrap();
        let mut f = Dataflow::new(&g).unwrap();
        assert_eq!(f.next_activation().unwrap().unwrap().process, "constant");
        f.emit("a", 0, 1).unwrap();
        f.emit("a", 0, 2).unwrap();
        f.emit("b", 0, 3).unwrap();
        f.emit("b", 0, 4).unwrap();
        assert!(f.next_activation().unwrap().is_none());
        f.emit("constant", 0, 9).unwrap();
        f.finish("constant").unwrap();
        assert_eq!(
            f.next_activation().unwrap().unwrap().inputs,
            vec![
                (0, Delivery::Value(1)),
                (1, Delivery::Value(3)),
                (2, Delivery::Value(9))
            ]
        );
        assert_eq!(
            f.next_activation().unwrap().unwrap().inputs,
            vec![
                (0, Delivery::Value(2)),
                (1, Delivery::Value(4)),
                (2, Delivery::Value(9))
            ]
        );
        f.finish("join").unwrap();
        f.finish("join").unwrap();
        f.close_source("a").unwrap();
        f.close_source("b").unwrap();
        assert!(f.next_activation().unwrap().is_none());
        assert!(f.is_closed());
    }
    #[test]
    fn value_source_is_reused_for_every_queue_item() {
        let g = FlowGraphBuilder::default()
            .source(Source::items("items", vec![]))
            .source(Source::value("reference", json!("ref")))
            .process(Process::new("align", "x", json!({})))
            .wire(Wire::direct("items", 0, "align", 0))
            .wire(Wire::direct("reference", 0, "align", 1))
            .build()
            .unwrap();
        let mut flow = Dataflow::new(&g).unwrap();
        flow.emit("reference", 0, 9).unwrap();
        flow.close_source("reference").unwrap();
        flow.emit("items", 0, 1).unwrap();
        flow.emit("items", 0, 2).unwrap();
        flow.close_source("items").unwrap();
        assert_eq!(
            flow.next_activation().unwrap().unwrap().inputs,
            vec![(0, Delivery::Value(1)), (1, Delivery::Value(9))]
        );
        assert_eq!(
            flow.next_activation().unwrap().unwrap().inputs,
            vec![(0, Delivery::Value(2)), (1, Delivery::Value(9))]
        );
        flow.finish("align").unwrap();
        flow.finish("align").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        assert!(flow.is_closed());
    }
    #[test]
    fn abandoned_output_requires_all_subscribers_to_detach() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new(
                "one",
                "channel",
                json!({"operator":"take", "count":1}),
            ))
            .process(Process::new(
                "two",
                "channel",
                json!({"operator":"take", "count":1}),
            ))
            .wire(Wire::direct("s", 0, "one", 0))
            .wire(Wire::direct("s", 0, "two", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::<i32>::new(&graph).unwrap();
        assert!(!flow.output_abandoned("s", 0));
        assert!(!flow.output_abandoned("one", 0));
        flow.stop_process("one").unwrap();
        assert!(!flow.output_abandoned("s", 0));
        flow.stop_process("two").unwrap();
        assert!(flow.output_abandoned("s", 0));
    }

    #[test]
    fn abandoned_consumers_prune_channel_chain_only_after_last_branch() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new(
                "map",
                "channel",
                json!({"operator":"map", "template":"{{item}}"}),
            ))
            .process(Process::new(
                "one",
                "channel",
                json!({"operator":"take", "count":1}),
            ))
            .process(Process::new(
                "two",
                "channel",
                json!({"operator":"take", "count":1}),
            ))
            .wire(Wire::direct("s", 0, "map", 0))
            .wire(Wire::direct("map", 0, "one", 0))
            .wire(Wire::direct("map", 0, "two", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::<i32>::new(&graph).unwrap();
        flow.stop_process("one").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        assert!(!flow.output_abandoned("s", 0));
        flow.stop_process("two").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        assert!(flow.output_abandoned("s", 0));
        assert!(flow.is_closed());
    }

    #[test]
    fn synchronous_channel_waits_when_its_subscriber_is_full() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("src", vec![]))
            .process(Process::new(
                "a",
                "channel",
                json!({"operator":"if_empty", "value":0}),
            ))
            .process(Process::new(
                "z",
                "channel",
                json!({"operator":"if_empty", "value":0}),
            ))
            .wire(Wire::direct("src", 0, "a", 0))
            .wire(Wire::direct("a", 0, "z", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::new(&graph).unwrap();
        flow.emit("src", 0, 1).unwrap();
        flow.emit("src", 0, 2).unwrap();
        let first = flow.next_activation_bounded(1).unwrap().unwrap();
        assert_eq!(first.process, "a");
        flow.emit("a", 0, 1).unwrap();
        flow.finish("a").unwrap();
        let next = flow.next_activation_bounded(1).unwrap().unwrap();
        assert_eq!(next.process, "z");
        flow.finish("z").unwrap();
        let resumed = flow.next_activation_bounded(1).unwrap().unwrap();
        assert_eq!(resumed.process, "a");
    }

    #[test]
    fn concat_waits_for_earlier_port_to_close_even_when_wires_are_reversed() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("a", vec![]))
            .source(Source::items("b", vec![]))
            .source(Source::value("c", json!(3)))
            .process(Process::new(
                "concat",
                "channel",
                json!({"operator":"concat"}),
            ))
            .wire(Wire::direct("c", 0, "concat", 2))
            .wire(Wire::direct("b", 0, "concat", 1))
            .wire(Wire::direct("a", 0, "concat", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::new(&graph).unwrap();
        flow.emit("b", 0, 2).unwrap();
        flow.close_source("b").unwrap();
        flow.emit("c", 0, 3).unwrap();
        flow.close_source("c").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        flow.emit("a", 0, 1).unwrap();
        assert_eq!(
            flow.next_activation().unwrap().unwrap().inputs,
            vec![(0, Delivery::Value(1))]
        );
        flow.finish("concat").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        flow.close_source("a").unwrap();
        assert_eq!(
            flow.next_activation().unwrap().unwrap().inputs,
            vec![(1, Delivery::Value(2))]
        );
        flow.finish("concat").unwrap();
        assert_eq!(
            flow.next_activation().unwrap().unwrap().inputs,
            vec![(2, Delivery::Value(3))]
        );
        flow.finish("concat").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        assert!(flow.is_closed());
    }

    #[test]
    fn mix_consumes_each_input_independently_and_waits_for_all_to_close() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("a", vec![]))
            .source(Source::items("b", vec![]))
            .process(Process::new("mix", "channel", json!({"operator":"mix"})))
            .wire(Wire::direct("a", 0, "mix", 0))
            .wire(Wire::direct("b", 0, "mix", 1))
            .build()
            .unwrap();
        let mut flow = Dataflow::new(&graph).unwrap();
        flow.emit("a", 0, 1).unwrap();
        flow.close_source("a").unwrap();
        let first = flow.next_activation().unwrap().unwrap();
        assert_eq!(first.inputs, vec![(0, Delivery::Value(1))]);
        flow.finish("mix").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        assert!(!flow.is_closed());
        flow.emit("b", 0, 2).unwrap();
        assert_eq!(
            flow.next_activation().unwrap().unwrap().inputs,
            vec![(1, Delivery::Value(2))]
        );
        flow.finish("mix").unwrap();
        flow.close_source("b").unwrap();
        assert!(flow.next_activation().unwrap().is_none());
        assert!(flow.is_closed());
    }
    #[test]
    fn finished_consumer_drops_buffer_without_affecting_siblings() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("a", vec![]))
            .source(Source::items("b", vec![]))
            .process(Process::new("consumer", "x", json!({})))
            .process(Process::new("survivor", "x", json!({})))
            .wire(Wire::direct("a", 0, "consumer", 0))
            .wire(Wire::direct("b", 0, "consumer", 1))
            .wire(Wire::direct("b", 0, "survivor", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::new(&graph).unwrap();
        flow.emit("a", 0, 1).unwrap();
        flow.close_source("a").unwrap();
        for value in 0..20 {
            flow.emit("b", 0, value).unwrap();
        }
        assert_eq!(flow.next_activation().unwrap().unwrap().process, "consumer");
        flow.finish("consumer").unwrap();
        let survivor = flow.next_activation().unwrap().unwrap();
        assert_eq!(survivor.process, "survivor");
        assert_eq!(flow.processes["consumer"].inputs.buffered_len(), 0);
        for value in 20..40 {
            flow.emit("b", 0, value).unwrap();
        }
        assert_eq!(flow.processes["consumer"].inputs.buffered_len(), 0);
        flow.finish("survivor").unwrap();
        flow.close_source("b").unwrap();
        let mut count = 1;
        while let Some(job) = flow.next_activation().unwrap() {
            assert_eq!(job.process, "survivor");
            count += 1;
            flow.finish("survivor").unwrap();
        }
        assert_eq!(count, 40);
        assert!(flow.is_closed());
    }
    #[test]
    fn source_backpressure_waits_for_every_live_subscriber() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("s", vec![]))
            .process(Process::new("a", "x", json!({})))
            .process(Process::new("b", "x", json!({})))
            .wire(Wire::direct("s", 0, "a", 0))
            .wire(Wire::direct("s", 0, "b", 0))
            .build()
            .unwrap();
        let mut flow = Dataflow::new(&graph).unwrap();
        assert!(flow.can_emit("s", 0, 2));
        flow.emit("s", 0, 1).unwrap();
        flow.emit("s", 0, 2).unwrap();
        assert!(!flow.can_emit("s", 0, 2));
        assert_eq!(flow.next_activation().unwrap().unwrap().process, "a");
        flow.finish("a").unwrap();
        assert_eq!(flow.next_activation().unwrap().unwrap().process, "a");
        flow.finish("a").unwrap();
        assert!(!flow.can_emit("s", 0, 2));
        assert_eq!(flow.next_activation().unwrap().unwrap().process, "b");
        flow.finish("b").unwrap();
        assert!(flow.can_emit("s", 0, 2));
    }
    #[test]
    fn rejects_invalid_channel_options_before_sources_are_consumed() {
        let graph = FlowGraphBuilder::default()
            .source(Source::items("a", vec![]))
            .source(Source::items("b", vec![]))
            .process(Process::new(
                "join",
                "channel",
                json!({"operator":"join", "remainder":true, "failOnMismatch":true}),
            ))
            .wire(Wire::direct("a", 0, "join", 0))
            .wire(Wire::direct("b", 0, "join", 1))
            .build()
            .unwrap();
        assert!(
            Dataflow::<()>::new(&graph)
                .err()
                .unwrap()
                .contains("conflicts")
        );
    }
}
