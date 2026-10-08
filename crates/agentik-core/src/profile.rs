//! Hardcoded agent kinds and the per-agent persisted configuration.
//!
//! Agents come in exactly two kinds — [`AgentKind::Researcher`] and
//! [`AgentKind::Developer`] — whose identity, prompts, and tool
//! capabilities are fixed in code. Per-agent state that outlives a spawn
//! (model preference, runtime overrides) lives in
//! [`AgentProfileConfig`], persisted in the `agents.config_json` column.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::storage::AgentRuntimeOverrides;

/// The kind of agent to instantiate.
///
/// Kinds replace the old hierarchical profile registry: every capability
/// that used to be resolved through a stored profile blueprint is
/// now a hardcoded property of the kind. Worker agents may still use
/// hierarchical *runtime* paths (`researcher/worker`); the kind is derived
/// from the first segment (see [`AgentKind::from_legacy_profile_path`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Researcher,
    Developer,
}

impl AgentKind {
    /// Every kind, in picker/display order.
    pub const ALL: [Self; 2] = [Self::Researcher, Self::Developer];

    /// Wire/stable name: `"researcher"` or `"developer"`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Researcher => "researcher",
            Self::Developer => "developer",
        }
    }

    /// Strict parse of a kind name. Returns `None` for anything else.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "researcher" => Some(Self::Researcher),
            "developer" => Some(Self::Developer),
            _ => None,
        }
    }

    /// Map a legacy hierarchical profile path onto a kind.
    ///
    /// `developer` and its descendants are [`AgentKind::Developer`];
    /// everything else — `researcher`, `researcher/<specialist>`, the
    /// legacy `default`, and unknown paths — is
    /// [`AgentKind::Researcher`]. This mirrors the role enforcement the
    /// runtime host applied before kinds existed.
    pub fn from_legacy_profile_path(path: &str) -> Self {
        if path == "developer" || path.starts_with("developer/") {
            Self::Developer
        } else {
            Self::Researcher
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Researcher => "Full-featured biomedical research assistant.",
            Self::Developer => "Node and plugin developer for the DAG ecosystem.",
        }
    }

    pub const fn agent_identity(self) -> &'static str {
        match self {
            Self::Researcher => {
                "You are a biomedical research assistant specializing in genomics, GWAS analysis, and literature mining."
            }
            Self::Developer => {
                "You are a DAG node and plugin developer. You build, validate, install, and uninstall plugins through the host-owned plugin lifecycle; you do not perform open-ended research analysis in plugin debug containers."
            }
        }
    }

    const fn is_researcher(self) -> bool {
        matches!(self, Self::Researcher)
    }

    // ── Tool capability flags ────────────────────────────────

    pub const fn enable_bibliography(self) -> bool {
        self.is_researcher()
    }

    pub const fn enable_writing(self) -> bool {
        self.is_researcher()
    }

    pub const fn enable_opengwas(self) -> bool {
        self.is_researcher()
    }

    pub const fn enable_opentargets(self) -> bool {
        self.is_researcher()
    }

    pub const fn enable_gwascatalog(self) -> bool {
        self.is_researcher()
    }

    pub const fn enable_chembl(self) -> bool {
        self.is_researcher()
    }

    pub const fn enable_rcsb(self) -> bool {
        self.is_researcher()
    }

    pub const fn enable_string(self) -> bool {
        self.is_researcher()
    }

    pub const fn enable_kegg(self) -> bool {
        self.is_researcher()
    }

    /// Enable `/plugins/dev/<plugin>` development tools.
    pub const fn enable_plugin_rsi(self) -> bool {
        !self.is_researcher()
    }

    /// Expose DAG history tools. Enabled for every kind.
    pub const fn enable_dag_history(self) -> bool {
        true
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for AgentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_name(s)
            .ok_or_else(|| format!("unknown agent kind `{s}`; expected researcher or developer"))
    }
}

