use std::collections::BTreeMap;

use container_plugin::node_definition::{NodeDefinition, OutputSpec, ParamSpec, PortSpec};

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

    /// List the node's parameter specifications.
    pub fn list_params(&self) -> Result<BTreeMap<String, ParamSpec>> {
        Ok(self.definition()?.params)
    }

    /// Read one parameter specification by name.
    pub fn read_param(&self, name: &str) -> Result<Option<ParamSpec>> {
        Ok(self.definition()?.params.get(name).cloned())
    }

    /// Add a parameter specification. Parameter names are the stable CRUD key.
    pub fn create_param(
        &mut self,
        name: impl Into<String>,
        spec: ParamSpec,
    ) -> Result<NodeDefinition> {
        let name = name.into();
        if self.definition()?.params.contains_key(&name) {
            return Err(Error::Validation(format!(
                "node `{}` param `{name}` already exists",
                self.kind
            )));
        }
        self.update_with(|node| {
            node.params.insert(name, spec);
        })
    }

    /// Replace an existing parameter specification.
    pub fn update_param(&mut self, name: &str, spec: ParamSpec) -> Result<NodeDefinition> {
        if !self.definition()?.params.contains_key(name) {
            return Err(Error::Validation(format!(
                "node `{}` param `{name}` does not exist",
                self.kind
            )));
        }
        self.update_with(|node| {
            node.params.insert(name.to_string(), spec);
        })
    }

    /// Delete a parameter specification.
    pub fn delete_param(&mut self, name: &str) -> Result<NodeDefinition> {
        if !self.definition()?.params.contains_key(name) {
            return Err(Error::Validation(format!(
                "node `{}` param `{name}` does not exist",
                self.kind
            )));
        }
        self.update_with(|node| {
            node.params.remove(name);
        })
    }

    /// List the node's declared input ports.
    pub fn list_input_ports(&self) -> Result<Vec<PortSpec>> {
        Ok(self.definition()?.ports.inputs)
    }

    /// Read one input port by its declaration index.
    pub fn read_input_port(&self, index: usize) -> Result<PortSpec> {
        self.definition()?
            .ports
            .inputs
            .get(index)
            .cloned()
            .ok_or_else(|| {
                Error::Validation(format!(
                    "node `{}` input port index {index} does not exist",
                    self.kind
                ))
            })
    }

    /// Append an input port.
    pub fn create_input_port(&mut self, port: PortSpec) -> Result<NodeDefinition> {
        self.update_with(|node| node.ports.inputs.push(port))
    }

    /// Replace an input port by its declaration index.
    pub fn update_input_port(&mut self, index: usize, port: PortSpec) -> Result<NodeDefinition> {
        if index >= self.definition()?.ports.inputs.len() {
            return Err(Error::Validation(format!(
                "node `{}` input port index {index} does not exist",
                self.kind
            )));
        }
        self.update_with(|node| node.ports.inputs[index] = port)
    }

    /// Delete an input port by its declaration index.
    pub fn delete_input_port(&mut self, index: usize) -> Result<NodeDefinition> {
        if index >= self.definition()?.ports.inputs.len() {
            return Err(Error::Validation(format!(
                "node `{}` input port index {index} does not exist",
                self.kind
            )));
        }
        self.update_with(|node| {
            node.ports.inputs.remove(index);
        })
    }

    /// List the node's declared output ports.
    pub fn list_output_ports(&self) -> Result<Vec<OutputSpec>> {
        Ok(self.definition()?.ports.outputs)
    }

    /// Read one output port by its output path.
    pub fn read_output_port(&self, path: &str) -> Result<OutputSpec> {
        self.definition()?
            .ports
            .outputs
            .iter()
            .find(|output| output.path == path)
            .cloned()
            .ok_or_else(|| {
                Error::Validation(format!(
                    "node `{}` output port path `{path}` does not exist",
                    self.kind
                ))
            })
    }

    /// Append an output port. Output paths are the stable CRUD key.
    pub fn create_output_port(&mut self, port: OutputSpec) -> Result<NodeDefinition> {
        if self
            .definition()?
            .ports
            .outputs
            .iter()
            .any(|output| output.path == port.path)
        {
            return Err(Error::Validation(format!(
                "node `{}` output port path `{}` already exists",
                self.kind, port.path
            )));
        }
        self.update_with(|node| node.ports.outputs.push(port))
    }

    /// Replace an existing output port. The replacement may change its path.
    pub fn update_output_port(&mut self, path: &str, port: OutputSpec) -> Result<NodeDefinition> {
        let node = self.definition()?;
        let index = node
            .ports
            .outputs
            .iter()
            .position(|output| output.path == path)
            .ok_or_else(|| {
                Error::Validation(format!(
                    "node `{}` output port path `{path}` does not exist",
                    self.kind
                ))
            })?;
        if node
            .ports
            .outputs
            .iter()
            .enumerate()
            .any(|(other_index, output)| other_index != index && output.path == port.path)
        {
            return Err(Error::Validation(format!(
                "node `{}` output port path `{}` already exists",
                self.kind, port.path
            )));
        }
        self.update_with(|node| node.ports.outputs[index] = port)
    }

    /// Delete an output port by its output path.
    pub fn delete_output_port(&mut self, path: &str) -> Result<NodeDefinition> {
        let index = self
            .definition()?
            .ports
            .outputs
            .iter()
            .position(|output| output.path == path)
            .ok_or_else(|| {
                Error::Validation(format!(
                    "node `{}` output port path `{path}` does not exist",
                    self.kind
                ))
            })?;
        self.update_with(|node| {
            node.ports.outputs.remove(index);
        })
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
