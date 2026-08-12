//! `WorkflowClient` — per-session actor handle.
//!
//! Each call dispatches a [`WorkflowCmd`] over an `mpsc` channel to the
//! per-session actor; the actor replies on a `oneshot` channel. The actor
//! itself is a tokio task spawned by [`WorkflowManager::new_session`].
//!
//! Cancellation tokens are passed per-command; a `Drop` on the client does
//! **not** cancel an in-flight `Run`.

use crate::api::commands::WorkflowCmd;
use crate::api::manager::WorkflowManager;
use crate::error::{ApiError, Result};
use crate::executor::{Scheduler, ValidationReport, WorkflowResult};
use crate::model::{
    EdgeEntry, NodeEntry, Skill, SkillInfo, SnapshotInfo, WorkflowManifest,
};
use crate::registry::{NodeRegistry, SkillRegistry};
use crate::store::{DbPool, NodeKindRepo, SkillRepo, WorkflowRepo, WorkflowSummary};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Per-session handle to a workflow editor actor.
#[derive(Clone)]
pub struct WorkflowClient {
    tx: mpsc::UnboundedSender<WorkflowCmd>,
    /// Handle to the actor task; held by the *first* client; clones drop it.
    #[allow(dead_code)]
    handle: Option<Arc<JoinHandle<()>>>,
}

impl WorkflowClient {
    /// Spawn a new actor task and return a client. Public so external
    /// callers can wrap a `WorkflowManager` in their own actor. Most users
    /// should call [`WorkflowManager::new_session`].
    pub(crate) fn spawn(
        pool: DbPool,
        node_registry: Arc<NodeRegistry>,
        skill_registry: Arc<SkillRegistry>,
        scheduler: Arc<Scheduler>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(actor_loop(
            rx,
            pool,
            node_registry,
            skill_registry,
            scheduler,
        ));
        Self {
            tx,
            handle: Some(Arc::new(handle)),
        }
    }

    // ── Workflow CRUD ───────────────────────────────────────────────────

    /// Create a fresh workflow with the given name and return its id.
    pub async fn create_workflow(&self, name: impl Into<String>) -> Result<Uuid> {
        let input = CreateWorkflowInput {
            manifest: WorkflowManifest::new(name),
        };
        self.request(input).await
    }

    /// Load a workflow's current manifest.
    pub async fn load_workflow(&self, id: Uuid) -> Result<WorkflowManifest> {
        self.request(LoadWorkflowInput { id }).await
    }

    /// Save a workflow (append a snapshot).
    pub async fn save_workflow(
        &self,
        manifest: WorkflowManifest,
        message: impl Into<String>,
    ) -> Result<Uuid> {
        let id = manifest.id;
        let input = SaveWorkflowInput {
            id,
            manifest,
            message: message.into(),
        };
        self.request(input).await
    }

    /// Append a node to a workflow.
    pub async fn add_node(&self, workflow_id: Uuid, node: NodeEntry) -> Result<()> {
        self.request(AddNodeInput { workflow_id, node }).await
    }

    /// Remove a node and its connected edges.
    pub async fn remove_node(&self, workflow_id: Uuid, node_id: Uuid) -> Result<()> {
        self.request(RemoveNodeInput { workflow_id, node_id })
            .await
    }

    /// Add an edge.
    pub async fn add_edge(&self, workflow_id: Uuid, edge: EdgeEntry) -> Result<()> {
        self.request(AddEdgeInput { workflow_id, edge }).await
    }

    /// Remove an edge.
    pub async fn remove_edge(&self, workflow_id: Uuid, edge_id: Uuid) -> Result<()> {
        self.request(RemoveEdgeInput { workflow_id, edge_id })
            .await
    }

    /// Validate a manifest without persisting.
    pub async fn validate(&self, manifest: WorkflowManifest) -> Result<ValidationReport> {
        self.request(ValidateInput { manifest }).await
    }

    /// Run a workflow end-to-end.
    pub async fn run(
        &self,
        workflow_id: Uuid,
        inputs: serde_json::Map<String, serde_json::Value>,
    ) -> Result<WorkflowResult> {
        let cancel = CancellationToken::new();
        self.request(RunInput {
            workflow_id,
            inputs,
            cancel,
        })
        .await
    }

