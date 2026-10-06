//! Deterministic routing for shared observations.
//!
//! Routing does not transfer ownership of an observation. It records which
//! evolution subsystems should consume it and preserves the derived request
//! or proposal identity on the route.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::observation::Observation;
use crate::observation::ObservationKind;

/// Systems that may consume one shared observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationAudience {
    Skill,
    Plugin,
    Both,
    Unassigned,
}

/// Lifecycle state of one routing decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationRouteStatus {
    Pending,
    Consumed,
    Rejected,
}

/// A durable decision that sends shared evidence to an evolution consumer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationRoute {
    pub id: String,
    pub observation_id: String,
    pub audience: ObservationAudience,
    pub status: ObservationRouteStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub reason: String,
}

/// The deterministic result of routing one observation.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteDecision {
    pub audience: ObservationAudience,
    pub reason: String,
}

/// Classify an observation using explicit and inferred ownership.
pub fn classify_observation(
    observation: &Observation,
    plugin_name: Option<&str>,
    plugin_owner: Option<&str>,
) -> RouteDecision {
    let explicit_plugin = plugin_name.or(plugin_owner);
    match (explicit_plugin, observation.kind) {
        (Some(owner), ObservationKind::Failure) => RouteDecision {
            audience: ObservationAudience::Plugin,
            reason: format!("failure is owned by plugin `{owner}`"),
        },
        (Some(owner), _) => RouteDecision {
            audience: ObservationAudience::Both,
            reason: format!("reusable guidance for plugin `{owner}`"),
        },
        (None, ObservationKind::Recipe | ObservationKind::Caveat) => RouteDecision {
            audience: ObservationAudience::Skill,
            reason: "unowned reusable guidance belongs to skill distillation".into(),
        },
        (None, _) => RouteDecision {
            audience: ObservationAudience::Unassigned,
            reason: "failure has no known plugin owner; needs triage".into(),
        },
    }
}

fn route_id(
    observation_id: &str,
    audience: ObservationAudience,
    plugin_name: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(observation_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(format!("{audience:?}").to_lowercase().as_bytes());
    hasher.update(b"\0");
    hasher.update(plugin_name.unwrap_or_default().as_bytes());
    let digest = hex(&hasher.finalize());
    format!("OR-{}", &digest[..16])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Build a pending route from a classification and inferred plugin target.
pub fn observation_route(
    observation: &Observation,
    plugin_name: Option<&str>,
    plugin_owner: Option<&str>,
) -> (RouteDecision, ObservationRoute) {
    let decision = classify_observation(observation, plugin_name, plugin_owner);
    let target_plugin = plugin_name.map(str::to_string).or_else(|| {
        matches!(
            decision.audience,
            ObservationAudience::Plugin | ObservationAudience::Both
        )
        .then(|| plugin_owner.map(str::to_string))
        .flatten()
    });
    let derived_id = route_id(&observation.id, decision.audience, target_plugin.as_deref());
    let route = ObservationRoute {
        id: derived_id,
        observation_id: observation.id.clone(),
        audience: decision.audience,
        status: ObservationRouteStatus::Pending,
        plugin_name: target_plugin,
        node_kind: observation.node_kind.clone(),
        request_id: None,
        reason: decision.reason.clone(),
    };
    (decision, route)
}

/// Durable routing ledger beneath `<state_dir>/feedback/routes`.
#[derive(Debug, Clone)]
pub struct ObservationRouteStore {
    root: PathBuf,
}

impl ObservationRouteStore {
    pub fn open(state_dir: &Path) -> Self {
        Self {
            root: state_dir.join("feedback").join("routes"),
        }
    }

    pub fn record(&self, route: ObservationRoute) -> Result<ObservationRoute> {
        let path = self.root.join(format!("{}.toml", route.id));
        if path.exists() {
            return self.find(&route.id)?.ok_or_else(|| {
                Error::InvalidRequest(format!("unreadable observation route {}", route.id))
            });
        }
        std::fs::create_dir_all(&self.root)?;
        atomic_toml(&path, &route)?;
        Ok(route)
    }

    pub fn find(&self, id: &str) -> Result<Option<ObservationRoute>> {
        let path = self.root.join(format!("{id}.toml"));
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&text)
            .map(Some)
            .map_err(|source| Error::Parse { path, source })
    }

    pub fn list(&self) -> Vec<ObservationRoute> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut routes = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".toml") || name.starts_with('.') {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(route) = toml::from_str::<ObservationRoute>(&text) {
                routes.push(route);
            }
        }
        routes.sort_by(|a, b| a.id.cmp(&b.id));
        routes
    }

    pub fn set_status(&self, id: &str, status: ObservationRouteStatus) -> Result<ObservationRoute> {
        let mut route = self
            .find(id)?
            .ok_or_else(|| Error::InvalidRequest(format!("unknown observation route {id:?}")))?;
        route.status = status;
        let path = self.root.join(format!("{id}.toml"));
        atomic_toml(&path, &route)?;
        Ok(route)
    }

    /// Mark a route consumed by a derived request, preserving its derived id.
    pub fn consume(&self, id: &str, request_id: &str) -> Result<ObservationRoute> {
        let mut route = self
            .find(id)?
            .ok_or_else(|| Error::InvalidRequest(format!("unknown observation route {id:?}")))?;
        route.status = ObservationRouteStatus::Consumed;
        route.request_id = Some(request_id.to_string());
        let path = self.root.join(format!("{id}.toml"));
        atomic_toml(&path, &route)?;
        Ok(route)
    }
}

fn atomic_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(value).map_err(|source| Error::Serialize {
        path: path.to_path_buf(),
        source,
    })?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|source| Error::Write {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| Error::Write {
        path: tmp.clone(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{ObservationInput, ObservationSource};

    fn observation(kind: ObservationKind) -> Observation {
        Observation::from_input(
            ObservationInput {
                kind,
                source: ObservationSource::Agent,
                summary: "adapter fails".into(),
                body: "Validate the input.".into(),
                node_kind: Some("demo_adapter".into()),
                error: Some("invalid input".into()),
            },
            0,
        )
    }

    #[test]
    fn owned_failures_go_to_plugins_and_unowned_recipes_go_to_skills() {
        let failure = observation(ObservationKind::Failure);
        assert_eq!(
            classify_observation(&failure, Some("demo"), None).audience,
            ObservationAudience::Plugin
        );

        let recipe = observation(ObservationKind::Recipe);
        assert_eq!(
            classify_observation(&recipe, None, None).audience,
            ObservationAudience::Skill
        );
    }

    #[test]
    fn routes_are_idempotent_and_consume_with_request_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationRouteStore::open(tmp.path());
        let (_, route) =
            observation_route(&observation(ObservationKind::Failure), None, Some("demo"));
        assert_eq!(store.record(route.clone()).unwrap().id, route.id);
        assert_eq!(store.list().len(), 1);

        let consumed = store.consume(&route.id, "R-test").unwrap();
        assert_eq!(consumed.status, ObservationRouteStatus::Consumed);
        assert_eq!(consumed.request_id.as_deref(), Some("R-test"));
        assert_eq!(
            store.find(&route.id).unwrap().unwrap().request_id,
            consumed.request_id
        );
    }
}
