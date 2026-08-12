//! `WorkflowCmd` enum — every command the API actor handles.
//!
//! Each variant carries a `oneshot::Sender<WfResult<T>>` for the reply so the
//! actor can hand results back to the caller without buffering or polling.
//! Pattern follows `data-engine/src/runtime/types.rs::DataEngineCmd`.

use crate::error::Result;
use crate::model::{NodeEntry, EdgeEntry, WorkflowManifest, Skill, SnapshotInfo};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// One command sent to the per-session actor.
///
/// `reply` is set on every variant; the actor fills it exactly once (success
/// or failure) before moving on.
#[derive(Debug)]
pub enum WorkflowCmd {
    /// Create a brand-new workflow row + initial snapshot.
    CreateWorkflow {
        /// The initial manifest (id will be regenerated).
        manifest: WorkflowManifest,
        /// Reply channel.
        reply: oneshot::Sender<Result<Uuid>>,
    },

    /// Load a workflow by id.
    LoadWorkflow {
        /// Target workflow id.
        id: Uuid,
        /// Reply channel.
        reply: oneshot::Sender<Result<WorkflowManifest>>,
    },

    /// Save a new snapshot for an existing workflow.
    SaveWorkflow {
        /// Target workflow id.
        id: Uuid,
        /// New manifest to persist.
        manifest: WorkflowManifest,
        /// Snapshot commit message.
        message: String,
        /// Reply channel.
        reply: oneshot::Sender<Result<Uuid>>,
    },

    /// Append a node to a workflow.
    AddNode {
        /// Workflow id.
        workflow_id: Uuid,
        /// Node to add.
        node: NodeEntry,
        /// Reply channel.
        reply: oneshot::Sender<Result<()>>,
    },

    /// Remove a node (and any connected edges) from a workflow.
    RemoveNode {
        /// Workflow id.
        workflow_id: Uuid,
        /// Node id to remove.
        node_id: Uuid,
        /// Reply channel.
        reply: oneshot::Sender<Result<()>>,
    },

    /// Add an edge between two nodes.
    AddEdge {
        /// Workflow id.
        workflow_id: Uuid,
        /// Edge to add.
        edge: EdgeEntry,
        /// Reply channel.
        reply: oneshot::Sender<Result<()>>,
    },

    /// Remove an edge.
    RemoveEdge {
        /// Workflow id.
        workflow_id: Uuid,
        /// Edge id to remove.
        edge_id: Uuid,
        /// Reply channel.
        reply: oneshot::Sender<Result<()>>,
    },

    /// Validate a manifest's structural + topological invariants.
    Validate {
        /// Manifest to validate.
        manifest: WorkflowManifest,
        /// Reply channel — [`ValidationReport`](crate::executor::ValidationReport) on Ok.
        reply: oneshot::Sender<Result<crate::executor::ValidationReport>>,
    },

    /// Execute a workflow end-to-end.
    Run {
        /// Workflow id.
        workflow_id: Uuid,
        /// Inputs keyed by surface input node id.
        inputs: serde_json::Map<String, serde_json::Value>,
        /// Reply channel — [`WorkflowResult`](crate::executor::WorkflowResult) on Ok.
        reply: oneshot::Sender<Result<crate::executor::WorkflowResult>>,
        /// Cooperative cancellation.
        cancel: CancellationToken,
    },

    /// Save a skill (new version).
    SaveSkill {
        /// Skill to persist.
        skill: Skill,
        /// Reply channel — returns the saved Skill (with id + version).
        reply: oneshot::Sender<Result<Skill>>,
    },

    /// List workflows (summary).
    ListWorkflows {
        /// Reply channel.
        reply: oneshot::Sender<Result<Vec<crate::store::WorkflowSummary>>>,
    },

    /// List skills (latest version each).
    ListSkills {
        /// Reply channel.
        reply: oneshot::Sender<Result<Vec<crate::model::SkillInfo>>>,
    },

    /// History (snapshots) for a workflow.
    History {
        /// Workflow id.
        workflow_id: Uuid,
        /// Reply channel.
        reply: oneshot::Sender<Result<Vec<SnapshotInfo>>>,
    },

    /// Restore the workflow's *current* state to a prior snapshot.
    Checkout {
        /// Workflow id.
        workflow_id: Uuid,
        /// Snapshot id to restore.
        snapshot_id: Uuid,
        /// Reply channel.
        reply: oneshot::Sender<Result<()>>,
    },

    /// Serialize a skill to JSON for export.
    ExportSkill {
        /// Skill id.
        skill_id: Uuid,
        /// Reply channel — JSON-encoded skill on Ok.
        reply: oneshot::Sender<Result<String>>,
    },

    /// Parse a JSON skill and save it (new version).
    ImportSkill {
        /// JSON-encoded skill.
        json: String,
        /// Reply channel — saved Skill on Ok.
        reply: oneshot::Sender<Result<Skill>>,
    },
}