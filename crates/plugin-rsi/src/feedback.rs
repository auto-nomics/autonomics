//! Shared observation feedback between skill and plugin evolution.
//!
//! Skill observations remain the canonical evidence store: they are
//! content-addressed, useful to skill distillation on their own, and can be
//! promoted into a plugin request without duplicating the evidence.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use skills::observation::{Observation, ObservationInput, ObservationKind, ObservationSource};

use crate::{Error, RequestIntent, RequestRecord, RequestSource, RequestStatus, Result};

/// A plugin request promoted from one canonical observation.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRequest {
    /// The shared skill observation retained for skill distillation.
    pub observation: Observation,
    /// The plugin demand backed by that observation.
    pub request: RequestRecord,
}

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

pub(crate) fn classify_observation(
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
        let text = std::fs::read_to_string(&path).map_err(|source| Error::ReadFile {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&text)
            .map(Some)
            .map_err(|source| Error::ParseToml { path, source })
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

pub(crate) fn observation_route(
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

fn atomic_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(value).map_err(|source| Error::SerializeToml {
        path: path.to_path_buf(),
        source,
    })?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|source| Error::WriteFile {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| Error::WriteFile {
        path: tmp.clone(),
        source,
    })
}

fn request_source(source: ObservationSource) -> RequestSource {
    match source {
        ObservationSource::Agent => RequestSource::Agent,
        ObservationSource::Eval => RequestSource::Eval,
        ObservationSource::WorkflowRun => RequestSource::WorkflowRun,
        ObservationSource::Cli => RequestSource::User,
    }
}

pub(crate) fn observation_request(
    requests: &crate::RequestStore,
    observations: &skills::observation::ObservationStore,
    observation_id: &str,
    intent: RequestIntent,
    plugin_name: Option<&str>,
) -> Result<ObservationRequest> {
    let observation = observations
        .list()
        .into_iter()
        .find(|observation| observation.id == observation_id)
        .ok_or_else(|| Error::InvalidRequest(format!("unknown observation {observation_id:?}")))?;
    let request = requests.record(RequestRecord {
        id: String::new(),
        created_at: 0,
        source: request_source(observation.source),
        intent,
        summary: observation.summary.clone(),
        body: observation.body.clone(),
        plugin_name: plugin_name.map(str::to_string),
        evidence_ids: vec![observation.id.clone()],
        status: RequestStatus::Open,
    })?;
    Ok(ObservationRequest {
        observation,
        request,
    })
}

pub(crate) fn default_request_intent(kind: ObservationKind) -> RequestIntent {
    match kind {
        ObservationKind::Failure => RequestIntent::FixNode,
        ObservationKind::Recipe | ObservationKind::Caveat => RequestIntent::OptimizeNode,
    }
}
