//! Versioned scientific figure briefs and explicit clarification before sketching.
use crate::{Error, Result, Store, proposal::Proposal, sketch};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

fn horizontal() -> sketch::Layout {
    sketch::Layout::Horizontal
}
#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub answer: String,
}
#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Brief {
    pub message: String,
    pub audience: String,
    pub layout: sketch::Layout,
    pub nodes: Vec<sketch::Node>,
    pub edges: Vec<sketch::Edge>,
    pub assumptions: Vec<String>,
    pub relationship_note: String,
    pub questions: Vec<Question>,
}
impl Default for Brief {
    fn default() -> Self {
        Self {
            message: String::new(),
            audience: String::new(),
            layout: horizontal(),
            nodes: Vec::new(),
            edges: Vec::new(),
            assumptions: Vec::new(),
            relationship_note: String::new(),
            questions: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub field: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Planning {
    pub version: i64,
    pub based_on_revision: i64,
    pub document: Brief,
    pub issues: Vec<Issue>,
    pub status: String,
    pub proposal_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningRevision {
    pub version: i64,
    pub based_on_revision: i64,
    pub document: Brief,
    pub created_at: String,
}
impl Brief {
    fn plan(&self) -> sketch::Plan {
        sketch::Plan {
            message: self.message.clone(),
            audience: self.audience.clone(),
            layout: self.layout,
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            assumptions: self.assumptions.clone(),
        }
    }
    pub fn issues(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        let mut add = |field: &str, message: &str| {
            issues.push(Issue {
                field: field.into(),
                message: message.into(),
            })
        };
        if self.message.trim().is_empty() {
            add("message", "请明确这张图希望传达的核心信息。");
        }
        if self.audience.trim().is_empty() {
            add("audience", "请明确图稿面向的受众。");
        }
        if self.nodes.is_empty() {
            add("nodes", "请列出需要展示的组件。");
        }
        if self.nodes.len() > 1 && self.edges.is_empty() && self.relationship_note.trim().is_empty()
        {
            add(
                "relationships",
                "请说明组件之间的关系，或明确说明仅并列展示。",
            );
        }
        for question in &self.questions {
            if question.question.trim().is_empty() || question.answer.trim().is_empty() {
                add(
                    &format!("question:{}", question.id),
                    "还有待澄清的问题，请填写问题与答案。",
                );
            }
        }
        if !self.nodes.is_empty()
            && !self.message.trim().is_empty()
            && !self.audience.trim().is_empty()
        {
            if let Err(error) = sketch::compile(&self.plan()) {
                add("structure", &format!("请检查组件、关系和布局：{error}"));
            }
        }
        issues
    }
    fn bounded_json(&self) -> Result<String> {
        let document = serde_json::to_string(self)?;
        let mut ids = std::collections::HashSet::new();
        let mut node_ids = std::collections::HashSet::new();
        if self
            .nodes
            .iter()
            .any(|node| node.id.trim().is_empty() || !node_ids.insert(&node.id))
        {
            return Err(Error::Invalid(
                "planning component IDs must be nonempty and unique",
            ));
        }
        if document.len() > 256 * 1024
            || self.nodes.len() > 100
            || self.edges.len() > 200
            || self.questions.len() > 20
            || self.assumptions.len() > 50
            || self.questions.iter().any(|q| {
                q.id.trim().is_empty()
                    || !ids.insert(&q.id)
                    || q.question.len() > 16000
                    || q.answer.len() > 16000
            })
        {
            return Err(Error::Invalid("planning document size or question IDs"));
        }
        Ok(document)
    }
}
type Row = (i64, i64, String, Option<String>, String, String);
fn latest(connection: &Connection, id: &str) -> Result<Option<Planning>> {
    let row: Option<Row> = connection.query_row("SELECT version,based_on_revision,document,proposal_id,created_at,updated_at FROM visualization_plans WHERE visualization_id=?1",[id],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
    row.map(
        |(version, based_on_revision, document, proposal_id, created_at, updated_at)| {
            let document: Brief = serde_json::from_str(&document)?;
            let issues = document.issues();
            let status = if !issues.is_empty() {
                "needs_clarification"
            } else if proposal_id.is_some() {
                "proposed"
            } else {
                "ready"
            }
            .into();
            Ok(Planning {
                version,
                based_on_revision,
                document,
                issues,
                status,
                proposal_id,
                created_at,
                updated_at,
            })
        },
    )
    .transpose()
}
impl Store {
    pub fn planning(&self, project: &str, id: &str) -> Result<Option<Planning>> {
        self.get(project, id)?;
        latest(&self.connection, id)
    }
    /// Planning versions are independent of immutable artwork revisions.
    pub fn save_planning(
        &mut self,
        project: &str,
        id: &str,
        expected_revision: i64,
        expected_plan_version: i64,
        brief: &Brief,
    ) -> Result<Planning> {
        let document = brief.bounded_json()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let head: i64 = transaction
            .query_row(
                "SELECT revision FROM visualizations WHERE project_id=?1 AND id=?2",
                params![project, id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        if head != expected_revision {
            return Err(Error::Conflict);
        }
        let previous = latest(&transaction, id)?;
        let version = previous.as_ref().map_or(0, |p| p.version);
        if version != expected_plan_version {
            return Err(Error::Conflict);
        }
        let now = chrono::Utc::now().to_rfc3339();
        let created = previous
            .as_ref()
            .map_or(now.as_str(), |p| p.created_at.as_str());
        transaction.execute("INSERT INTO visualization_plans VALUES(?1,?2,?3,?4,NULL,?5,?6) ON CONFLICT(visualization_id) DO UPDATE SET version=excluded.version,based_on_revision=excluded.based_on_revision,document=excluded.document,proposal_id=NULL,updated_at=excluded.updated_at",params![id,version+1,expected_revision,document,created,now])?;
        transaction.execute(
            "INSERT INTO visualization_plan_history VALUES(?1,?2,?3,?4,?5)",
            params![id, version + 1, expected_revision, document, now],
        )?;
        let result = latest(&transaction, id)?.ok_or(Error::NotFound)?;
        transaction.commit()?;
        Ok(result)
    }
    pub fn planning_revision(
        &self,
        project: &str,
        id: &str,
        version: i64,
    ) -> Result<PlanningRevision> {
        self.get(project, id)?;
        let (based_on_revision,document,created_at):(i64,String,String)=self.connection.query_row("SELECT based_on_revision,document,created_at FROM visualization_plan_history WHERE visualization_id=?1 AND version=?2",params![id,version],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?.ok_or(Error::NotFound)?;
        Ok(PlanningRevision {
            version,
            based_on_revision,
            document: serde_json::from_str(&document)?,
            created_at,
        })
    }
    /// Repeated generation of the same ready version returns its pending proposal.
    pub fn planned_proposal(
        &mut self,
        project: &str,
        id: &str,
        expected_revision: i64,
        expected_plan_version: i64,
        summary: &str,
    ) -> Result<Proposal> {
        if summary.trim().is_empty() {
            return Err(Error::Invalid("planning proposal summary"));
        }
        let planning = self.planning(project, id)?.ok_or(Error::NotFound)?;
        if planning.version != expected_plan_version
            || planning.based_on_revision != expected_revision
        {
            return Err(Error::Conflict);
        }
        if !planning.issues.is_empty() {
            return Err(Error::Invalid(
                "planning is incomplete; review its issues before sketching",
            ));
        }
        let mut document = sketch::compile(&planning.document.plan())
            .map_err(|_| Error::Invalid("planning structure"))?;
        document["elements"][0]["customData"]["planningSource"] = serde_json::json!({"version":planning.version,"based_on_revision":planning.based_on_revision,"document":planning.document});
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let head: i64 = transaction
            .query_row(
                "SELECT revision FROM visualizations WHERE project_id=?1 AND id=?2",
                params![project, id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        let current = latest(&transaction, id)?.ok_or(Error::NotFound)?;
        if head != expected_revision || current.version != expected_plan_version {
            return Err(Error::Conflict);
        }
        if let Some(proposal_id) = current.proposal_id {
            let row:Option<(String,String,String,String)>=transaction.query_row("SELECT summary,document,status,created_at FROM visualization_proposals WHERE visualization_id=?1 AND id=?2",params![id,proposal_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
            if let Some((summary, document, status, created_at)) = row {
                if status == "pending" {
                    return Ok(Proposal {
                        id: proposal_id,
                        visualization_id: id.into(),
                        base_revision: head,
                        summary,
                        document: serde_json::from_str(&document)?,
                        status,
                        created_at,
                    });
                }
            }
        }
        let proposal = Proposal {
            id: uuid::Uuid::new_v4().to_string(),
            visualization_id: id.into(),
            base_revision: head,
            summary: summary.trim().into(),
            document,
            status: "pending".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        transaction.execute(
            "INSERT INTO visualization_proposals VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                proposal.id,
                id,
                head,
                proposal.summary,
                serde_json::to_string(&proposal.document)?,
                proposal.status,
                proposal.created_at
            ],
        )?;
        transaction.execute(
            "UPDATE visualization_plans SET proposal_id=?1,updated_at=?2 WHERE visualization_id=?3",
            params![proposal.id, proposal.created_at, id],
        )?;
        transaction.commit()?;
        Ok(proposal)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn clarification_versions_preserve_history_and_only_generate_ready_plans() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        store.create_project("other", "Other")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        assert!(store.planning("p", &figure.id)?.is_none());
        let mut brief = Brief {
            message: "A conceptual signaling example".into(),
            audience: "Researchers".into(),
            questions: vec![Question {
                id: "receptor".into(),
                question: "Which receptor should be shown?".into(),
                answer: String::new(),
            }],
            ..Brief::default()
        };
        let first = store.save_planning("p", &figure.id, 1, 0, &brief)?;
        assert_eq!(first.status, "needs_clarification");
        assert!(
            store
                .planned_proposal("p", &figure.id, 1, 1, "Sketch")
                .is_err()
        );
        assert!(store.proposals("p", &figure.id)?.is_empty());
        brief.questions[0].answer = "Use a generic receptor; no specific mechanism".into();
        brief.nodes = serde_json::from_value(
            json!([{"id":"a","label":"Ligand"},{"id":"b","label":"Receptor"}]),
        )?;
        assert!(
            brief
                .issues()
                .iter()
                .any(|issue| issue.field == "relationships")
        );
        brief.edges = serde_json::from_value(json!([{"from":"a","to":"b","label":"binds"}]))?;
        let ready = store.save_planning("p", &figure.id, 1, 1, &brief)?;
        assert_eq!(ready.status, "ready");
        assert_eq!(ready.version, 2);
        assert!(matches!(
            store.save_planning("p", &figure.id, 1, 1, &brief),
            Err(Error::Conflict)
        ));
        let proposal = store.planned_proposal("p", &figure.id, 1, 2, "Reviewed sketch")?;
        assert_eq!(
            store.planned_proposal("p", &figure.id, 1, 2, "Again")?.id,
            proposal.id
        );
        assert_eq!(store.get("p", &figure.id)?.revision, 1);
        assert_eq!(store.proposals("p", &figure.id)?.len(), 1);
        assert_eq!(store.planning("p", &figure.id)?.unwrap().status, "proposed");
        assert!(
            store
                .planning_revision("p", &figure.id, 1)?
                .document
                .questions[0]
                .answer
                .is_empty()
        );
        assert!(matches!(
            store.planning("other", &figure.id),
            Err(Error::NotFound)
        ));
        store.accept_proposal("p", &figure.id, &proposal.id)?;
        assert_eq!(
            store.revision("p", &figure.id, 2)?.document["elements"][0]["customData"]["planningSource"]
                ["version"],
            2
        );
        assert!(matches!(
            store.planned_proposal("p", &figure.id, 2, 2, "Stale plan"),
            Err(Error::Conflict)
        ));
        assert!(matches!(
            store.save_planning("p", &figure.id, 1, 2, &brief),
            Err(Error::Conflict)
        ));
        store.delete("p", &figure.id, 2)?;
        assert_eq!(
            store.connection.query_row(
                "SELECT count(*) FROM visualization_plan_history",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }
    #[test]
    fn invalid_questions_and_incomplete_relationships_are_explicit() -> Result<()> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let brief: Brief = serde_json::from_value(
            json!({"message":"Independent objects","audience":"Researchers","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"relationship_note":"Show components side by side, without a relationship."}),
        )?;
        assert!(brief.issues().is_empty());
        let mut invalid = brief.clone();
        invalid.nodes[1].id = invalid.nodes[0].id.clone();
        assert!(
            store
                .save_planning("p", &figure.id, 1, 0, &invalid)
                .is_err()
        );
        invalid = brief.clone();
        invalid.questions = vec![
            Question {
                id: "q".into(),
                question: "Why?".into(),
                answer: "".into()
            };
            2
        ];
        assert!(
            store
                .save_planning("p", &figure.id, 1, 0, &invalid)
                .is_err()
        );
        invalid = brief;
        invalid.edges = serde_json::from_value(json!([{"from":"a","to":"missing"}]))?;
        assert!(
            invalid
                .issues()
                .iter()
                .any(|issue| issue.field == "structure")
        );
        Ok(())
    }
}
