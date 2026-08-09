//! DAG source nodes that pull tabular data from the Crossref REST API and
//! emit it as DataFusion DataFrames.
//!
//! - [`CrossrefWorksNode`] (`source_crossref_works`) — search `/works` and
//!   emit a table of results for structured-data pipelines.
//!
//! These are zero-input / single-output source nodes. They reuse the
//! [`CrossrefClient`]; `DagNode::execute` is async on the engine's tokio
//! runtime.

pub mod works;
