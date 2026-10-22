//! Ordered gate orchestration for one plugin validation attempt.

use container_plugin::manifest::PluginManifest;

use crate::foundation::report::{GateResult, ValidationReport};
use crate::foundation::workspace::PluginWorkspace;

use super::RegistryCompiler;
use super::manifest_loader::ManifestLoader;
use super::policy::PolicyValidator;
use super::script_review::StaticScriptReviewer;
use super::secret_scanner::SecretScanner;
use crate::environment::catalog::EnvironmentCatalog;

/// Gate names are centralized to keep report construction typo-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Manifest,
    Policy,
    KindCollision,
    ScriptStatic,
    RegistryCompile,
    SecretScan,
}

impl Gate {
    fn name(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::Policy => "policy",
            Self::KindCollision => "kind_collision",
            Self::ScriptStatic => "script_static",
            Self::RegistryCompile => "registry_compile",
            Self::SecretScan => "secret_scan",
        }
    }
}

/// Inputs that remain immutable for one validation attempt.
struct ValidationContext<'a> {
    plugin_name: &'a str,
    workspace: &'a PluginWorkspace,
    catalog: &'a EnvironmentCatalog,
    installed_addresses: &'a [String],
    owned_kinds: &'a [String],
}

/// State accumulated while the validator moves through the gate sequence.
struct ValidationState {
    attempt: u32,
    gates: Vec<GateResult>,
    manifest: Option<PluginManifest>,
}

impl ValidationState {
    fn new(attempt: u32) -> Self {
        Self {
            attempt,
            gates: Vec::new(),
            manifest: None,
        }
    }

    fn pass(&mut self, gate: Gate) {
        self.gates.push(GateResult::pass(gate.name()));
    }

    fn fail(&mut self, gate: Gate, reason: impl Into<String>) {
        self.gates.push(GateResult::fail(gate.name(), reason));
    }

    fn contains(&self, gate: Gate) -> bool {
        self.gates.iter().any(|result| result.name == gate.name())
    }

    fn report(&self, plugin_name: &str) -> ValidationReport {
        ValidationReport::new(plugin_name, self.attempt, self.gates.clone())
    }

    fn blocked_report(&mut self, plugin_name: &str, failed_gate: Gate) -> ValidationReport {
        for gate in [Gate::ScriptStatic, Gate::RegistryCompile, Gate::SecretScan] {
            if !self.contains(gate) {
                self.gates.push(GateResult::blocked(
                    gate.name(),
                    format!("not run after `{}` failed", failed_gate.name()),
                ));
            }
        }
        self.report(plugin_name)
    }
}

/// Object-oriented facade over the six deterministic plugin gates.
///
/// Each gate is one method and every early stop is explicit. Keeping the
/// loaded manifest in [`ValidationState`] also makes the dependency between
/// manifest loading and every later gate visible in one place.
pub(crate) struct WorkspaceValidator<'a> {
    context: ValidationContext<'a>,
    state: ValidationState,
}

impl<'a> WorkspaceValidator<'a> {
    pub(crate) fn new(
        plugin_name: &'a str,
        workspace: &'a PluginWorkspace,
        catalog: &'a EnvironmentCatalog,
        installed_addresses: &'a [String],
        owned_kinds: &'a [String],
        attempt: u32,
    ) -> Self {
        Self {
            context: ValidationContext {
                plugin_name,
                workspace,
                catalog,
                installed_addresses,
                owned_kinds,
            },
            state: ValidationState::new(attempt),
        }
    }

    pub(crate) fn run(mut self) -> ValidationReport {
        if !self.manifest_gate() {
            return self.stop(Gate::Manifest);
        }
        if !self.policy_gate() {
            return self.stop(Gate::Policy);
        }
        if !self.kind_collision_gate() {
            return self.stop(Gate::KindCollision);
        }
        if !self.script_static_gate() {
            return self.stop(Gate::ScriptStatic);
        }
        if !self.registry_compile_gate() {
            return self.stop(Gate::RegistryCompile);
        }
        self.secret_scan_gate();
        self.state.report(self.context.plugin_name)
    }

    fn manifest_gate(&mut self) -> bool {
        match ManifestLoader::new(self.context.workspace).load() {
            Ok(manifest) => {
                self.state.manifest = Some(manifest);
                self.state.pass(Gate::Manifest);
                true
            }
            Err(errors) => {
                self.state.fail(Gate::Manifest, errors.join("; "));
                false
            }
        }
    }

    fn policy_gate(&mut self) -> bool {
        let result = {
            let Some(manifest) = self.state.manifest.as_ref() else {
                unreachable!("policy gate runs only after a successful manifest gate");
            };
            PolicyValidator::new(
                self.context.plugin_name,
                self.context.workspace,
                manifest,
                self.context.catalog,
            )
            .validate()
        };

        self.record_unit_gate(Gate::Policy, result)
    }

    fn kind_collision_gate(&mut self) -> bool {
        let collisions = self.kind_collisions();
        if collisions.is_empty() {
            self.state.pass(Gate::KindCollision);
            return true;
        }

        self.state.fail(
            Gate::KindCollision,
            format!("node kinds already registered: {}", collisions.join(", ")),
        );
        false
    }

