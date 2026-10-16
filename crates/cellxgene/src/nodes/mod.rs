//! CZ CELLxGENE Discover query DAG source nodes.

mod discover;

pub use discover::*;

pub(crate) mod util {
    use std::sync::Arc;

    use arrow_array::{Array, Float64Array, RecordBatch, StringArray, UInt64Array};
    use arrow_schema::{DataType, Field};
    use dag_core::dag::{DagError, NodePorts, graph::PortOutputs};
    use dag_core::registry::NodeCtx;

    pub fn dataframe_port() -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    pub fn utf8(name: &str) -> Field {
        Field::new(name, DataType::Utf8, true)
    }

    pub fn u64(name: &str) -> Field {
        Field::new(name, DataType::UInt64, true)
    }

    pub fn f64(name: &str) -> Field {
        Field::new(name, DataType::Float64, true)
    }

    pub fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
        let refs: Vec<Option<&str>> = rows.iter().map(|row| row.as_deref()).collect();
        Arc::new(StringArray::from(refs))
    }

    pub fn u64_array(rows: Vec<Option<u64>>) -> Arc<dyn Array> {
        Arc::new(UInt64Array::from(rows))
    }

    pub fn f64_array(rows: Vec<Option<f64>>) -> Arc<dyn Array> {
        Arc::new(Float64Array::from(rows))
    }

    pub fn schedule_error(kind: &str, error: impl std::fmt::Display) -> DagError {
        DagError::Schedule(format!("{kind}: {error}"))
    }

    pub fn request_error(error: crate::CellxgeneError, kind: &str) -> DagError {
        DagError::Schedule(format!("{kind} request failed: {error}"))
    }

    pub fn dataframe_output(
        ctx: &NodeCtx,
        batch: RecordBatch,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|error| schedule_error(kind, format!("failed to read batch: {error}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}
