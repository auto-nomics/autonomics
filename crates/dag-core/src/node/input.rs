//! Runtime input envelope injected into node implementations.

use datafusion::prelude::DataFrame;

use super::id::PortId;
use crate::dag::DagError;
use crate::value::{DataRef, FileRef, NodeValue};

/// One upstream output injected into a node at execution time, one per
/// connected input port.
#[derive(Debug, Clone)]
pub struct NodeInput {
    /// The consuming node's input port index.
    pub port: PortId,
    pub data: NodeValue,
}

impl NodeInput {
    pub fn new_dataframe(port: PortId, df: DataFrame) -> Self {
        Self {
            port,
            data: NodeValue::DataFrame(df),
        }
    }

    pub fn file(port: PortId, file: FileRef) -> Self {
        Self {
            port,
            data: NodeValue::File(file),
        }
    }

    pub fn data(port: PortId, data: DataRef) -> Self {
        Self {
            port,
            data: NodeValue::Data(data),
        }
    }

    pub fn dataframe(&self) -> Result<&DataFrame, DagError> {
        self.data.as_dataframe()
    }

    pub fn dataframe_value(&self) -> Result<&DataFrame, DagError> {
        self.dataframe()
    }

    pub fn file_value(&self) -> Result<&FileRef, DagError> {
        self.data.as_file()
    }

    pub fn data_value(&self) -> Result<&DataRef, DagError> {
        self.data.as_data()
    }
}
