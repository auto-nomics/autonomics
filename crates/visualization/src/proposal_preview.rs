//! Demand-driven browser renders of immutable proposal documents, independent of the figure head.
use crate::proposal::document_hash as hash;
use crate::{Error, Result, Store, preview::validate_png_preview};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Serialize, Deserialize)]
pub struct CandidatePreview {
    pub scope: String,
    pub proposal_id: String,
    pub visualization_id: String,
    pub base_revision: i64,
    pub current_revision: i64,
    pub base_is_current: bool,
    pub proposal_status: String,
    pub document_sha256: String,
    pub render_status: String,
    pub renderer: Option<String>,
    pub error: Option<String>,
    #[serde(rename = "dataURL", skip_serializing_if = "Option::is_none")]
    pub data_url: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct RenderRequest {
    pub proposal_id: String,
    pub base_revision: i64,
    pub document_sha256: String,
    pub document: Value,
}
impl Store {
    pub fn request_proposal_preview(
        &self,
        project: &str,
        id: &str,
        proposal: &str,
        expected_revision: i64,
        retry_failed: bool,
    ) -> Result<CandidatePreview> {
        let transaction =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if self.get(project, id)?.revision != expected_revision {
            return Err(Error::Conflict);
        }
        let document: String = transaction
            .query_row(
                "SELECT document FROM visualization_proposals WHERE id=?1 AND visualization_id=?2",
                params![proposal, id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(Error::NotFound)?;
        let state: Option<String> = transaction
            .query_row(
                "SELECT render_status FROM visualization_proposal_previews WHERE proposal_id=?1",
                [proposal],
                |r| r.get(0),
            )
            .optional()?;
        if state.is_none() || (retry_failed && state.as_deref() == Some("failed")) {
            let pending:i64=transaction.query_row("SELECT COUNT(*) FROM visualization_proposal_previews v JOIN visualization_proposals p ON p.id=v.proposal_id WHERE p.visualization_id=?1 AND v.render_status='requested'",[id],|r|r.get(0))?;
            if pending >= 16 {
                return Err(Error::Invalid("too many pending proposal preview requests"));
            }
            transaction.execute("INSERT INTO visualization_proposal_previews(proposal_id,document_sha256,render_status,requested_at) VALUES(?1,?2,'requested',?3) ON CONFLICT(proposal_id) DO UPDATE SET render_status='requested',error=NULL,renderer=NULL,requested_at=excluded.requested_at WHERE visualization_proposal_previews.render_status='failed'",params![proposal,hash(&document),chrono::Utc::now().to_rfc3339()])?;
        }
        transaction.commit()?;
        self.proposal_preview(project, id, proposal, expected_revision)
    }
    pub fn proposal_preview(
        &self,
        project: &str,
        id: &str,
        proposal: &str,
        expected_revision: i64,
    ) -> Result<CandidatePreview> {
        let head = self.get(project, id)?;
        if head.revision != expected_revision {
            return Err(Error::Conflict);
        }
        self.connection.query_row("SELECT p.base_revision,p.status,v.document_sha256,v.render_status,v.renderer,v.error,v.data_url FROM visualization_proposals p JOIN visualization_proposal_previews v ON v.proposal_id=p.id WHERE p.id=?1 AND p.visualization_id=?2",params![proposal,id],|r|{
            let base_revision=r.get(0)?;
            Ok(CandidatePreview{scope:"proposal_candidate".into(),proposal_id:proposal.into(),visualization_id:id.into(),base_revision,current_revision:head.revision,base_is_current:base_revision==head.revision,proposal_status:r.get(1)?,document_sha256:r.get(2)?,render_status:r.get(3)?,renderer:r.get(4)?,error:r.get(5)?,data_url:r.get(6)?})
        }).optional()?.ok_or(Error::NotFound)
    }
    pub fn proposal_render_requests(&self, project: &str, id: &str) -> Result<Vec<RenderRequest>> {
        self.get(project, id)?;
        let mut statement=self.connection.prepare("SELECT p.id,p.base_revision,v.document_sha256,p.document FROM visualization_proposals p JOIN visualization_proposal_previews v ON v.proposal_id=p.id WHERE p.visualization_id=?1 AND v.render_status='requested' ORDER BY v.requested_at,p.id LIMIT 2")?;
        let rows = statement.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (proposal_id, base_revision, document_sha256, document) = row?;
            if hash(&document) != document_sha256 {
                return Err(Error::Conflict);
            }
            Ok(RenderRequest {
                proposal_id,
                base_revision,
                document_sha256,
                document: serde_json::from_str(&document)?,
            })
        })
        .collect()
    }
    /// First successful render wins. Repeated identical delivery is idempotent.
    pub fn complete_proposal_preview(
        &self,
        project: &str,
        id: &str,
        proposal: &str,
        completion: PreviewCompletion<'_>,
    ) -> Result<()> {
        self.get(project, id)?;
        if completion.renderer.trim().is_empty() || completion.renderer.len() > 120 {
            return Err(Error::Invalid("preview renderer"));
        }
        match (completion.data_url, completion.error) {
            (Some(png), None) => validate_png_preview(png)?,
            (None, Some(error)) if !error.trim().is_empty() && error.len() <= 200 => {}
            _ => {
                return Err(Error::Invalid(
                    "exactly one preview PNG or bounded render error is required",
                ));
            }
        }
        let transaction =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let row=transaction.query_row("SELECT p.base_revision,p.document,v.document_sha256,v.render_status,v.data_url FROM visualization_proposals p JOIN visualization_proposal_previews v ON v.proposal_id=p.id WHERE p.id=?1 AND p.visualization_id=?2",params![proposal,id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?))).optional()?.ok_or(Error::NotFound)?;
        if row.0 != completion.base_revision
            || row.2 != completion.document_sha256
            || hash(&row.1) != row.2
        {
            return Err(Error::Conflict);
        }
        if row.3 == "ready" {
            return if row.4.as_deref() == completion.data_url {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        if row.3 != "requested" {
            return Err(Error::Conflict);
        }
        transaction.execute("UPDATE visualization_proposal_previews SET render_status=?1,data_url=?2,error=?3,renderer=?4 WHERE proposal_id=?5",params![if completion.data_url.is_some(){"ready"}else{"failed"},completion.data_url,completion.error,completion.renderer,proposal])?;
        transaction.commit()?;
        Ok(())
    }
}
pub struct PreviewCompletion<'a> {
    pub base_revision: i64,
    pub document_sha256: &'a str,
    pub data_url: Option<&'a str>,
    pub error: Option<&'a str>,
    pub renderer: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
    fn fixture() -> (Store, String, crate::proposal::Proposal) {
        let mut store = Store::in_memory().unwrap();
        store.create_project("p", "Study").unwrap();
        store.create_project("other", "Other").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
            .unwrap();
        let proposal = store
            .propose(
                "p",
                &figure.id,
                1,
                "Candidate",
                &json!({"elements":[{"id":"candidate","type":"rectangle"}]}),
            )
            .unwrap();
        (store, figure.id, proposal)
    }
    #[test]
    fn candidates_have_separate_immutable_previews_across_adoption_and_project_scopes() {
        let (mut store, figure, p) = fixture();
        let requested = store
            .request_proposal_preview("p", &figure, &p.id, 1, false)
            .unwrap();
        assert_eq!(requested.render_status, "requested");
        assert!(requested.data_url.is_none());
        let jobs = store.proposal_render_requests("p", &figure).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].document, p.document);
        assert_eq!(
            jobs[0].document_sha256,
            hash(&serde_json::to_string(&p.document).unwrap())
        );
        let completion = || PreviewCompletion {
            base_revision: 1,
            document_sha256: &requested.document_sha256,
            data_url: Some(PNG),
            error: None,
            renderer: "test-renderer",
        };
        assert!(matches!(
            store.complete_proposal_preview("other", &figure, &p.id, completion()),
            Err(Error::NotFound)
        ));
        let mut wrong = completion();
        wrong.document_sha256 = "wrong-hash";
        assert!(matches!(
            store.complete_proposal_preview("p", &figure, &p.id, wrong),
            Err(Error::Conflict)
        ));
        store
            .complete_proposal_preview("p", &figure, &p.id, completion())
            .unwrap();
        store
            .complete_proposal_preview("p", &figure, &p.id, completion())
            .unwrap();
        assert!(
            store
                .proposal_render_requests("p", &figure)
                .unwrap()
                .is_empty()
        );
        let ready = store.proposal_preview("p", &figure, &p.id, 1).unwrap();
        assert_eq!(ready.scope, "proposal_candidate");
        assert!(ready.base_is_current);
        assert_eq!(ready.data_url.as_deref(), Some(PNG));
        assert!(
            matches!(store.preview("p", &figure, 1), Err(Error::NotFound)),
            "candidate cache is not the saved-canvas cache"
        );
        assert_eq!(store.get("p", &figure).unwrap().revision, 1);
        use base64::Engine;
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(PNG.split_once(',').unwrap().1)
            .unwrap();
        bytes.extend_from_slice(b"different transport bytes");
        let different = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let mut overwrite = completion();
        overwrite.data_url = Some(&different);
        assert!(matches!(
            store.complete_proposal_preview("p", &figure, &p.id, overwrite),
            Err(Error::Conflict)
        ));
        let mut late_error = completion();
        late_error.data_url = None;
        late_error.error = Some("late failure");
        assert!(matches!(
            store.complete_proposal_preview("p", &figure, &p.id, late_error),
            Err(Error::Conflict)
        ));
        store.accept_proposal("p", &figure, &p.id).unwrap();
        let historical = store.proposal_preview("p", &figure, &p.id, 2).unwrap();
        assert_eq!(historical.base_revision, 1);
        assert_eq!(historical.current_revision, 2);
        assert!(!historical.base_is_current);
        assert_eq!(historical.proposal_status, "accepted");
        assert_eq!(historical.data_url.as_deref(), Some(PNG));
        assert!(matches!(
            store.proposal_preview("p", &figure, &p.id, 1),
            Err(Error::Conflict)
        ));
        assert_eq!(
            store
                .request_proposal_preview("p", &figure, &p.id, 2, true)
                .unwrap()
                .render_status,
            "ready"
        );
        store.delete("p", &figure, 2).unwrap();
        let count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM visualization_proposal_previews",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    #[test]
    fn render_failure_requires_explicit_retry_and_bad_images_never_publish() {
        let (store, figure, p) = fixture();
        let request = store
            .request_proposal_preview("p", &figure, &p.id, 1, false)
            .unwrap();
        let failed = PreviewCompletion {
            base_revision: 1,
            document_sha256: &request.document_sha256,
            data_url: None,
            error: Some("missing image"),
            renderer: "test-renderer",
        };
        store
            .complete_proposal_preview("p", &figure, &p.id, failed)
            .unwrap();
        assert_eq!(
            store
                .request_proposal_preview("p", &figure, &p.id, 1, false)
                .unwrap()
                .render_status,
            "failed"
        );
        assert!(
            store
                .proposal_render_requests("p", &figure)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .request_proposal_preview("p", &figure, &p.id, 1, true)
                .unwrap()
                .render_status,
            "requested"
        );
        let invalid = PreviewCompletion {
            base_revision: 1,
            document_sha256: &request.document_sha256,
            data_url: Some("data:text/plain;base64,aGk="),
            error: None,
            renderer: "test-renderer",
        };
        assert!(
            store
                .complete_proposal_preview("p", &figure, &p.id, invalid)
                .is_err()
        );
        assert_eq!(
            store
                .proposal_preview("p", &figure, &p.id, 1)
                .unwrap()
                .render_status,
            "requested"
        );
        assert!(store.proposal_preview("other", &figure, &p.id, 1).is_err());
    }
    #[test]
    fn queue_is_bounded_and_does_not_cross_figures() {
        let (mut store, figure, p) = fixture();
        let other = store
            .create(
                "p",
                "Other figure",
                "excalidraw/v2",
                &json!({"elements":[]}),
            )
            .unwrap();
        assert!(matches!(
            store.request_proposal_preview("p", &other.id, &p.id, 1, false),
            Err(Error::NotFound)
        ));
        for index in 0..16 {
            let p = store
                .propose(
                    "p",
                    &figure,
                    1,
                    "Candidate",
                    &json!({"elements":[],"index":index}),
                )
                .unwrap();
            store
                .request_proposal_preview("p", &figure, &p.id, 1, false)
                .unwrap();
        }
        assert_eq!(
            store.proposal_render_requests("p", &figure).unwrap().len(),
            2
        );
        assert!(matches!(
            store.request_proposal_preview("p", &figure, &p.id, 1, false),
            Err(Error::Invalid(_))
        ));
        assert!(
            store
                .proposal_render_requests("p", &other.id)
                .unwrap()
                .is_empty()
        );
    }
}
