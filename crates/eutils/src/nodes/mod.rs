//! DAG source nodes for E-utilities endpoints used by GEO.

pub mod geo;

pub(crate) mod util {
    use std::sync::Arc;

    use arrow_array::{Array, RecordBatch, StringArray, UInt64Array};
    use arrow_schema::{DataType, Field};
    use dag_core::dag::{DagError, NodePorts, graph::PortOutputs};
    use dag_core::registry::NodeCtx;

    pub fn dataframe_port() -> NodePorts {
        NodePorts::new().add_output_port(None)
    }

    pub fn utf8(name: &str) -> Field {
        Field::new(name, DataType::Utf8, true)
    }

    pub fn uint64(name: &str) -> Field {
        Field::new(name, DataType::UInt64, true)
    }

    pub fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
        let refs: Vec<Option<&str>> = rows.iter().map(|row| row.as_deref()).collect();
        Arc::new(StringArray::from(refs))
    }

    pub fn u64_array(rows: Vec<Option<u64>>) -> Arc<dyn Array> {
        Arc::new(UInt64Array::from(rows))
    }

    pub fn dataframe_output(
        ctx: &NodeCtx,
        batch: RecordBatch,
        kind: &str,
    ) -> Result<PortOutputs, DagError> {
        let dataframe = ctx.session().read_batch(batch).map_err(|error| {
            DagError::Schedule(format!("failed to read {kind} output batch: {error}"))
        })?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }

    pub fn request_error(error: crate::EutilsError, kind: &str) -> DagError {
        DagError::Schedule(format!("{kind} request failed: {error}"))
    }

    pub fn schedule_error(kind: &str, message: impl std::fmt::Display) -> DagError {
        DagError::Schedule(format!("{kind}: {message}"))
    }

    pub fn parse_u64(value: &serde_json::Value) -> Option<u64> {
        value.as_u64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
    }

    pub fn value_string(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::String(text) => Some(text.clone()),
            serde_json::Value::Number(number) => Some(number.to_string()),
            serde_json::Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        }
    }

    pub fn json_string(value: &serde_json::Value) -> Option<String> {
        serde_json::to_string(value).ok()
    }
}
