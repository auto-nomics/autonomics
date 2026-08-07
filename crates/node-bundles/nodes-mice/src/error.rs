//! Error type shared by MICE nodes.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MiceNodeError {
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("column '{name}' is not numeric (got type: {dtype})")]
    NonNumericColumn { name: String, dtype: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
    #[error("mice failed: {0}")]
    Mice(String),
    #[error("collect failed: {0}")]
    Collect(String),
    #[error("read_batch failed: {0}")]
    ReadBatch(String),
    #[error("invalid specification: {0}")]
    InvalidSpec(String),
    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("DataFusion error: {0}")]
    DataFusion(#[from] datafusion::error::DataFusionError),
}

impl ::dag_core::dag::NodeError for MiceNodeError {
    fn node_type(&self) -> &str {
        "mice"
    }
}