/// Per-agent state persisted in the `agents.config_json` column.
///
/// Rows written before the agent-kind refactor hold a full serialized
/// profile (with the profile `path`, capability flags, and timestamps);
/// [`AgentProfileConfig::from_json`] parses both shapes and never fails.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfileConfig {
    #[serde(default)]
    pub kind: AgentKind,
    /// Preferred model in `"provider:model"` format, or `None` to use the
    /// global default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    /// Runtime settings layered on top of the daemon-wide defaults.
    #[serde(default)]
    pub runtime: AgentRuntimeOverrides,
}

impl AgentProfileConfig {
    pub const fn new(kind: AgentKind) -> Self {
        Self {
            kind,
            preferred_model: None,
            runtime: AgentRuntimeOverrides {
                use_memory: None,
                generate_memory: None,
            },
        }
    }

    /// Tolerant parse of a persisted `config_json` value.
    ///
    /// Resolution order for the kind: the `"kind"` key (strict), then the
    /// legacy `"path"` key, then its serde alias `"name"`, both mapped
    /// through [`AgentKind::from_legacy_profile_path`]. `preferred_model`
    /// and `runtime` survive from either shape; every other legacy key is
    /// ignored. `null`, non-objects, and garbage degrade to
    /// `Researcher` + defaults rather than failing.
    pub fn from_json(value: &serde_json::Value) -> Self {
        let Some(obj) = value.as_object() else {
            return Self::new(AgentKind::Researcher);
        };

        let kind = if let Some(kind) = obj
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(AgentKind::from_name)
        {
            kind
        } else if let Some(kind) = ["path", "name"]
            .iter()
            .find_map(|key| obj.get(*key))
            .and_then(|v| v.as_str())
            .map(AgentKind::from_legacy_profile_path)
        {
            kind
        } else {
            AgentKind::Researcher
        };

        let preferred_model = obj
            .get("preferred_model")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let runtime = obj
            .get("runtime")
            .and_then(|v| serde_json::from_value::<AgentRuntimeOverrides>(v.clone()).ok())
            .unwrap_or_default();

        Self {
            kind,
            preferred_model,
            runtime,
        }
    }

