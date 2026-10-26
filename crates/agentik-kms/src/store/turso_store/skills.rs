use turso::{Value, params_from_iter};
use uuid::Uuid;

use super::{
    Error, Result, TursoKmsStorage, db_err, int, row_to_skill_artifact, row_to_skill_proposal,
    row_to_skill_version, text, uuid_value,
};
use crate::artifacts::KnowledgeSkillVersionInput;
use crate::knowledge::{Knowledge, KnowledgePath};
use crate::store::{SkillArtifact, SkillProposal, SkillVersion};

impl TursoKmsStorage {
    pub(super) async fn import_skill_version_inner(
        &self,
        input: KnowledgeSkillVersionInput,
    ) -> Result<Option<SkillVersion>> {
        let skill_name = input.skill_name.trim().to_string();
        let title = input.title.trim().to_string();
        let body_md = input.body_md.trim().to_string();
        let node_path = input.node_path.trim().to_string();
        let source_tier = input.source_tier.trim().to_lowercase();
        if skill_name.is_empty()
            || title.is_empty()
            || body_md.is_empty()
            || node_path.is_empty()
            || source_tier.is_empty()
        {
            return Err(Error::Storage(
                "skill name, title, body, node path, and source tier must not be empty".into(),
            ));
        }

        let content_hash = input.content_hash();
        let mut existing = self
            .conn
            .query(
                "SELECT id FROM skill_versions
                 WHERE skill_name = ?1 AND content_hash = ?2",
                params_from_iter([
                    Value::Text(skill_name.clone()),
                    Value::Text(content_hash.clone()),
                ]),
            )
            .await
            .map_err(db_err)?;
        if existing.next().await.map_err(db_err)?.is_some() {
            return Ok(None);
        }

        let metadata_json = serde_json::to_string(&input.metadata_json)
            .map_err(|e| Error::Storage(format!("serialize skill metadata: {e}")))?;
        let now = crate::now_ms();
        let id = Uuid::new_v4();
        self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
        let result = async {
            let mut max_version = self
                .conn
                .query(
                    "SELECT COALESCE(MAX(version), 0) FROM skill_versions WHERE skill_name = ?1",
                    params_from_iter([Value::Text(skill_name.clone())]),
                )
                .await
                .map_err(db_err)?;
            let version = match max_version.next().await.map_err(db_err)? {
                Some(row) => int(&row, 0)?,
                None => 0,
            } + 1;
            self.conn
                .execute(
                    "UPDATE skill_versions SET status = 'superseded'
                     WHERE skill_name = ?1 AND status = 'active'",
                    params_from_iter([Value::Text(skill_name.clone())]),
                )
                .await
                .map_err(db_err)?;
            self.conn
                .execute(
                    "INSERT INTO skill_versions
                        (id, skill_name, node_path, version, status, title, body_md,
                         metadata_json, content_hash, source_tier, source_path, created_at)
                     VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params_from_iter([
                        uuid_value(id),
                        Value::Text(skill_name.clone()),
                        Value::Text(node_path.clone()),
                        Value::Integer(version),
                        Value::Text(title.clone()),
                        Value::Text(body_md.clone()),
                        Value::Text(metadata_json),
                        Value::Text(content_hash.clone()),
                        Value::Text(source_tier.clone()),
                        input
                            .source_path
                            .clone()
                            .map(Value::Text)
                            .unwrap_or(Value::Null),
                        Value::Integer(now),
                    ]),
                )
                .await
                .map_err(db_err)?;
            for artifact in input.artifacts {
                let relative_path = artifact.relative_path.trim().to_string();
                if relative_path.is_empty() || artifact.content.trim().is_empty() {
                    return Err(Error::Storage(
                        "skill artifact path and content must not be empty".into(),
                    ));
                }
                let artifact_hash = crate::artifacts::hash_source(&artifact.content.trim());
                self.conn
                    .execute(
                        "INSERT INTO skill_artifacts
                            (id, skill_version_id, relative_path, content, content_hash, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params_from_iter([
                            uuid_value(Uuid::new_v4()),
                            uuid_value(id),
                            Value::Text(relative_path),
                            Value::Text(artifact.content.trim().to_string()),
                            Value::Text(artifact_hash),
                            Value::Integer(now),
                        ]),
                    )
                    .await
                    .map_err(db_err)?;
            }
            if let Some(path) = KnowledgePath::normalize(&node_path) {
                let node = Knowledge::leaf(
                    path,
                    title.clone(),
                    body_md.clone(),
                    1.0,
                    &content_hash,
                    now,
                );
                self.ensure_knowledge_node(&node).await?;
            }
            Ok(Some(SkillVersion {
                id,
                skill_name,
                node_path,
                version: u32::try_from(version.max(1))
                    .map_err(|e| Error::Storage(format!("skill version overflow: {e}")))?,
                status: "active".to_string(),
                title,
                body_md,
                metadata_json: String::new(),
                content_hash,
                source_tier,
                source_path: input.source_path,
                created_at: now,
            }))
        }
        .await;
        match result {
            Ok(_) => {
                self.conn.execute("COMMIT", ()).await.map_err(db_err)?;
                let mut rows = self
                    .conn
                    .query(
                        "SELECT id, skill_name, node_path, version, status, title, body_md,
                                metadata_json, content_hash, source_tier, source_path, created_at
                         FROM skill_versions WHERE id = ?1",
                        params_from_iter([uuid_value(id)]),
                    )
                    .await
                    .map_err(db_err)?;
                Ok(match rows.next().await.map_err(db_err)? {
                    Some(row) => Some(row_to_skill_version(&row)?),
                    None => None,
                })
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    pub(super) async fn list_skill_versions_inner(
        &self,
        limit: usize,
    ) -> Result<Vec<SkillVersion>> {
        self.collect(
            "SELECT id, skill_name, node_path, version, status, title, body_md,
                    metadata_json, content_hash, source_tier, source_path, created_at
             FROM skill_versions
             ORDER BY created_at DESC, skill_name ASC LIMIT ?1",
            vec![Value::Integer(limit as i64)],
            row_to_skill_version,
        )
        .await
    }

    pub(super) async fn list_skill_artifacts_inner(
        &self,
        skill_version_id: Uuid,
    ) -> Result<Vec<SkillArtifact>> {
        self.collect(
            "SELECT id, skill_version_id, relative_path, content, content_hash, created_at
             FROM skill_artifacts
             WHERE skill_version_id = ?1
             ORDER BY relative_path ASC",
            vec![uuid_value(skill_version_id)],
            row_to_skill_artifact,
        )
        .await
    }

    pub(super) async fn create_skill_proposal_inner(
        &self,
        input: crate::artifacts::KnowledgeSkillProposalInput,
    ) -> Result<Option<SkillProposal>> {
        let skill_name = input.skill_name.trim().to_string();
        let title = input.title.trim().to_string();
        let body_md = input.body_md.trim().to_string();
        let rationale = input.rationale.trim().to_string();
        if skill_name.is_empty()
            || title.is_empty()
            || body_md.is_empty()
            || rationale.is_empty()
            || input.metadata_json.as_object().is_none()
        {
            return Err(Error::Storage(
                "skill proposal requires name, title, body, rationale, and object metadata".into(),
            ));
        }
        let Some(path) = KnowledgePath::normalize(&input.node_path) else {
            return Err(Error::Storage(format!(
                "invalid skill proposal node path {}",
                input.node_path
            )));
        };
        let mut artifact_paths = input
            .artifacts
            .iter()
            .map(|artifact| artifact.relative_path.trim().to_string())
            .collect::<Vec<_>>();
        artifact_paths.sort();
        artifact_paths.dedup();
        if artifact_paths.len() != input.artifacts.len()
            || artifact_paths.iter().any(String::is_empty)
            || input
                .artifacts
                .iter()
                .any(|artifact| artifact.content.trim().is_empty())
        {
            return Err(Error::Storage(
                "skill proposal artifacts require unique nonempty paths and content".into(),
            ));
        }
        let mut source_evidence_ids = input
            .source_evidence_ids
            .iter()
            .map(|id| id.trim().to_string())
            .collect::<Vec<_>>();
        source_evidence_ids.sort();
        source_evidence_ids.dedup();
        if source_evidence_ids.iter().any(String::is_empty) {
            return Err(Error::Storage(
                "skill proposal evidence ids must not be empty".into(),
            ));
        }

        let content_hash = input.content_hash();
        let mut existing = self
            .conn
            .query(
                "SELECT id FROM skill_proposals
                 WHERE skill_name = ?1 AND content_hash = ?2",
                params_from_iter([
                    Value::Text(skill_name.clone()),
                    Value::Text(content_hash.clone()),
                ]),
            )
            .await
            .map_err(db_err)?;
        if existing.next().await.map_err(db_err)?.is_some() {
            return Ok(None);
        }

        let metadata_json = serde_json::to_string(&input.metadata_json)
            .map_err(|e| Error::Storage(format!("serialize skill proposal metadata: {e}")))?;
        let artifacts_json = serde_json::to_string(&input.artifacts)
            .map_err(|e| Error::Storage(format!("serialize skill proposal artifacts: {e}")))?;
        let id = Uuid::new_v4();
        let now = crate::now_ms();
        self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
        let result = self
            .conn
            .execute(
                "INSERT INTO skill_proposals
                    (id, skill_name, node_path, status, title, body_md, metadata_json,
                     rationale, source_evidence_ids_json, artifacts_json, content_hash,
                     approved_skill_version_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?12)",
                params_from_iter([
                    uuid_value(id),
                    Value::Text(skill_name),
                    Value::Text(path.as_str().to_string()),
                    Value::Text(title),
                    Value::Text(body_md),
                    Value::Text(metadata_json),
                    Value::Text(rationale),
                    Value::Text(serde_json::to_string(&source_evidence_ids).map_err(|e| {
                        Error::Storage(format!("serialize skill proposal evidence: {e}"))
                    })?),
                    Value::Text(artifacts_json),
                    Value::Text(content_hash),
                    Value::Integer(now),
                    Value::Integer(now),
                ]),
            )
            .await;
        match result {
            Ok(_) => {
                self.conn.execute("COMMIT", ()).await.map_err(db_err)?;
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                return Err(db_err(error));
            }
        }
        self.get_skill_proposal_inner(id).await
    }

    pub(super) async fn list_skill_proposals_inner(
        &self,
        limit: usize,
    ) -> Result<Vec<SkillProposal>> {
        self.collect(
            "SELECT id, skill_name, node_path, status, title, body_md, metadata_json,
                    rationale, source_evidence_ids_json, artifacts_json, content_hash,
                    approved_skill_version_id, created_at, updated_at
             FROM skill_proposals
             ORDER BY CASE status WHEN 'pending' THEN 0 ELSE 1 END, created_at DESC
             LIMIT ?1",
            vec![Value::Integer(limit as i64)],
            row_to_skill_proposal,
        )
        .await
    }

    pub(super) async fn get_skill_proposal_inner(&self, id: Uuid) -> Result<Option<SkillProposal>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, skill_name, node_path, status, title, body_md, metadata_json,
                        rationale, source_evidence_ids_json, artifacts_json, content_hash,
                        approved_skill_version_id, created_at, updated_at
                 FROM skill_proposals WHERE id = ?1",
                params_from_iter([uuid_value(id)]),
            )
            .await
            .map_err(db_err)?;
        Ok(match rows.next().await.map_err(db_err)? {
            Some(row) => Some(row_to_skill_proposal(&row)?),
            None => None,
        })
    }

    pub(super) async fn approve_skill_proposal_inner(
        &self,
        proposal_id: Uuid,
    ) -> Result<Option<SkillVersion>> {
        let Some(proposal) = self.get_skill_proposal_inner(proposal_id).await? else {
            return Ok(None);
        };
        if proposal.status != "pending" {
            return Ok(None);
        }
        let metadata_json = serde_json::from_str::<serde_json::Value>(&proposal.metadata_json)
            .map_err(|e| Error::Storage(format!("parse skill proposal metadata: {e}")))?;
        let version_input = KnowledgeSkillVersionInput {
            skill_name: proposal.skill_name.clone(),
            title: proposal.title.clone(),
            body_md: proposal.body_md.clone(),
            metadata_json,
            node_path: proposal.node_path.clone(),
            source_tier: "kms".to_string(),
            source_path: None,
            artifacts: proposal.artifacts.clone(),
        };
        let version_content_hash = version_input.content_hash();
        let version = self.import_skill_version_inner(version_input).await?.or({
            self.skill_version_by_content_hash_inner(&proposal.skill_name, &version_content_hash)
                .await?
        });
        let Some(version) = version else {
            return Err(Error::Storage(
                "approved skill proposal did not produce a skill version".into(),
            ));
        };
        self.conn
            .execute(
                "UPDATE skill_proposals
                 SET status = 'approved', approved_skill_version_id = ?1, updated_at = ?2
                 WHERE id = ?3 AND status = 'pending'",
                params_from_iter([
                    uuid_value(version.id),
                    Value::Integer(crate::now_ms()),
                    uuid_value(proposal_id),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(Some(version))
    }

    pub(super) async fn reject_skill_proposal_inner(&self, proposal_id: Uuid) -> Result<bool> {
        let Some(proposal) = self.get_skill_proposal_inner(proposal_id).await? else {
            return Ok(false);
        };
        if proposal.status != "pending" {
            return Ok(false);
        }
        let changed = self
            .conn
            .execute(
                "UPDATE skill_proposals SET status = 'rejected', updated_at = ?1
                 WHERE id = ?2 AND status = 'pending'",
                params_from_iter([Value::Integer(crate::now_ms()), uuid_value(proposal_id)]),
            )
            .await
            .map_err(db_err)?;
        Ok(changed > 0)
    }

    async fn skill_version_by_content_hash_inner(
        &self,
        skill_name: &str,
        content_hash: &str,
    ) -> Result<Option<SkillVersion>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, skill_name, node_path, version, status, title, body_md,
                        metadata_json, content_hash, source_tier, source_path, created_at
                 FROM skill_versions
                 WHERE skill_name = ?1 AND content_hash = ?2
                 ORDER BY version DESC LIMIT 1",
                params_from_iter([
                    Value::Text(skill_name.to_string()),
                    Value::Text(content_hash.to_string()),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(match rows.next().await.map_err(db_err)? {
            Some(row) => Some(row_to_skill_version(&row)?),
            None => None,
        })
    }
}
