use async_trait::async_trait;
use turso::{Value, params_from_iter};
use uuid::Uuid;

use crate::TursoAgentStorage;
use crate::memory::{
    MemoryEntry, MemoryNote, MemoryStage1Record, MemoryStore, MemorySummary, SemanticObservation,
};
use crate::storage::{AgentStorage, StorageError};

use super::{int_col, text_col};

fn uuid_value(id: Uuid) -> Value {
    Value::Text(id.to_string())
}

fn real_col(row: &turso::Row, idx: usize) -> Result<f64, StorageError> {
    match row.get_value(idx)? {
        Value::Real(value) => Ok(value),
        Value::Integer(value) => Ok(value as f64),
        other => Err(StorageError::Other(
            format!("expected REAL at column {idx}, got {other:?}").into(),
        )),
    }
}

fn row_to_entry(row: &turso::Row) -> Result<MemoryEntry, StorageError> {
    Ok(MemoryEntry {
        id: Uuid::parse_str(&text_col(row, 0)?).map_err(|e| StorageError::Other(e.into()))?,
        scope_id: Uuid::parse_str(&text_col(row, 1)?).map_err(|e| StorageError::Other(e.into()))?,
        entry_type: text_col(row, 2)?,
        title: text_col(row, 3)?,
        body_md: text_col(row, 4)?,
        status: text_col(row, 5)?,
        confidence: real_col(row, 6)?,
        created_at: int_col(row, 7)?,
        updated_at: int_col(row, 8)?,
    })
}

fn row_to_note(row: &turso::Row) -> Result<MemoryNote, StorageError> {
    Ok(MemoryNote {
        id: Uuid::parse_str(&text_col(row, 0)?).map_err(|e| StorageError::Other(e.into()))?,
        scope_id: Uuid::parse_str(&text_col(row, 1)?).map_err(|e| StorageError::Other(e.into()))?,
        slug: text_col(row, 2)?,
        content: text_col(row, 3)?,
        status: text_col(row, 4)?,
        created_at: int_col(row, 5)?,
    })
}

fn row_to_observation(row: &turso::Row) -> Result<SemanticObservation, StorageError> {
    Ok(SemanticObservation {
        id: Uuid::parse_str(&text_col(row, 0)?).map_err(|e| StorageError::Other(e.into()))?,
        scope_id: Uuid::parse_str(&text_col(row, 1)?).map_err(|e| StorageError::Other(e.into()))?,
        subject: text_col(row, 2)?,
        predicate: text_col(row, 3)?,
        object: text_col(row, 4)?,
        content: text_col(row, 5)?,
        status: text_col(row, 6)?,
        confidence: real_col(row, 7)?,
        source_hash: text_col(row, 8)?,
        created_at: int_col(row, 9)?,
        last_error: match row.get_value(10)? {
            Value::Text(error) if !error.is_empty() => Some(error),
            _ => None,
        },
    })
}

#[async_trait]
impl MemoryStore for TursoAgentStorage {
    async fn get_stage1_output(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
    ) -> Result<Option<MemoryStage1Record>, StorageError> {
        self.get_memory_stage1_output(scope_id, session_id).await
    }

    async fn claim_stage1(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> Result<bool, StorageError> {
        self.claim_memory_stage1(scope_id, session_id, source_hash, lease_until)
            .await
    }

    async fn complete_stage1(
        &self,
        scope_id: Uuid,
        output: MemoryStage1Record,
    ) -> Result<(), StorageError> {
        self.complete_memory_stage1(scope_id, output).await
    }

    async fn fail_stage1(
        &self,
        scope_id: Uuid,
        session_id: Uuid,
        source_hash: &str,
        error: &str,
    ) -> Result<(), StorageError> {
        self.fail_memory_stage1(scope_id, session_id, source_hash, error)
            .await
    }

    async fn list_stage1_outputs(
        &self,
        scope_id: Uuid,
        limit: usize,
    ) -> Result<Vec<MemoryStage1Record>, StorageError> {
        self.list_memory_stage1_outputs(scope_id, limit).await
    }

    async fn claim_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        lease_until: i64,
    ) -> Result<bool, StorageError> {
        self.claim_memory_phase2(scope_id, source_hash, lease_until)
            .await
    }

