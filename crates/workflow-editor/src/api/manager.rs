//! `WorkflowManager` — per-app handle to the storage pool and registries.
//!
//! `WorkflowManager` is what an application embeds at startup. It holds the
//! [`DbPool`](crate::store::DbPool), [`NodeRegistry`](crate::registry::NodeRegistry),
//! [`SkillRegistry`](crate::registry::SkillRegistry), and a
//! [`Scheduler`](crate::executor::Scheduler).
//!
//! It is cheap to clone (`Arc`s inside) and never blocks — DB operations
//! are forwarded to a tokio task via `WorkflowClient`.

use crate::error::Result;
use crate::executor::Scheduler;
use crate::model::{NodeKindInfo, Skill, SkillInfo};
use crate::registry::{NodeRegistry, SkillRegistry};
use crate::store::{DbPool, NodeKindRepo, SkillRepo, WorkflowRepo, migrations};
use std::path::Path;
use std::sync::Arc;

/// Per-app workflow editor handle.
#[derive(Clone)]
pub struct WorkflowManager {
    /// Shared DB pool.
    pub(crate) pool: DbPool,
    /// In-memory node factory registry.
    pub(crate) node_registry: Arc<NodeRegistry>,
    /// In-memory skill cache.
    pub(crate) skill_registry: Arc<SkillRegistry>,
    /// Topological scheduler.
    pub(crate) scheduler: Arc<Scheduler>,
}

impl WorkflowManager {
    /// Open (or create) a workflow editor at `path` and run migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let pool = DbPool::open(path.as_ref())?;
        migrations::run(&pool)?;
        Ok(Self::from_pool(pool))
    }

    /// Open an in-memory database. Used by tests and ephemeral sessions.
    pub fn open_memory() -> Result<Self> {
        let pool = DbPool::open(Path::new(":memory:"))?;
        migrations::run(&pool)?;
        Ok(Self::from_pool(pool))
    }

    /// Wrap an existing pool.
    pub fn from_pool(pool: DbPool) -> Self {
        let node_registry = Arc::new(NodeRegistry::new());
        let skill_registry = Arc::new(SkillRegistry::new());
        let scheduler = Scheduler::new(Arc::clone(&node_registry), Arc::clone(&skill_registry));
        Self {
            pool,
            node_registry,
            skill_registry,
            scheduler,
        }
    }

    /// Spawn a new session actor and return a [`WorkflowClient`].
    ///
    /// Each session has its own message queue; multiple clients can run
    /// concurrently without contending. Session state is in the DB; nothing
    /// lives in the actor beyond its channel.
    pub fn new_session(&self) -> crate::api::WorkflowClient {
        crate::api::WorkflowClient::spawn(
            self.pool.clone(),
            Arc::clone(&self.node_registry),
            Arc::clone(&self.skill_registry),
            Arc::clone(&self.scheduler),
        )
    }

    // ── Metadata (sync, bypasses actor) ─────────────────────────────────

    /// List every registered node kind.
    pub fn list_node_kinds(&self) -> Vec<NodeKindInfo> {
        self.node_registry.list()
    }

    /// JSON Schema for one node kind, or `None` if the kind is unknown.
    pub fn get_node_spec(&self, kind: &str) -> Option<schemars::Schema> {
        self.node_registry.get(kind).map(|f| f.spec_schema())
    }

    /// Summary of every skill currently in the cache.
    pub fn list_skills(&self) -> Vec<SkillInfo> {
        self.skill_registry.list()
    }

    /// Fetch a skill by id from the cache, or `None` if not present.
    pub fn get_skill(&self, id: uuid::Uuid) -> Option<Skill> {
        self.skill_registry.get_by_id(id)
    }

    /// Direct access to the underlying workflow repo (sync). Used by tests
    /// and one-off administrative scripts.
    pub fn workflow_repo(&self) -> WorkflowRepo {
        WorkflowRepo::new(self.pool.clone())
    }

    /// Direct access to the in-memory node registry. Used by tests and
    /// applications that want to register factories at startup.
    pub fn node_registry(&self) -> Arc<NodeRegistry> {
        Arc::clone(&self.node_registry)
    }

    /// Direct access to the in-memory skill registry.
    pub fn skill_registry(&self) -> Arc<SkillRegistry> {
        Arc::clone(&self.skill_registry)
    }

    /// Direct access to the skill repo.
    pub fn skill_repo(&self) -> SkillRepo {
        SkillRepo::new(self.pool.clone())
    }

    /// Direct access to the node kind cache.
    pub fn node_kind_repo(&self) -> NodeKindRepo {
        NodeKindRepo::new(self.pool.clone())
    }

    /// Direct access to the shared [`Scheduler`]. Used by callers that want
    /// to run a workflow or build an executor tree outside the actor.
    pub fn scheduler_ref(&self) -> Arc<Scheduler> {
        Arc::clone(&self.scheduler)
    }
}
