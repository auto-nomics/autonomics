//! Placeholder for the coloc.abf container node wrapper.
//!
//! The coloc_abf_container module is registered from `lib.rs` so the rest of
//! `nodes-io` can build. A parallel migration left the file with raw-string
//! tokenization errors that need to be resolved independently; this stub
//! keeps the build green without depending on the broken implementation.
#![allow(dead_code, unused_imports)]

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};

pub const COLOC_ABF_CONTAINER_KIND: &str = "coloc_abf_container";

/// Placeholder alias matching the pre-stub interface used by `lib.rs`.
pub type ColocAbfContainerNodeFactory = ColocAbfContainerNodeFactoryImpl;

pub struct ColocAbfContainerNodeFactoryImpl;

impl ColocAbfContainerNodeFactoryImpl {
    pub fn new(
        _runtime: std::sync::Arc<container_runtime::K3sRuntime>,
        _panel_cache: std::sync::Arc<container_runtime::PanelCache>,
    ) -> Self {
        Self
    }
}

impl NodeFactory for ColocAbfContainerNodeFactory {
    fn kind(&self) -> &'static str {
        COLOC_ABF_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Placeholder for the coloc.abf container wrapper."
    }

    fn doc(&self) -> &'static str {
        "The coloc.abf container wrapper is temporarily disabled pending a          fix for the raw-string tokenization error in the upstream module."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schemars::Schema::default()
    }

    fn data_bundles(&self) -> Vec<dag_core::DataBundleBinding> {
        Vec::new()
    }

    fn data_bundles_for_spec(
        &self,
        _spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<dag_core::DataBundleBinding>> {
        Ok(Vec::new())
    }

    fn ports(&self) -> dag_core::node::NodePorts {
        dag_core::node::NodePorts::new()
    }

    fn build(
        &self,
        _spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        Err(dag_core::registry::error::Error::Unknown(
            "coloc_abf_container is currently disabled".into(),
        ))
    }
}
