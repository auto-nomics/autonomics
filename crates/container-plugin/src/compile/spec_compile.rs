use std::collections::BTreeMap;

use crate::node_definition::NodeDefinition;
use crate::node_definition::ParamType;
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
            .or_else(|| field.optional.then_some(serde_json::Value::Null))
            .ok_or_else(|| Error::MissingParam {
                kind: node.kind.clone(),
                name: key.clone(),
            })?;

        if resolved_field.is_null() {
            // Optional-and-absent: no type or bounds apply. Renders as an
            // empty string on every surface.
            resolved.insert(key.clone(), resolved_field);
            continue;
        }

        super::utils::check_bounds(&node.kind, key, field, &resolved_field)?;
        super::utils::check_type(&node.kind, key, field, &resolved_field)?;

        resolved.insert(key.clone(), resolved_field);
    }

    // Gates run over the complete map: targets may be satisfied (or not) by
    // defaults rather than submitted values.
    super::utils::check_requires(node, &resolved)?;

    // Presence mapping for flag params (F02): a flag with a resolved
    // `false` renders as empty — identical to an optional-and-absent
    // param — so `[ -n "$VAR" ]` consumers see "unset". The mapping
    // happens here, one step upstream of the renderer, which stays
    // type-blind with pure value semantics (`Bool(false)` renders
    // "false"). `bool` params never enter this branch.
    let mut renderable = resolved.clone();
    for (name, spec) in &node.params {
        if spec.r#type == ParamType::Flag
            && renderable.get(name) == Some(&serde_json::Value::Bool(false))
        {
            renderable.insert(name.clone(), serde_json::Value::Null);
        }
    }

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

    let argv = super::render::render_argv(&node.command.argv, &renderable)?;
    let env = super::render::render_env(&node.command.env, &renderable)?;
    let files = super::render::render_files(&node.command.files, &renderable);
    let script = node
        .command
        .script
        .as_deref()
        .map(|text| super::render::render_script(text, &node.command.interpreter, &renderable))
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
mod tests {
    use super::*;
    use crate::manifest::ImageMetadata;
    use serde_json::json;

    const NODE_TOML: &str = r#"
kind = "render_kind"
desc = "Rendering contract"
doc = "Rendering contract"
timeout_secs = 60

[ports]
outputs = [{ path = "out.txt" }]

[params]
force = { type = "bool", default = false, doc = "Value semantics" }
verbose = { type = "flag", default = false, doc = "Presence semantics" }
label = { type = "string", optional = true }

[command]
interpreter = "sh"
script = """
run --force "$RENDER_FORCE" $RENDER_EXTRA
"""

[command.env]
RENDER_FORCE = "{{ force }}"
RENDER_VERBOSE = "{{ verbose }}"
RENDER_LABEL = "{{ label }}"
"#;

    fn compiled(values: serde_json::Value) -> ContainerCommandSpec {
        let node: NodeDefinition = toml::from_str(NODE_TOML).unwrap();
        crate::node_definition::validate(&node).unwrap();
        compile_container_spec(&node, &ImageMetadata::default(), &[], &values).unwrap()
    }

    #[test]
    fn bool_renders_value_semantics_end_to_end() {
        // A defaulted false bool renders the literal through the whole
        // manifest → resolve → presence-map → render pipeline (F02).
        let spec = compiled(json!({}));
        assert_eq!(spec.env.get("RENDER_FORCE").unwrap(), "false");
        let spec = compiled(json!({ "force": true }));
        assert_eq!(spec.env.get("RENDER_FORCE").unwrap(), "true");
    }

    #[test]
    fn flag_renders_presence_semantics_end_to_end() {
        // Flag false — defaulted or submitted — renders empty, matching
        // the optional-and-absent channel; true renders "true".
        let spec = compiled(json!({}));
        assert_eq!(spec.env.get("RENDER_VERBOSE").unwrap(), "");
        assert_eq!(spec.env.get("RENDER_LABEL").unwrap(), "");

        let spec = compiled(json!({ "verbose": false }));
        assert_eq!(spec.env.get("RENDER_VERBOSE").unwrap(), "");

        let spec = compiled(json!({ "verbose": true }));
        assert_eq!(spec.env.get("RENDER_VERBOSE").unwrap(), "true");
    }

    #[test]
    fn flag_enforces_the_boolean_shape() {
        let node: NodeDefinition = toml::from_str(NODE_TOML).unwrap();
        let error = compile_container_spec(
            &node,
            &ImageMetadata::default(),
            &[],
            &json!({ "verbose": "yes" }),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("render_kind"), "{message}");
        assert!(message.contains("boolean flag"), "{message}");
    }
}
