//! `SkillFactory` trait — placeholder for Phase 1.
//!
//! Skills are pure data (a `Skill` struct), so unlike nodes there's no real
//! factory to register. This trait exists to mirror [`node_factory::NodeFactory`]
//! for API symmetry — a skill can be "registered" (added to the registry cache)
//! and "instantiated" (materialized into a `SubgraphNode`).

use crate::error::Result;
use crate::model::Skill;
use async_trait::async_trait;

#[async_trait]
#[allow(missing_docs)]
pub trait SkillFactory: Send + Sync {
    /// Skill name (unique across the registry).
    fn name(&self) -> &str;
    /// One-line description.
    fn description(&self) -> &str;
    /// Materialize the skill from persistence.
    async fn load(&self) -> Result<Skill>;
}
