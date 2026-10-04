use container_plugin::node_definition::NodeDefinition;

use crate::{Error, PluginDevelopment, Proposal, Result};

/// A development handle for one node in a plugin manifest.
///
/// Manifest persistence and proposal metadata remain owned by
/// [`PluginDevelopment`], while this type exposes node-specific operations.
pub struct NodeDevelopment<'a, 'b> {
    plugin: &'a mut PluginDevelopment<'b>,
    kind: String,
}

impl<'a, 'b> NodeDevelopment<'a, 'b> {
    pub(crate) fn new(plugin: &'a mut PluginDevelopment<'b>, kind: impl Into<String>) -> Self {
        Self {
            plugin,
            kind: kind.into(),
        }
    }

    /// Return the node's runtime kind.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Read the current persisted node definition.
    pub fn definition(&self) -> Result<NodeDefinition> {
        self.plugin
            .read_node(&self.kind)?
            .ok_or_else(|| Error::Validation(format!("node `{}` does not exist", self.kind)))
    }

    /// Replace the node definition. The kind cannot change in place.
    pub fn update(&mut self, node: NodeDefinition) -> Result<NodeDefinition> {
        if node.kind != self.kind {
            return Err(Error::Validation(format!(
                "node kind cannot change from `{}` to `{}`; delete and create a new node",
                self.kind, node.kind
            )));
        }
        self.plugin.update_node(node)
    }

    /// Mutate the current node definition through a closure.
    pub fn update_with<F>(&mut self, mutate: F) -> Result<NodeDefinition>
    where
        F: FnOnce(&mut NodeDefinition),
    {
        let mut node = self.definition()?;
        mutate(&mut node);
        self.update(node)
    }

    /// Write the adapter referenced by this node's `script_file`.
    pub fn write_script(&mut self, contents: &str) -> Result<()> {
        let node = self.definition()?;
        let script = node
            .command
            .script_file
            .ok_or_else(|| Error::Validation("node has no script_file".into()))?;
        self.plugin.workspace().write_text(&script, contents)
    }

    /// Delete this node from the plugin manifest.
    pub fn delete(self) -> Result<Proposal> {
        self.plugin.delete_node(&self.kind)
    }
}
