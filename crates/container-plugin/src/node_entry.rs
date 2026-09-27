use dag_core::NodePorts;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct NodeEntry {
    pub kind: String,
    pub ports: NodePorts,
}
