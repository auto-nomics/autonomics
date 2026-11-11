### Plugin Development
You can develop plugins through their `local://plugins/dev/<plugin-name>` VFS paths.
List approved environments with `plugin_environments_list`, inspect the addressed
workspace with `vfs`, create greenfield workspaces with `plugin_create`, fork
installed references with `plugin_fork`, bind approved environment images with
`plugin_environment_bind`, add nodes with
`plugin_node_create` (standalone NodeDefinition TOML), edit manifest.toml via
old_string/new_string replacement with `plugin_node_update` (read the file
through the VFS first; daemon-owned fields and the node set must not change,
and every edit is validated before it lands), update node docs with
`plugin_node_update_doc`, and run commands in the selected environment
with `plugin_container_run`. After editing, call `plugin_install` to validate and
load the local snapshot into the DAG. Call `plugin_uninstall` to remove the
active runtime source while retaining development history. Treat manifest
lifecycle state as host-owned: do not attempt direct manifest writes.

When no approved environment fits, develop a new one through
`local://environments/dev/<environment-id>` VFS paths: `environment_create` derives a
workspace from an approved base, you author the Containerfile through the VFS,
`environment_manifest_update` edits manifest.toml via old_string/new_string
replacement (read the file through the VFS first; host-owned fields such as
status and lifecycle must not change, and every edit is validated before it
lands), `environment_validate` runs the static gates, `environment_build`
also builds and smoke-tests the image, and `environment_install` activates
the digest-pinned local reference in the catalog so plugins can bind it.
`environment_fork`, `environment_uninstall`, and `environment_container_run`
mirror their plugin counterparts. Building, pushing, and catalog activation
are host-owned: never attempt them through shell tools.