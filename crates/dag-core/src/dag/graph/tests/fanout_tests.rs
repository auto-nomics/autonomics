//! Dynamic fanout: empty channels, job expansion + collection, manifest
//! round-trip restoration, and incremental reuse of an unchanged channel.

use std::sync::Arc;

use super::common::*;
use crate::dag::graph::{DAG, Result};
use crate::dag::node_event::{NodeEvent, NodeEventKind};
use crate::dag::runtime::{RuntimeStatus, SchedulerConfig};
use crate::dag::{ChannelOperator, DagNode, NodePorts, TaskInputSource, TaskResources};
use crate::value::{NodeValue, PortType};



#[derive(Clone)]
struct ResumableDynamicItemNode {
    value: String,
    attempts: Arc<std::sync::atomic::AtomicUsize>,
    failures: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    ports: NodePorts,
}

#[async_trait::async_trait]
impl DagNode for ResumableDynamicItemNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[crate::dag::NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> std::result::Result<crate::dag::graph::PortOutputs, crate::dag::error::DagError> {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self
            .failures
            .lock()
            .expect("failure set mutex")
            .contains(&self.value)
        {
            return Err(crate::dag::error::DagError::Schedule(format!(
                "intentional failure for `{}`",
                self.value
            )));
        }

        let mut outputs = crate::dag::graph::PortOutputs::new();
        outputs.insert(
            0,
            crate::value::FileRef::new(format!("/{}.txt", self.value), Some("txt".into())),
        );
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "resumable_dynamic_item"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}






