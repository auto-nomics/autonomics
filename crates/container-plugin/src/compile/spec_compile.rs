use std::collections::BTreeMap;

use crate::node_definition::NodeDefinition;
use nodes_io::container_command::ContainerCommandSpec;

use super::error::{Error, Result};
use crate::manifest::{ImageMetadata, PanelBinding};

pub fn compile_container_spec(
    node: &NodeDefinition,
    image: &ImageMetadata,
    panels: &[PanelBinding],
    values: &serde_json::Value,
) -> Result<ContainerCommandSpec> {
    // A non-object spec (array, string, null from a mangled DAG) must fail
    // with its own diagnosis instead of degrading into a chain of
    // MissingParam errors.
    let object = values.as_object().ok_or_else(|| Error::ParamsNotObject {
        kind: node.kind.clone(),
    })?;

    // The compiled schema is `additionalProperties: false`; this is the
    // same gate, enforced independently of the schema pipeline.
    for key in object.keys() {
        if !node.params.contains_key(key) {
            return Err(Error::UnknownParam {
                kind: node.kind.clone(),
                name: key.clone(),
            });
        }
    }

    let mut resolved: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    for (key, field) in &node.params {
        let resolved_field = object
            .get(key)
            .cloned()
            .or_else(|| field.default.clone())
            .ok_or_else(|| Error::MissingParam {
                kind: node.kind.clone(),
                name: key.clone(),
            })?;

        super::utils::check_bounds(&node.kind, key, field, &resolved_field)?;
        super::utils::check_type(&node.kind, key, field, &resolved_field)?;

        resolved.insert(key.clone(), resolved_field);
    }

    // Gates run over the complete map: targets may be satisfied (or not) by
    // defaults rather than submitted values.
    super::utils::check_requires(node, &resolved)?;

    let outputs = node
        .ports
        .outputs
        .iter()
        .map(
            |output| nodes_io::container_command::ContainerCommandOutputSpec {
                path: output.path.clone(),
                format: output.format.clone(),
            },
        )
        .collect();

    let panel_bundles = panels
        .iter()
        .map(
            |panel| nodes_io::container_command::ContainerPanelBundleSpec {
                panel_id: panel.bundle.to_string(),
                mount_path: panel.mount.clone(),
            },
        )
        .collect();

    let network = match node.resources.network {
        Some(container_runtime::ContainerNetwork::Isolated) | None => "isolated".into(),
        Some(container_runtime::ContainerNetwork::Egress) => "egress".into(),
    };

    let argv = super::render::render_argv(&node.command.argv, &resolved)?;
    let env = super::render::render_env(&node.command.env, &resolved)?;
    let files = super::render::render_files(&node.command.files, &resolved);
    let script = node
        .command
        .script
        .as_deref()
        .map(|text| super::render::render_script(text, &node.command.interpreter, &resolved))
        .transpose()?;

    let spec = ContainerCommandSpec {
        image: image.reference.as_str().to_string(),
        command: std::iter::once(node.command.interpreter.clone())
            .chain(argv)
            .collect(),
        script,
        files,
        env,
        outputs,
        workdir: None,
        artifact_prefix: node
            .artifact_prefix
            .clone()
            .unwrap_or_else(|| format!("/artifacts/{}", node.kind)),
        timeout_secs: node.timeout_secs,
        // Plugin-derived specs never populate `panels` (the legacy
        // direct-mount field with embedded digest + source); the manifest
        // pipeline resolves panels through the DataBundle directory via
        // `panel_bundles` only. The empty `panels` is intentional.
        // TODO: remove this field after pluginfy
        panels: Vec::new(),
        panel_bundles,
        network,
        read_only_rootfs: node.resources.read_only_rootfs,
        pull_policy: node.resources.pull_policy.unwrap_or_default(),
        cpus: node.resources.cpus,
        memory: node.resources.memory.clone(),
        pids_limit: node.resources.pids_limit,
        shm_size: node.resources.shm_size.clone(),
        gpus: node.resources.gpus.clone(),
        user: node.resources.user.clone(),
    };
    Ok(spec)
}

#[cfg(test)]
mod tests {}