    async fn complete_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        entries: Vec<MemoryEntry>,
        summary_md: &str,
        observations: Vec<SemanticObservation>,
        consumed_note_ids: Vec<Uuid>,
    ) -> Result<(), StorageError> {
        let now = crate::memory::now_ms();
        self.conn.execute("BEGIN", ()).await?;
        let result = async {
            self.conn
                .execute(
                    "UPDATE memory_entries SET status = 'superseded', updated_at = ?1
                     WHERE scope_id = ?2 AND status = 'active'",
                    params_from_iter([Value::Integer(now), uuid_value(scope_id)]),
                )
                .await?;

            for entry in entries {
                self.conn
                    .execute(
                        "INSERT INTO memory_entries
                            (id, scope_id, entry_type, title, body_md, status,
                             confidence, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?7)",
                        params_from_iter([
                            uuid_value(entry.id),
                            uuid_value(scope_id),
                            Value::Text(entry.entry_type),
                            Value::Text(entry.title),
                            Value::Text(entry.body_md),
                            Value::Real(entry.confidence),
                            Value::Integer(now),
                        ]),
                    )
                    .await?;
            }

            self.conn
                .execute(
                    "INSERT INTO memory_summaries
                        (scope_id, schema_version, summary_md, source_hash, generated_at)
                     VALUES (?1, 'v1', ?2, ?3, ?4)
                     ON CONFLICT(scope_id) DO UPDATE SET
                        schema_version = 'v1',
                        summary_md = excluded.summary_md,
                        source_hash = excluded.source_hash,
                        generated_at = excluded.generated_at",
                    params_from_iter([
                        uuid_value(scope_id),
                        Value::Text(summary_md.to_string()),
                        Value::Text(source_hash.to_string()),
                        Value::Integer(now),
                    ]),
                )
                .await?;

            for observation in observations {
                let mut existing_rows = self
                    .conn
                    .query(
                        "SELECT id FROM memory_semantic_observations
                         WHERE scope_id = ?1 AND subject = ?2 AND predicate = ?3
                           AND object = ?4",
                        params_from_iter([
                            uuid_value(scope_id),
                            Value::Text(observation.subject.clone()),
                            Value::Text(observation.predicate.clone()),
                            Value::Text(observation.object.clone()),
                        ]),
                    )
                    .await?;
                let existing_id = match existing_rows.next().await? {
                    Some(row) => Uuid::parse_str(&text_col(&row, 0)?)
                        .map_err(|e| StorageError::Other(e.into()))?,
                    None => Uuid::nil(),
                };
                if existing_id != Uuid::nil() {
                    self.conn
                        .execute(
                            "UPDATE memory_semantic_observations SET
                                content = ?1,
                                status = 'candidate',
                                confidence = ?2,
                                source_hash = ?3,
                                created_at = ?4,
                                last_error = NULL
                             WHERE id = ?5 AND scope_id = ?6",
                            params_from_iter([
                                Value::Text(observation.content),
                                Value::Real(observation.confidence),
                                Value::Text(observation.source_hash),
                                Value::Integer(observation.created_at),
                                uuid_value(existing_id),
                                uuid_value(scope_id),
                            ]),
                        )
                        .await?;
                    continue;
                }

                self.conn
                    .execute(
                        "INSERT INTO memory_semantic_observations
                            (id, scope_id, subject, predicate, object, content, status,
                             confidence, source_hash, created_at, last_error)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'candidate', ?7, ?8, ?9, NULL)",
                        params_from_iter([
                            uuid_value(observation.id),
                            uuid_value(scope_id),
                            Value::Text(observation.subject),
                            Value::Text(observation.predicate),
                            Value::Text(observation.object),
                            Value::Text(observation.content),
                            Value::Real(observation.confidence),
                            Value::Text(observation.source_hash),
                            Value::Integer(now),
                        ]),
                    )
                    .await?;
            }

            for note_id in consumed_note_ids {
                self.conn
                    .execute(
                        "UPDATE memory_notes SET status = 'consumed'
                         WHERE id = ?1 AND scope_id = ?2",
                        params_from_iter([uuid_value(note_id), uuid_value(scope_id)]),
                    )
                    .await?;
            }

            self.conn
                .execute(
                    "INSERT INTO memory_jobs
                        (scope_id, status, source_hash, lease_until, attempts, updated_at)
                     VALUES (?1, 'succeeded', ?2, 0, 1, ?3)
                     ON CONFLICT(scope_id) DO UPDATE SET
                        status = 'succeeded',
                        source_hash = excluded.source_hash,
                        lease_until = 0,
                        last_error = NULL,
                        updated_at = excluded.updated_at",
                    params_from_iter([
                        uuid_value(scope_id),
                        Value::Text(source_hash.to_string()),
                        Value::Integer(now),
                    ]),
                )
                .await?;
            Ok(())
        }
        .await;

        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", ()).await?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn fail_phase2(
        &self,
        scope_id: Uuid,
        source_hash: &str,
        error: &str,
    ) -> Result<(), StorageError> {
        self.fail_memory_phase2(scope_id, source_hash, error).await
    }

    async fn get_summary(&self, scope_id: Uuid) -> Result<Option<MemorySummary>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT scope_id, schema_version, summary_md, source_hash, generated_at
                 FROM memory_summaries WHERE scope_id = ?1",
                params_from_iter([uuid_value(scope_id)]),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(MemorySummary {
                scope_id: Uuid::parse_str(&text_col(&row, 0)?)
                    .map_err(|e| StorageError::Other(e.into()))?,
                schema_version: text_col(&row, 1)?,
                summary_md: text_col(&row, 2)?,
                source_hash: text_col(&row, 3)?,
                generated_at: int_col(&row, 4)?,
            })),
            None => Ok(None),
        }
    }

    async fn list_entries(
        &self,
        scope_id: Uuid,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, scope_id, entry_type, title, body_md, status,
                        confidence, created_at, updated_at
                 FROM memory_entries
                 WHERE scope_id = ?1 AND status = 'active'
                 ORDER BY updated_at DESC, title ASC
                 LIMIT ?2",
                params_from_iter([uuid_value(scope_id), Value::Integer(limit as i64)]),
            )
            .await?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().await? {
            entries.push(row_to_entry(&row)?);
        }
        Ok(entries)
    }

    async fn get_entry(
        &self,
        scope_id: Uuid,
        entry_id: Uuid,
    ) -> Result<Option<MemoryEntry>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, scope_id, entry_type, title, body_md, status,
                        confidence, created_at, updated_at
                 FROM memory_entries WHERE scope_id = ?1 AND id = ?2",
                params_from_iter([uuid_value(scope_id), uuid_value(entry_id)]),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row_to_entry(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_pending_notes(&self, scope_id: Uuid) -> Result<Vec<MemoryNote>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, scope_id, slug, content, status, created_at
                 FROM memory_notes
                 WHERE scope_id = ?1 AND status = 'pending'
                 ORDER BY created_at ASC, id ASC",
                params_from_iter([uuid_value(scope_id)]),
            )
            .await?;
        let mut notes = Vec::new();
        while let Some(row) = rows.next().await? {
            notes.push(row_to_note(&row)?);
        }
        Ok(notes)
    }

    async fn insert_note(
        &self,
        scope_id: Uuid,
        slug: &str,
        content: &str,
    ) -> Result<MemoryNote, StorageError> {
        let note = MemoryNote {
            id: Uuid::new_v4(),
            scope_id,
            slug: slug.to_string(),
            content: content.to_string(),
            status: "pending".to_string(),
            created_at: crate::memory::now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO memory_notes
                    (id, scope_id, slug, content, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params_from_iter([
                    uuid_value(note.id),
                    uuid_value(note.scope_id),
                    Value::Text(note.slug.clone()),
                    Value::Text(note.content.clone()),
                    Value::Integer(note.created_at),
                ]),
            )
            .await?;
        Ok(note)
    }

    async fn list_observations(
        &self,
        scope_id: Uuid,
        status: &str,
        limit: usize,
    ) -> Result<Vec<SemanticObservation>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, scope_id, subject, predicate, object, content, status,
                        confidence, source_hash, created_at, last_error
                 FROM memory_semantic_observations
                 WHERE scope_id = ?1 AND status = ?2
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?3",
                params_from_iter([
                    uuid_value(scope_id),
                    Value::Text(status.to_string()),
                    Value::Integer(limit as i64),
                ]),
            )
            .await?;
        let mut observations = Vec::new();
        while let Some(row) = rows.next().await? {
            observations.push(row_to_observation(&row)?);
        }
        Ok(observations)
    }

    async fn set_observation_status(
        &self,
        scope_id: Uuid,
        observation_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), StorageError> {
        let changed = self
            .conn
            .execute(
                "UPDATE memory_semantic_observations
                 SET status = ?1, last_error = ?2
                 WHERE scope_id = ?3 AND id = ?4",
                params_from_iter([
                    Value::Text(status.to_string()),
                    error
                        .map(|message| Value::Text(message.to_string()))
                        .unwrap_or(Value::Null),
                    uuid_value(scope_id),
                    uuid_value(observation_id),
                ]),
            )
            .await?;
        if changed == 0 {
            return Err(StorageError::NotFound(format!(
                "semantic observation {observation_id}"
            )));
        }
        Ok(())
    }
}
