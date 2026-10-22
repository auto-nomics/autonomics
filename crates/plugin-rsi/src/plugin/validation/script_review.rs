//! Static review of inlined adapter scripts.

use container_plugin::node_definition::NodeDefinition;

/// Reviews the non-executable security contract for one node script.
pub(crate) struct StaticScriptReviewer<'a> {
    node: &'a NodeDefinition,
    script: &'a str,
}

impl<'a> StaticScriptReviewer<'a> {
    pub(crate) fn new(node: &'a NodeDefinition, script: &'a str) -> Self {
        Self { node, script }
    }

    pub(crate) fn review(&self) -> Result<(), String> {
        self.ensure_nonempty()?;
        self.ensure_strict_shell()?;
        self.ensure_file_contract()?;
        self.ensure_no_forbidden_tokens()
    }

    fn ensure_nonempty(&self) -> Result<(), String> {
        if self.script.trim().is_empty() {
            return Err("adapter script is empty".into());
        }
        Ok(())
    }

    fn ensure_strict_shell(&self) -> Result<(), String> {
        if self.node.command.interpreter.contains("sh") && !self.script.contains("set -eu") {
            return Err("shell adapters must start with strict `set -eu` behavior".into());
        }
        Ok(())
    }

    fn ensure_file_contract(&self) -> Result<(), String> {
        if self.node.ports.inputs.is_empty() {
            return Err("MVP nodes must have at least one file input".into());
        }
        if !self.script.contains("AUTONOMICS_INPUT0") {
            return Err("script must validate/read AUTONOMICS_INPUT0".into());
        }
        if !self.script.contains("AUTONOMICS_OUTPUT0") {
            return Err("script must write AUTONOMICS_OUTPUT0".into());
        }
        Ok(())
    }

    fn ensure_no_forbidden_tokens(&self) -> Result<(), String> {
        for forbidden in [
            "curl ",
            "wget ",
            "ssh ",
            "scp ",
            "/home/",
            "/Users/",
            "/mnt/",
            "AUTONOMICS_PLUGIN_ROOT",
        ] {
            if self.script.contains(forbidden) {
                return Err(format!(
                    "forbidden host/network token in script: {forbidden:?}"
                ));
            }
        }
        Ok(())
    }
}
