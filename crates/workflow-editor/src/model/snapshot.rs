//! Snapshot metadata returned by history queries.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lightweight info returned by `WorkflowClient::history`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct SnapshotInfo {
    /// Snapshot id.
    pub id: Uuid,
    /// Owning workflow id.
    pub workflow_id: Uuid,
    /// Previous snapshot id (None for the initial commit).
    pub parent_id: Option<Uuid>,
    /// blake3 hash of the manifest at this snapshot.
    pub manifest_hash: String,
    /// Free-form commit message.
    pub commit_message: String,
    /// When the snapshot was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}
