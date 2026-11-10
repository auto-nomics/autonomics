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

    /// Canonicalize mounted addresses at the execution boundary, including
    /// nested channel artifacts. Commit only when every reference is valid.
    pub fn canonicalize_files(
        &mut self,
        storage: &vfs::OpendalFileStorage,
    ) -> Result<(), DagError> {
        use crate::dag::execution::ChannelTransportValue as T;
        fn file(value: &mut FileRef, storage: &vfs::OpendalFileStorage) -> Result<(), DagError> {
            *value = value.clone().canonicalized(storage)?;
            Ok(())
        }
        fn channel(value: &mut T, storage: &vfs::OpendalFileStorage) -> Result<(), DagError> {
            match value {
                T::File(value) | T::Arrow(value) => file(value, storage)?,
                T::List(values) | T::Tuple(values) => {
                    for value in values {
                        channel(value, storage)?;
                    }
                }
                T::Map(values) => {
                    for value in values.values_mut() {
                        channel(value, storage)?;
                    }
                }
                T::Json(_) => {}
            }
            Ok(())
        }
        let mut values = self.values.clone();
        for value in values.values_mut() {
            match value {
                NodeValue::File(value) => file(value, storage)?,
                NodeValue::FileSet(values) => {
                    for value in values {
                        file(value, storage)?;
                    }
                }
                NodeValue::Structured(value) => channel(value, storage)?,
                _ => {}
            }
        }
        self.values = values;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::PortOutputs;

    #[test]
    fn nested_file_outputs_are_upgraded_transactionally() {
        use crate::dag::execution::ChannelTransportValue as T;
        use crate::{FileRef, NodeValue};
        let storage = vfs::OpendalFileStorage::new_temp();
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, FileRef::new("vfs:///answer.tsv", None));
        outputs.insert(
            1,
            NodeValue::Structured(T::List(vec![T::File(FileRef::new(
                "vfs:///nested.tsv",
                None,
            ))])),
        );
        outputs.canonicalize_files(&storage).unwrap();
        assert_eq!(
            outputs.get(&0).unwrap().as_file().unwrap().path,
            "local://answer.tsv"
        );
        let NodeValue::Structured(T::List(values)) = outputs.get(&1).unwrap() else {
            panic!()
        };
        let T::File(file) = &values[0] else { panic!() };
        assert_eq!(file.path, "local://nested.tsv");
        outputs.insert_file(0, FileRef::new("vfs:///uncommitted.tsv", None));
        outputs.insert_file(2, FileRef::new("unknown://file.tsv", None));
        assert!(outputs.canonicalize_files(&storage).is_err());
        assert_eq!(
            outputs.get(&0).unwrap().as_file().unwrap().path,
            "vfs:///uncommitted.tsv"
        );
    }

    #[test]
    fn legacy_graph_path_refers_to_the_same_output_type() {
        let output: crate::dag::graph::PortOutputs = PortOutputs::new();
        assert!(output.is_empty());
    }
}