    fn kind_collisions(&self) -> Vec<String> {
        let Some(manifest) = self.state.manifest.as_ref() else {
            unreachable!("collision gate runs only after manifest loading");
        };
        let plugin_name = self.context.plugin_name;
        let owned_addresses: Vec<String> = self
            .context
            .owned_kinds
            .iter()
            .map(|kind| format!("{plugin_name}/{kind}"))
            .collect();

        manifest
            .nodes
            .iter()
            .map(|node| format!("{plugin_name}/{}", node.kind))
            .filter(|address| {
                self.context.installed_addresses.contains(address)
                    && !owned_addresses.contains(address)
            })
            .collect()
    }

    fn script_static_gate(&mut self) -> bool {
        let Some(manifest) = self.state.manifest.as_ref() else {
            unreachable!("script gate runs only after manifest loading");
        };
        let Some(error) = manifest.nodes.iter().find_map(|node| {
            let script = node.command.script.as_deref().unwrap_or_default();
            StaticScriptReviewer::new(node, script).review().err()
        }) else {
            self.state.pass(Gate::ScriptStatic);
            return true;
        };

        self.state.fail(Gate::ScriptStatic, error);
        false
    }

    fn registry_compile_gate(&mut self) -> bool {
        let Some(manifest) = self.state.manifest.as_ref() else {
            unreachable!("compile gate runs only after manifest loading");
        };
        let result = RegistryCompiler::new(manifest).compile();
        self.record_unit_gate(Gate::RegistryCompile, result)
    }

    fn secret_scan_gate(&mut self) -> bool {
        let result = SecretScanner::new(self.context.workspace).scan();
        self.record_unit_gate(Gate::SecretScan, result)
    }

    fn record_unit_gate(&mut self, gate: Gate, result: Result<(), String>) -> bool {
        match result {
            Ok(()) => {
                self.state.pass(gate);
                true
            }
            Err(error) => {
                self.state.fail(gate, error);
                false
            }
        }
    }

    fn stop(mut self, failed_gate: Gate) -> ValidationReport {
        self.state
            .blocked_report(self.context.plugin_name, failed_gate)
    }
}

/// Stable public entry point retained by lifecycle and external integrations.
pub fn validate_workspace(
    plugin_name: &str,
    workspace: &PluginWorkspace,
    catalog: &crate::environment::catalog::EnvironmentCatalog,
    installed_addresses: &[String],
    owned_kinds: &[String],
    attempt: u32,
) -> ValidationReport {
    WorkspaceValidator::new(
        plugin_name,
        workspace,
        catalog,
        installed_addresses,
        owned_kinds,
        attempt,
    )
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::catalog::{Environment, EnvironmentCatalog};
    use crate::foundation::report::GateStatus;
    use crate::foundation::workspace::PluginWorkspace;

    const ENVIRONMENT_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";

    fn catalog() -> EnvironmentCatalog {
        let mut catalog = EnvironmentCatalog::default();
        catalog.insert(
            "alpine",
            Environment {
                reference: ENVIRONMENT_REFERENCE.into(),
                interpreters: vec!["sh".into()],
            },
        );
        catalog
    }

    fn workspace(panel: bool) -> PluginWorkspace {
        let root = tempfile::tempdir().unwrap();
        let workspace = PluginWorkspace::new(root.keep());
        let panels = if panel {
            r#"
[[panels]]
binding = "reference"
mount = "/panels/reference"
bundle = "owner/reference-panel"
"#
        } else {
            ""
        };
        workspace
            .write_text(
                "manifest.toml",
                &format!(
                    r#"
schema_version = 1
plugin_name = "panel-plugin"
{panels}

[image]
reference = "{ENVIRONMENT_REFERENCE}"

[[nodes]]
kind = "copy"
desc = "Copy input"
doc = "Copy input to output."

[nodes.ports]
inputs = [{{ type = "file" }}]
outputs = [{{ path = "result.txt" }}]

[nodes.command]
interpreter = "sh"
argv = []
script_file = "scripts/adapter.sh"
"#
                ),
            )
            .unwrap();
        workspace
            .write_text(
                "scripts/adapter.sh",
                "set -eu\ncp \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"\n",
            )
            .unwrap();
        workspace
            .write_text("README.md", "# Panel plugin\n\nDeterministic fixture.\n")
            .unwrap();
        workspace
    }

    fn reason(report: &ValidationReport, gate: &str) -> String {
        report
            .gates
            .iter()
            .find(|result| result.name == gate)
            .and_then(|result| result.reason.clone())
            .unwrap_or_default()
    }

    #[test]
    fn panel_free_fixture_passes_all_six_gates() {
        let report = validate_workspace("panel-plugin", &workspace(false), &catalog(), &[], &[], 1);

        assert_eq!(report.overall, GateStatus::Pass);
        assert_eq!(report.gates.len(), 6);
    }

    #[test]
    fn greenfield_panel_policy_is_isolated_and_blocks_later_gates() {
        let report = validate_workspace("panel-plugin", &workspace(true), &catalog(), &[], &[], 1);

        assert_eq!(report.overall, GateStatus::Fail);
        assert_eq!(
            reason(&report, "policy"),
            "panels are not allowed in the greenfield MVP"
        );
        assert_eq!(
            report
                .gates
                .iter()
                .map(|gate| gate.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "manifest",
                "policy",
                "script_static",
                "registry_compile",
                "secret_scan"
            ]
        );
    }
}