    /// Save a skill (creates or bumps the version of its `name`).
    pub async fn save_skill(&self, skill: Skill) -> Result<Skill> {
        self.request(SaveSkillInput { skill }).await
    }

    /// List workflow summaries.
    pub async fn list_workflows(&self) -> Result<Vec<WorkflowSummary>> {
        self.request(ListWorkflowsInput).await
    }

    /// List skill summaries.
    pub async fn list_skills(&self) -> Result<Vec<SkillInfo>> {
        self.request(ListSkillsInput).await
    }

    /// History (snapshot list) for a workflow.
    pub async fn history(&self, workflow_id: Uuid) -> Result<Vec<SnapshotInfo>> {
        self.request(HistoryInput { workflow_id }).await
    }

    /// Restore a workflow's current state to a prior snapshot.
    pub async fn checkout(&self, workflow_id: Uuid, snapshot_id: Uuid) -> Result<()> {
        self.request(CheckoutInput {
            workflow_id,
            snapshot_id,
        })
        .await
    }

    /// Serialize a skill (looked up by id) to pretty-printed JSON for export.
    pub async fn export_skill(&self, skill_id: Uuid) -> Result<String> {
        self.request(ExportSkillInput { skill_id }).await
    }

    /// Parse a JSON skill and save it (creating a new version).
    pub async fn import_skill(&self, json: String) -> Result<Skill> {
        self.request(ImportSkillInput { json }).await
    }

    /// Internal: send a command and await its reply.
    async fn request<T, I>(&self, input: I) -> Result<T>
    where
        I: IntoRequest<T>,
    {
        let (cmd, rx) = I::into_cmd_with_reply(input);
        self.tx.send(cmd).map_err(|_| ApiError::Disconnected)?;
        rx.await
            .map_err(|_| ApiError::Disconnected.into())
            .and_then(|r| r)
    }
}

// ── Input structs + IntoRequest impls ───────────────────────────────────
//
// Each public method builds one of these and hands it to `request`. The
// `IntoRequest` trait builds the corresponding `WorkflowCmd` variant
// (including a fresh `oneshot` channel) and returns the receiver.

/// Bridge from a typed input struct to a `WorkflowCmd` + reply receiver.
pub trait IntoRequest<T>: Sized {
    /// Build the command and reply receiver pair.
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<T>>);
}

macro_rules! input_struct {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[allow(missing_docs)]
        pub struct $name {
            $(pub $field: $ty),*
        }
    };
    ($name:ident) => {
        #[allow(missing_docs)]
        pub struct $name;
    };
}

input_struct!(CreateWorkflowInput { manifest: WorkflowManifest });
input_struct!(LoadWorkflowInput { id: Uuid });
input_struct!(SaveWorkflowInput {
    id: Uuid,
    manifest: WorkflowManifest,
    message: String,
});
input_struct!(AddNodeInput { workflow_id: Uuid, node: NodeEntry });
input_struct!(RemoveNodeInput { workflow_id: Uuid, node_id: Uuid });
input_struct!(AddEdgeInput { workflow_id: Uuid, edge: EdgeEntry });
input_struct!(RemoveEdgeInput { workflow_id: Uuid, edge_id: Uuid });
input_struct!(ValidateInput { manifest: WorkflowManifest });
input_struct!(RunInput {
    workflow_id: Uuid,
    inputs: serde_json::Map<String, serde_json::Value>,
    cancel: CancellationToken,
});
input_struct!(SaveSkillInput { skill: Skill });
input_struct!(ListWorkflowsInput);
input_struct!(ListSkillsInput);
input_struct!(HistoryInput { workflow_id: Uuid });
input_struct!(CheckoutInput { workflow_id: Uuid, snapshot_id: Uuid });
input_struct!(ExportSkillInput { skill_id: Uuid });
input_struct!(ImportSkillInput { json: String });

