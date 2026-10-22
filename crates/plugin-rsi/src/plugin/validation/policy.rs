//! Greenfield plugin release policy.

use std::path::Path;

use container_runtime::ContainerNetwork;

use crate::{
    environment::catalog::{Environment, EnvironmentCatalog},
    foundation::workspace::PluginWorkspace,
};

use container_plugin::manifest::PluginManifest;

/// Applies the host-owned policy that surrounds an otherwise valid manifest.
pub(crate) struct PolicyValidator<'a> {
    plugin_name: &'a str,
    workspace: &'a PluginWorkspace,
    manifest: &'a PluginManifest,
    catalog: &'a EnvironmentCatalog,
}

impl<'a> PolicyValidator<'a> {
    pub(crate) fn new(
        plugin_name: &'a str,
        workspace: &'a PluginWorkspace,
        manifest: &'a PluginManifest,
        catalog: &'a EnvironmentCatalog,
    ) -> Self {
        Self {
            plugin_name,
            workspace,
            manifest,
            catalog,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        self.validate_identity()?;
        self.validate_panel_policy()?;
        self.validate_workspace_files()?;
        self.validate_nodes()?;
        self.validate_readme()
    }

    fn validate_identity(&self) -> Result<(), String> {
        if self.manifest.nodes.is_empty() {
            return Err("manifest must declare at least one node".into());
        }
        if self.manifest.plugin_name != self.plugin_name {
            return Err(format!(
                "manifest plugin_name `{}` does not match workspace `{}`",
                self.manifest.plugin_name, self.plugin_name
            ));
        }
        Ok(())
    }

    /// Panel policy is intentionally isolated here so release-class decisions
    /// and issue diagnostics point at one explicit rule.
    /// NOTE: leave empty for future policy validation.
    fn validate_panel_policy(&self) -> Result<(), String> {
        Ok(())
    }

    fn validate_workspace_files(&self) -> Result<(), String> {
        let files = self
            .workspace
            .list_files()
            .map_err(|error| error.to_string())?;
        if files.iter().any(|path| {
            Path::new(path)
                .file_name()
                .is_some_and(|name| name == "Dockerfile")
        }) {
            return Err(
                "ordinary plugins cannot contain a Dockerfile; use an approved environment".into(),
            );
        }
        Ok(())
    }

    fn validate_nodes(&self) -> Result<(), String> {
        let reference = self.manifest.image.reference.as_str();
        let Some(environment_id) = self.catalog.find_reference(&reference) else {
            return Err(format!("environment `{reference}` is not approved for RSI"));
        };
        let Some(environment) = self.catalog.get(environment_id) else {
            return Err(format!("environment `{environment_id}` is not approved"));
        };

        for node in &self.manifest.nodes {
            self.validate_node(node, environment_id, environment)?;
        }
        Ok(())
    }

    fn validate_node(
        &self,
        node: &container_plugin::node_definition::NodeDefinition,
        environment_id: &str,
        environment: &Environment,
    ) -> Result<(), String> {
        if !environment
            .interpreters
            .iter()
            .any(|item| item == &node.command.interpreter)
        {
            return Err(format!(
                "interpreter `{}` is not approved for environment `{}`",
                node.command.interpreter, environment_id
            ));
        }
        if !matches!(
            node.resources.network,
            None | Some(ContainerNetwork::Isolated)
        ) {
            return Err("network egress requires a later reviewed release class".into());
        }
        if !node.resources.read_only_rootfs {
            return Err("read_only_rootfs cannot be disabled in the MVP".into());
        }
        if node.resources.gpus.is_some() || node.resources.user.is_some() {
            return Err("GPU and user overrides are not allowed in the MVP".into());
        }
        Ok(())
    }

    fn validate_readme(&self) -> Result<(), String> {
        let readme = self
            .workspace
            .read_text("README.md")
            .map_err(|error| format!("README.md is required: {error}"))?;
        if readme.contains("TODO") {
            return Err("TODO placeholders are not reviewable".into());
        }
        Ok(())
    }
}
