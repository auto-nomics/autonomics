//! Per-port output values produced by a node execution.

use datafusion::common::HashMap;
use datafusion::prelude::DataFrame;

use crate::dag::error::DagError;
use crate::value::{FileRef, NodeValue};

/// Output values keyed by output port index.
#[derive(Debug, Clone, Default)]
pub struct PortOutputs {
    values: HashMap<u8, NodeValue>,
}

impl PortOutputs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<V: Into<NodeValue>>(&mut self, port: u8, value: V) -> Option<NodeValue> {
        self.values.insert(port, value.into())
    }

    pub fn insert_file(&mut self, port: u8, file: FileRef) -> Option<NodeValue> {
        self.values.insert(port, NodeValue::File(file))
    }

    pub fn get(&self, port: &u8) -> Option<&NodeValue> {
        self.values.get(port)
    }

    pub fn dataframe(&self, port: u8) -> std::result::Result<&DataFrame, DagError> {
        self.values
            .get(&port)
            .ok_or_else(|| DagError::Schedule(format!("output port {port} has no value")))?
            .as_dataframe()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u8, &NodeValue)> {
        self.values.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &NodeValue> {
        self.values.values()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl std::ops::Index<&u8> for PortOutputs {
    type Output = NodeValue;

    fn index(&self, index: &u8) -> &Self::Output {
        &self.values[index]
    }
}
