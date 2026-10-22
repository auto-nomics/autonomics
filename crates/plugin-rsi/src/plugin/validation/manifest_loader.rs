//! Manifest parsing and script inlining for plugin validation.

use container_plugin::{
    manifest::PluginManifest,
    node_definition::{self, NodeDefinition},
};

use crate::foundation::workspace::PluginWorkspace;

/// Reads and normalizes the exact manifest used by every later gate.
///
/// `script_file` is deliberately inlined here so static review and spec
/// compilation always inspect the executable text that will run.
pub(crate) struct ManifestLoader<'a> {
    workspace: &'a PluginWorkspace,
}

impl<'a> ManifestLoader<'a> {
    pub(crate) fn new(workspace: &'a PluginWorkspace) -> Self {
        Self { workspace }
    }

    pub(crate) fn load(&self) -> Result<PluginManifest, Vec<String>> {
        let text = self
            .workspace
            .read_text("manifest.toml")
            .map_err(|error| vec![error.to_string()])?;
        let mut manifest: PluginManifest =
            toml::from_str(&text).map_err(|error| vec![error.to_string()])?;

        self.ensure_nodes(&manifest)?;
        self.inline_scripts(&mut manifest)?;

        for node in &manifest.nodes {
            node_definition::validate(node).map_err(|error| vec![error])?;
        }
        Ok(manifest)
    }

    fn ensure_nodes(&self, manifest: &PluginManifest) -> Result<(), Vec<String>> {
        if manifest.nodes.is_empty() {
            return Err(vec!["manifest has no nodes".into()]);
        }
        Ok(())
    }

    fn inline_scripts(&self, manifest: &mut PluginManifest) -> Result<(), Vec<String>> {
        for node in manifest.nodes.iter_mut() {
            self.inline_script(node)?;
        }
        Ok(())
    }

    fn inline_script(&self, node: &mut NodeDefinition) -> Result<(), Vec<String>> {
        match (&node.command.script, &node.command.script_file) {
            (Some(_), Some(_)) => Err(vec!["script and script_file are both set".into()]),
            (None, Some(relative)) => {
                let script = self
                    .workspace
                    .read_text(relative)
                    .map_err(|error| vec![error.to_string()])?;
                node.command.script = Some(script);
                node.command.script_file = None;
                Ok(())
            }
            (None, None) => Err(vec!["command must declare script_file".into()]),
            (Some(_), None) => Err(vec!["RSI plugins must use script_file".into()]),
        }
    }
}
