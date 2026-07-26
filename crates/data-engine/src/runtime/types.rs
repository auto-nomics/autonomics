use tokio::sync::oneshot;

use crate::dag::RunReport;
use crate::dag::graph::PortOutputs;
use crate::error::Result as EngineResult;
use schemars;

pub enum DataEngineCmd {
    AddNode {
        id: String,
        kind: String,
        spec: serde_json::Value,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    AddEdge {
        from: String,
        from_port: Option<u8>,
        to: String,
        to_port: Option<u8>,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    RunDag {
        reply: oneshot::Sender<EngineResult<RunReport>>,
    },
    GetOutput {
        id: String,
        reply: oneshot::Sender<EngineResult<Option<PortOutputs>>>,
    },
    RemoveNode {
        id: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    ViewDag {
        reply: oneshot::Sender<EngineResult<String>>,
    },
    ClearDag {
        reply: oneshot::Sender<EngineResult<()>>,
    },
    GetNodeSpec {
        kind: String,
        reply: oneshot::Sender<EngineResult<schemars::Schema>>,
    },
    ListNodeFactories {
        reply: oneshot::Sender<EngineResult<Vec<crate::node_registry::NodeInfo>>>,
    },
    UpdateNode {
        id: String,
        spec: serde_json::Value,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    GetNodePorts {
        kind: String,
        reply: oneshot::Sender<EngineResult<crate::nodes::meta::NodePorts>>,
    },
    GetNodeDoc {
        kind: String,
        reply: oneshot::Sender<EngineResult<String>>,
    },
}
