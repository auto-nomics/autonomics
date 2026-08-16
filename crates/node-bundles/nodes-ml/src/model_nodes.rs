//! Model artifact DAG nodes — save/load fitted models.
//!
//! Uses local filesystem for now; opendal integration planned for the
//! full model-artifact framework.

use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

mod model_save;
pub use model_save::ModelSaveFactory;

mod model_load;
pub use model_load::ModelLoadFactory;
