use async_trait::async_trait;
use uuid::Uuid;

use crate::knowledge::Knowledge;
use crate::store::KnowledgeDiagnosticFinding;
use crate::store::error::Result;

#[async_trait]
pub trait KnowledgeStore {
    async fn create_knowledge(&self, knowledge: Knowledge) -> Result<Knowledge>;

    async fn get_knowledge(&self, id: Uuid) -> Result<Option<Knowledge>>;

    async fn update_knowledge(&self, knowledge: Knowledge) -> Result<()>;

    async fn list_knowledge(&self, limit: usize) -> Result<Vec<Knowledge>>;

    async fn get_knowledge_by_path(&self, path: &str) -> Result<Option<Knowledge>>;

    async fn list_child_knowledge(&self, parent_path: &str) -> Result<Vec<Knowledge>>;

    async fn insert_diagnostic_finding(
        &self,
        finding: KnowledgeDiagnosticFinding,
    ) -> Result<KnowledgeDiagnosticFinding>;

    async fn list_diagnostic_findings(
        &self,
        target_path: &str,
        limit: usize,
    ) -> Result<Vec<KnowledgeDiagnosticFinding>>;

    async fn get_diagnostic_finding(
        &self,
        finding_id: Uuid,
    ) -> Result<Option<KnowledgeDiagnosticFinding>>;

    async fn set_diagnostic_finding_status(&self, finding_id: Uuid, status: &str) -> Result<()>;

    async fn apply_knowledge_repair(
        &self,
        nodes: Vec<Knowledge>,
        archive_paths: Vec<String>,
    ) -> Result<()>;
}
