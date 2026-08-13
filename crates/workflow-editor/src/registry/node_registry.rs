//! `NodeRegistry` — thread-safe map of `kind -> NodeFactory`.

use crate::error::{ApiError, Result};
use crate::model::NodeKindInfo;
use crate::registry::node_factory::NodeFactory;
use schemars::Schema;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// In-memory registry of node factories.
#[derive(Clone, Default)]
pub struct NodeRegistry {
    by_kind: Arc<RwLock<HashMap<&'static str, Arc<dyn NodeFactory>>>>,
}

impl NodeRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a factory. Overwrites any existing entry for the same kind.
    pub fn register(&self, factory: Arc<dyn NodeFactory>) {
        let kind = factory.kind();
        self.by_kind.write().unwrap().insert(kind, factory);
    }

    /// Fetch a factory by kind.
    pub fn get(&self, kind: &str) -> Option<Arc<dyn NodeFactory>> {
        self.by_kind.read().unwrap().get(kind).cloned()
    }

    /// List all registered kinds, sorted by category then label.
    pub fn list(&self) -> Vec<NodeKindInfo> {
        let mut out: Vec<NodeKindInfo> = self
            .by_kind
            .read()
            .unwrap()
            .values()
            .map(|f| NodeKindInfo {
                kind: f.kind().into(),
                label: f.label().into(),
                description: f.description().into(),
                category: f.category().into(),
                in_use: 0,
            })
            .collect();
        out.sort_by(|a, b| a.category.cmp(&b.category).then(a.label.cmp(&b.label)));
        out
    }

    /// Look up the JSON Schema for one kind.
    pub fn spec_schema(&self, kind: &str) -> Result<Schema> {
        self.get(kind)
            .map(|f| f.spec_schema())
            .ok_or_else(|| ApiError::InvalidCommand(format!("unknown node kind: {kind}")).into())
    }
}
