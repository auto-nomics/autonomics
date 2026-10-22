//! Deterministic manifest-to-container-spec compilation.

use container_plugin::{
    compile::{compile_schema, spec_compile::compile_container_spec},
    manifest::PluginManifest,
    node_definition::{self, NodeDefinition},
};
use serde_json::{Map, Value};

/// Compiles every node using its default parameter map.
///
/// This mirrors the manifest-editing tools' dry run and the deterministic
/// `registry_compile` gate.
pub(crate) struct RegistryCompiler<'a> {
    manifest: &'a PluginManifest,
}

impl<'a> RegistryCompiler<'a> {
    pub(crate) fn new(manifest: &'a PluginManifest) -> Self {
        Self { manifest }
    }

    pub(crate) fn compile(&self) -> Result<(), String> {
        for node in &self.manifest.nodes {
            self.compile_node(node)?;
        }
        Ok(())
    }

    fn compile_node(&self, node: &NodeDefinition) -> Result<(), String> {
        let _schema = compile_schema(&node.params);
        let ports = node_definition::compile_ports(&node.ports);
        if ports.input_ports().len() != node.ports.inputs.len()
            || ports.output_ports().len() != node.ports.outputs.len()
        {
            return Err("compiled port count does not match manifest".into());
        }

        let values = self.default_values(node)?;
        compile_container_spec(
            node,
            &self.manifest.image,
            &self.manifest.panels,
            &Value::Object(values),
        )
        .map_err(|error| format!("node `{}`: {error}", node.kind))?;
        Ok(())
    }

    fn default_values(&self, node: &NodeDefinition) -> Result<Map<String, Value>, String> {
        let mut values = Map::new();
        for (name, param) in &node.params {
            let value = param
                .default
                .clone()
                .or_else(|| param.optional.then_some(Value::Null))
                .ok_or_else(|| format!("required MVP parameter `{name}` must provide a default"))?;
            values.insert(name.clone(), value);
        }
        Ok(values)
    }
}

/// Shared by manifest-editing tools so candidate manifests fail before write.
pub(crate) fn registry_compile(manifest: &PluginManifest) -> Result<(), String> {
    RegistryCompiler::new(manifest).compile()
}
