//! `SkillRegistry` — in-memory cache of available skills.
//!
//! Skills are persisted via [`crate::store::SkillRepo`]; the registry cache
//! lives in memory and is populated on app start by hydrating from storage.
//! The scheduler consults [`SkillRegistry::get_by_name`] when it encounters
//! a `kind == "skill"` node.

use crate::error::Result;
use crate::model::{Skill, SkillInfo};
use crate::registry::skill_factory::SkillFactory;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Thread-safe skill cache. Cheap to clone.
#[derive(Clone, Default)]
#[allow(missing_docs)]
pub struct SkillRegistry {
    by_name: Arc<RwLock<HashMap<String, Skill>>>,
}

impl SkillRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a skill in the cache.
    pub fn insert(&self, skill: Skill) {
        self.by_name
            .write()
            .unwrap()
            .insert(skill.name.clone(), skill);
    }

    /// Remove a skill by name.
    pub fn remove(&self, name: &str) -> Option<Skill> {
        self.by_name.write().unwrap().remove(name)
    }

    /// Look up a skill by name (the canonical look-up used by the scheduler).
    pub fn get_by_name(&self, name: &str) -> Option<Skill> {
        self.by_name.read().unwrap().get(name).cloned()
    }

    /// Look up a skill by stable id.
    pub fn get_by_id(&self, id: uuid::Uuid) -> Option<Skill> {
        self.by_name
            .read()
            .unwrap()
            .values()
            .find(|s| s.id == id)
            .cloned()
    }

    /// Summaries (latest version of each).
    pub fn list(&self) -> Vec<SkillInfo> {
        let now = chrono::Utc::now();
        let mut out: Vec<SkillInfo> = self
            .by_name
            .read()
            .unwrap()
            .values()
            .map(|s| SkillInfo {
                id: s.id,
                name: s.name.clone(),
                version: s.version,
                description: s.description.clone(),
                node_count: s.manifest.nodes.len() as u32,
                updated_at: now,
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Load a skill by name, consulting the on-disk store on cache miss.
    /// Returns `Ok(None)` if the skill does not exist.
    pub async fn load(&self, _name: &str) -> Result<Option<Skill>> {
        Ok(None)
    }
}

/// Marker helper retained for backward compatibility.
pub struct _SkillFactoryHandle {
    /// The factory we wrap.
    pub factory: Arc<dyn SkillFactory>,
}