impl IntoRequest<Uuid> for CreateWorkflowInput {
    fn into_cmd_with_reply(
        input: Self,
    ) -> (WorkflowCmd, oneshot::Receiver<Result<Uuid>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::CreateWorkflow {
            manifest: input.manifest,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<WorkflowManifest> for LoadWorkflowInput {
    fn into_cmd_with_reply(
        input: Self,
    ) -> (WorkflowCmd, oneshot::Receiver<Result<WorkflowManifest>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::LoadWorkflow { id: input.id, reply: tx };
        (cmd, rx)
    }
}
impl IntoRequest<Uuid> for SaveWorkflowInput {
    fn into_cmd_with_reply(
        input: Self,
    ) -> (WorkflowCmd, oneshot::Receiver<Result<Uuid>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::SaveWorkflow {
            id: input.id,
            manifest: input.manifest,
            message: input.message,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<()> for AddNodeInput {
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<()>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::AddNode {
            workflow_id: input.workflow_id,
            node: input.node,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<()> for RemoveNodeInput {
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<()>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::RemoveNode {
            workflow_id: input.workflow_id,
            node_id: input.node_id,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<()> for AddEdgeInput {
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<()>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::AddEdge {
            workflow_id: input.workflow_id,
            edge: input.edge,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<()> for RemoveEdgeInput {
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<()>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::RemoveEdge {
            workflow_id: input.workflow_id,
            edge_id: input.edge_id,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<ValidationReport> for ValidateInput {
    fn into_cmd_with_reply(
        input: Self,
    ) -> (WorkflowCmd, oneshot::Receiver<Result<ValidationReport>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::Validate {
            manifest: input.manifest,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<WorkflowResult> for RunInput {
    fn into_cmd_with_reply(
        input: Self,
    ) -> (WorkflowCmd, oneshot::Receiver<Result<WorkflowResult>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::Run {
            workflow_id: input.workflow_id,
            inputs: input.inputs,
            reply: tx,
            cancel: input.cancel,
        };
        (cmd, rx)
    }
}
impl IntoRequest<Skill> for SaveSkillInput {
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<Skill>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::SaveSkill {
            skill: input.skill,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<Vec<WorkflowSummary>> for ListWorkflowsInput {
    fn into_cmd_with_reply(
        _input: Self,
    ) -> (WorkflowCmd, oneshot::Receiver<Result<Vec<WorkflowSummary>>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::ListWorkflows { reply: tx };
        (cmd, rx)
    }
}
impl IntoRequest<Vec<SkillInfo>> for ListSkillsInput {
    fn into_cmd_with_reply(
        _input: Self,
    ) -> (WorkflowCmd, oneshot::Receiver<Result<Vec<SkillInfo>>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::ListSkills { reply: tx };
        (cmd, rx)
    }
}
impl IntoRequest<Vec<SnapshotInfo>> for HistoryInput {
    fn into_cmd_with_reply(
        input: Self,
    ) -> (WorkflowCmd, oneshot::Receiver<Result<Vec<SnapshotInfo>>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::History {
            workflow_id: input.workflow_id,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<()> for CheckoutInput {
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<()>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::Checkout {
            workflow_id: input.workflow_id,
            snapshot_id: input.snapshot_id,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<String> for ExportSkillInput {
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<String>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::ExportSkill {
            skill_id: input.skill_id,
            reply: tx,
        };
        (cmd, rx)
    }
}
impl IntoRequest<Skill> for ImportSkillInput {
    fn into_cmd_with_reply(input: Self) -> (WorkflowCmd, oneshot::Receiver<Result<Skill>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = WorkflowCmd::ImportSkill {
            json: input.json,
            reply: tx,
        };
        (cmd, rx)
    }
}

// ── Actor loop ──────────────────────────────────────────────────────────

async fn actor_loop(
    mut rx: mpsc::UnboundedReceiver<WorkflowCmd>,
    pool: DbPool,
    _node_registry: Arc<NodeRegistry>,
    _skill_registry: Arc<SkillRegistry>,
    scheduler: Arc<Scheduler>,
) {
    let workflow_repo = WorkflowRepo::new(pool.clone());
    let skill_repo = SkillRepo::new(pool.clone());
    let _node_kind_repo = NodeKindRepo::new(pool.clone());

    while let Some(cmd) = rx.recv().await {
        match cmd {
            WorkflowCmd::CreateWorkflow { manifest, reply } => {
                let r = workflow_repo.create(&manifest);
                let _ = reply.send(r);
            }
            WorkflowCmd::LoadWorkflow { id, reply } => {
                let r = workflow_repo.get(id);
                let _ = reply.send(r);
            }
            WorkflowCmd::SaveWorkflow {
                id,
                manifest,
                message,
                reply,
            } => {
                let r = workflow_repo.save(id, &manifest, message);
                let _ = reply.send(r);
            }
            WorkflowCmd::AddNode {
                workflow_id,
                node,
                reply,
            } => {
                let r = (|| -> Result<()> {
                    let mut m = workflow_repo.get(workflow_id)?;
                    m.nodes.push(node);
                    m.validate()?;
                    workflow_repo.save(workflow_id, &m, "add_node")?;
                    Ok(())
                })();
                let _ = reply.send(r);
            }
            WorkflowCmd::RemoveNode {
                workflow_id,
                node_id,
                reply,
            } => {
                let r = (|| -> Result<()> {
                    let mut m = workflow_repo.get(workflow_id)?;
                    m.nodes.retain(|n| n.id != node_id);
                    m.edges.retain(|e| e.source != node_id && e.target != node_id);
                    m.validate()?;
                    workflow_repo.save(workflow_id, &m, "remove_node")?;
                    Ok(())
                })();
                let _ = reply.send(r);
            }
            WorkflowCmd::AddEdge {
                workflow_id,
                edge,
                reply,
            } => {
                let r = (|| -> Result<()> {
                    let mut m = workflow_repo.get(workflow_id)?;
                    m.edges.push(edge);
                    m.validate()?;
                    workflow_repo.save(workflow_id, &m, "add_edge")?;
                    Ok(())
                })();
                let _ = reply.send(r);
            }
            WorkflowCmd::RemoveEdge {
                workflow_id,
                edge_id,
                reply,
            } => {
                let r = (|| -> Result<()> {
                    let mut m = workflow_repo.get(workflow_id)?;
                    m.edges.retain(|e| e.id != edge_id);
                    m.validate()?;
                    workflow_repo.save(workflow_id, &m, "remove_edge")?;
                    Ok(())
                })();
                let _ = reply.send(r);
            }
            WorkflowCmd::Validate { manifest, reply } => {
                let r = scheduler.validate(&manifest);
                let _ = reply.send(Ok(r));
            }
            WorkflowCmd::Run {
                workflow_id,
                inputs,
                reply,
                cancel,
            } => {
                let repo = workflow_repo.clone();
                let sched = Arc::clone(&scheduler);
                tokio::spawn(async move {
                    let r = async move {
                        let m = repo.get(workflow_id)?;
                        let res = sched.run(&m, inputs, cancel).await?;
                        Ok(res)
                    }
                    .await;
                    let _ = reply.send(r);
                });
            }
            WorkflowCmd::SaveSkill { skill, reply } => {
                let r = skill_repo.save(&skill);
                let _ = reply.send(r);
            }
            WorkflowCmd::ListWorkflows { reply } => {
                let r = workflow_repo.list();
                let _ = reply.send(r);
            }
            WorkflowCmd::ListSkills { reply } => {
                let r = skill_repo.list();
                let _ = reply.send(r);
            }
            WorkflowCmd::History {
                workflow_id,
                reply,
            } => {
                let r = workflow_repo.history(workflow_id);
                let _ = reply.send(r);
            }
            WorkflowCmd::Checkout {
                workflow_id,
                snapshot_id,
                reply,
            } => {
                let r = workflow_repo.checkout(workflow_id, snapshot_id);
                let _ = reply.send(r);
            }
            WorkflowCmd::ExportSkill { skill_id, reply } => {
                // Read from the latest version of the skill directly.
                let r = (|| -> Result<String> {
                    let skill = skill_repo
                        .latest_by_id(skill_id)?
                        .ok_or(crate::error::StorageError::NotFound {
                            kind: "skill",
                            id: skill_id.to_string(),
                        })?;
                    Ok(serde_json::to_string_pretty(&skill)?)
                })();
                let _ = reply.send(r);
            }
            WorkflowCmd::ImportSkill { json, reply } => {
                let r = (|| -> Result<Skill> {
                    let skill: Skill = serde_json::from_str(&json)?;
                    skill_repo.save(&skill)
                })();
                let _ = reply.send(r);
            }
        }
    }
}

/// `WorkflowManager` is referenced by the API surface docs; silence the
/// unused-import warning in this module.
#[allow(dead_code)]
fn _mgr_ref(_: &WorkflowManager) {}