    /// Layer caller-provided runtime overrides onto this config.
    ///
    /// Existing values remain active for fields the caller leaves as
    /// `None`. This is the shared entry point used by headless callers
    /// before an agent is spawned.
    pub fn apply_runtime_overrides(&mut self, overrides: AgentRuntimeOverrides) {
        self.runtime.use_memory = overrides.use_memory.or(self.runtime.use_memory);
        self.runtime.generate_memory = overrides.generate_memory.or(self.runtime.generate_memory);
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_shape_round_trips() {
        let config = AgentProfileConfig {
            kind: AgentKind::Developer,
            preferred_model: Some("openai:gpt-4o".into()),
            runtime: AgentRuntimeOverrides {
                use_memory: Some(false),
                generate_memory: Some(true),
            },
        };
        let restored = AgentProfileConfig::from_json(&config.to_json());
        assert_eq!(restored, config);
    }

    #[test]
    fn serializes_as_flat_object() {
        let json = AgentProfileConfig::new(AgentKind::Researcher).to_json();
        assert_eq!(
            json,
            serde_json::json!({ "kind": "researcher", "runtime": {} })
        );
    }

    #[test]
    fn legacy_full_profile_rows_parse_via_path_and_name_alias() {
        // Shape written by the old runtime: a full serialized profile.
        let legacy = serde_json::json!({
            "id": "0b7ec0e8-6c86-4e0b-9d5a-3f7a4c1e2a11",
            "path": "developer",
            "description": "stale",
            "agent_identity": "stale",
            "system_prompt": null,
            "enable_bibliography": true,
            "enable_plugin_rsi": false,
            "preferred_model": "anthropic:claude",
            "runtime": { "use_memory": true, "generate_memory": null },
            "created_at": 1,
            "updated_at": 2
        });
        let parsed = AgentProfileConfig::from_json(&legacy);
        assert_eq!(parsed.kind, AgentKind::Developer);
        assert_eq!(parsed.preferred_model.as_deref(), Some("anthropic:claude"));
        assert_eq!(parsed.runtime.use_memory, Some(true));
        assert_eq!(parsed.runtime.generate_memory, None);

        // Even older rows stored the path under the `name` key.
        let mut aliased = legacy.clone();
        aliased.as_object_mut().unwrap().remove("path");
        aliased["name"] = "researcher".into();
        let parsed = AgentProfileConfig::from_json(&aliased);
        assert_eq!(parsed.kind, AgentKind::Researcher);
    }

    #[test]
    fn kind_classification_table() {
        let developer = ["developer", "developer/nodes"];
        for path in developer {
            assert_eq!(
                AgentKind::from_legacy_profile_path(path),
                AgentKind::Developer,
                "{path}"
            );
        }
        let researcher = [
            "researcher",
            "researcher/genomics",
            "developer-x",
            "default",
            "literature",
            "",
            "garbage",
        ];
        for path in researcher {
            assert_eq!(
                AgentKind::from_legacy_profile_path(path),
                AgentKind::Researcher,
                "{path}"
            );
        }
    }

    #[test]
    fn degenerate_values_fall_back_to_researcher_defaults() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!("researcher"),
            serde_json::json!(42),
            serde_json::json!({ "kind": "wizard" }),
            serde_json::json!({ "kind": 7 }),
        ] {
            let parsed = AgentProfileConfig::from_json(&value);
            assert_eq!(parsed, AgentProfileConfig::new(AgentKind::Researcher));
        }
    }

    #[test]
    fn runtime_override_layering_keeps_existing_values() {
        let mut config = AgentProfileConfig::new(AgentKind::Researcher);
        config.runtime.generate_memory = Some(false);
        config.apply_runtime_overrides(AgentRuntimeOverrides {
            use_memory: Some(false),
            generate_memory: None,
        });
        assert_eq!(config.runtime.use_memory, Some(false));
        assert_eq!(config.runtime.generate_memory, Some(false));
    }

    #[test]
    fn from_name_is_strict() {
        assert_eq!(
            AgentKind::from_name("researcher"),
            Some(AgentKind::Researcher)
        );
        assert_eq!(
            AgentKind::from_name("developer"),
            Some(AgentKind::Developer)
        );
        for rejected in ["Researcher", "researcher/genomics", "", "default"] {
            assert_eq!(AgentKind::from_name(rejected), None, "{rejected}");
        }
        let err = "wizard".parse::<AgentKind>().unwrap_err();
        assert_eq!(
            err,
            "unknown agent kind `wizard`; expected researcher or developer"
        );
    }

    #[test]
    fn kind_capability_flags_match_the_old_defaults() {
        let researcher = AgentKind::Researcher;
        assert!(researcher.enable_bibliography());
        assert!(researcher.enable_writing());
        assert!(researcher.enable_opengwas());
        assert!(researcher.enable_opentargets());
        assert!(researcher.enable_gwascatalog());
        assert!(researcher.enable_chembl());
        assert!(researcher.enable_rcsb());
        assert!(researcher.enable_string());
        assert!(researcher.enable_kegg());
        assert!(researcher.enable_dag_history());
        assert!(!researcher.enable_plugin_rsi());

        let developer = AgentKind::Developer;
        assert!(!developer.enable_bibliography());
        assert!(!developer.enable_writing());
        assert!(!developer.enable_opengwas());
        assert!(!developer.enable_opentargets());
        assert!(!developer.enable_gwascatalog());
        assert!(!developer.enable_chembl());
        assert!(!developer.enable_rcsb());
        assert!(!developer.enable_string());
        assert!(!developer.enable_kegg());
        assert!(developer.enable_dag_history());
        assert!(developer.enable_plugin_rsi());
    }

    #[test]
    fn all_lists_both_kinds_with_matching_names() {
        assert_eq!(AgentKind::ALL.len(), 2);
        for kind in AgentKind::ALL {
            assert_eq!(AgentKind::from_name(kind.name()), Some(kind));
            assert_eq!(kind.to_string(), kind.name());
        }
    }
}
