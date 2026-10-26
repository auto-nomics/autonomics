use std::sync::Arc;
use std::time::Duration;

use agentik_core::memory::{
    KmsSystem, KnowledgeEvent, KnowledgeEvidenceInput, KnowledgePath, KnowledgeSkillArtifactInput,
    KnowledgeSkillVersionInput, MemoryStore,
};
use evolution_core::{ObservationHub, ObservationStorage};
use skills::{SkillIndexEntry, SkillManager};

const LEGACY_OBSERVATION_SOURCE: &str = "legacy_observation";
const LEGACY_OBSERVATION_SYNC_INTERVAL: Duration = Duration::from_secs(60);

/// Idempotically mirror legacy evolution observations into KMS evidence.
pub(super) async fn sync_legacy_observations_once(
    observations: &ObservationHub,
    kms: &KmsSystem,
) -> usize {
    let store = kms.store();
    let mut submitted = 0;
    for observation in observations.storage().list().await {
        let already_imported = match store
            .has_evidence_source(LEGACY_OBSERVATION_SOURCE, &observation.id)
            .await
        {
            Ok(true) => true,
            Ok(false) => false,
            Err(error) => {
                tracing::warn!(
                    observation_id = %observation.id,
                    error = %error,
                    "cannot check legacy observation import state"
                );
                continue;
            }
        };
        if already_imported {
            continue;
        }

        let input = KnowledgeEvidenceInput {
            source_kind: LEGACY_OBSERVATION_SOURCE.to_string(),
            source_id: observation.id.clone(),
            evidence_kind: observation.kind.label().to_string(),
            summary: observation.summary.clone(),
            body: observation.body.clone(),
            anchor_path: observation.node_kind.clone(),
            error: observation.error.clone(),
        };
        if let Err(error) = kms
            .submit_event(KnowledgeEvent::ObservationRecorded { input })
            .await
        {
            tracing::warn!(
                observation_id = %observation.id,
                error = %error,
                "cannot queue legacy observation for KMS ingestion"
            );
            continue;
        }
        submitted += 1;
    }
    submitted
}

pub(super) fn spawn_legacy_observation_sync(
    observations: Arc<ObservationHub>,
    kms: Arc<KmsSystem>,
) {
    tokio::spawn(async move {
        loop {
            let submitted = sync_legacy_observations_once(&observations, &kms).await;
            if submitted > 0 {
                tracing::info!(
                    submitted,
                    "legacy observations queued for KMS evidence ingestion"
                );
            }
            tokio::time::sleep(LEGACY_OBSERVATION_SYNC_INTERVAL).await;
        }
    });
}

/// Idempotently import visible skill documents and text artifacts into KMS.
pub(super) async fn sync_skills_once(manager: &SkillManager, kms: &KmsSystem) -> usize {
    let registry = manager.registry();
    let store = kms.store();
    let mut imported = 0;
    for entry in registry.list() {
        let Some(document) = registry.get(&entry.meta.name).ok().flatten() else {
            tracing::warn!(skill = %entry.meta.name, "cannot read skill for KMS import");
            continue;
        };

        let mut artifacts = Vec::new();
        if let Some(dir) = &document.dir {
            for stem in &document.workflows {
                let path = dir.join("workflow").join(format!("{stem}.toml"));
                match std::fs::read_to_string(&path) {
                    Ok(content) => artifacts.push(KnowledgeSkillArtifactInput {
                        relative_path: format!("workflow/{stem}.toml"),
                        content,
                    }),
                    Err(error) => tracing::warn!(
                        skill = %entry.meta.name,
                        path = %path.display(),
                        %error,
                        "cannot read skill workflow for KMS import"
                    ),
                }
            }
            for stem in &document.evals {
                let path = dir.join("evals").join(format!("{stem}.toml"));
                match std::fs::read_to_string(&path) {
                    Ok(content) => artifacts.push(KnowledgeSkillArtifactInput {
                        relative_path: format!("evals/{stem}.toml"),
                        content,
                    }),
                    Err(error) => tracing::warn!(
                        skill = %entry.meta.name,
                        path = %path.display(),
                        %error,
                        "cannot read skill eval for KMS import"
                    ),
                }
            }
        }

        let node_path = KnowledgePath::normalize(&format!("/procedures/{}", entry.meta.name))
            .unwrap_or_else(|| KnowledgePath::new(["procedures", "uncategorized"]))
            .as_str();
        let input = KnowledgeSkillVersionInput {
            skill_name: entry.meta.name.clone(),
            title: entry.meta.name.clone(),
            body_md: document.body.clone(),
            metadata_json: serde_json::json!({
                "description": entry.meta.description,
                "tags": entry.meta.tags,
                "tier": document.tier.as_str(),
            }),
            node_path,
            source_tier: document.tier.as_str().to_string(),
            source_path: document.dir.as_ref().map(|path| path.display().to_string()),
            artifacts,
        };
        match store.import_skill_version(input).await {
            Ok(Some(_)) => imported += 1,
            Ok(None) => {}
            Err(error) => tracing::warn!(
                skill = %entry.meta.name,
                %error,
                "cannot import skill version into KMS"
            ),
        }
    }
    imported
}

pub(super) fn spawn_kms_skill_sync(manager: Arc<SkillManager>, kms: Arc<KmsSystem>) {
    tokio::spawn(async move {
        loop {
            let imported = sync_skills_once(&manager, &kms).await;
            if imported > 0 {
                tracing::info!(imported, "new skill versions imported into KMS");
            }
            tokio::time::sleep(LEGACY_OBSERVATION_SYNC_INTERVAL).await;
        }
    });
}

/// Build the prompt index from KMS, or return `None` to use the registry.
pub(super) async fn kms_skill_prompt_section(kms: &KmsSystem) -> Option<String> {
    let store = kms.store();
    let versions = store.list_skill_versions(10_000).await.ok()?;
    let active = versions
        .iter()
        .filter(|version| version.status == "active")
        .collect::<Vec<_>>();
    if active.is_empty() {
        return None;
    }

    #[derive(serde::Deserialize)]
    struct Metadata {
        description: String,
    }

    let mut entries = Vec::new();
    for version in active {
        let Some(metadata) = serde_json::from_str::<Metadata>(&version.metadata_json).ok() else {
            tracing::warn!(
                skill = %version.skill_name,
                "skipping malformed KMS skill prompt metadata"
            );
            continue;
        };
        let has_workflows = store
            .list_skill_artifacts(version.id)
            .await
            .map(|artifacts| {
                artifacts
                    .iter()
                    .any(|artifact| artifact.relative_path.starts_with("workflow/"))
            })
            .unwrap_or_default();
        entries.push(SkillIndexEntry {
            name: version.skill_name.clone(),
            description: metadata.description,
            has_workflows,
        });
    }
    Some(skills::prompt_section_from_indices(&entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::memory::{EmptyMemorySource, MemoryConfig, TursoKmsStorage};
    use evolution_core::{ObservationInput, ObservationKind, ObservationSource};
    use skills::SkillTier;

    #[tokio::test]
    async fn legacy_observations_mirror_idempotently_into_kms() {
        let state = tempfile::tempdir().unwrap();
        let observations = ObservationHub::new(state.path());
        for i in 0..3 {
            observations
                .record(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: format!("failure {i}"),
                    body: format!("Use fix {i}."),
                    node_kind: Some("file_to_dataframe".to_string()),
                    error: Some("schema mismatch".to_string()),
                })
                .await
                .unwrap();
        }

        let storage = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let kms = KmsSystem::new(MemoryConfig::new(), storage, Arc::new(EmptyMemorySource)).await;
        assert_eq!(sync_legacy_observations_once(&observations, &kms).await, 3);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut evidence = kms.store().list_pending_evidence(10).await.unwrap();
        while evidence.len() < 3 {
            assert!(
                std::time::Instant::now() < deadline,
                "legacy observation bridge never ingested evidence"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            evidence = kms.store().list_pending_evidence(10).await.unwrap();
        }
        assert!(
            evidence
                .iter()
                .all(|item| item.source_kind == LEGACY_OBSERVATION_SOURCE)
        );

        // The source projection makes the next mirror pass a no-op.
        assert_eq!(sync_legacy_observations_once(&observations, &kms).await, 0);
        kms.shutdown();
    }

    #[tokio::test]
    async fn builtin_skills_import_idempotently_into_kms() {
        let state = tempfile::tempdir().unwrap();
        let manager = SkillManager::new(state.path());
        let storage = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let kms = KmsSystem::new(MemoryConfig::new(), storage, Arc::new(EmptyMemorySource)).await;

        assert_eq!(sync_skills_once(&manager, &kms).await, 1);
        let versions = kms.store().list_skill_versions(10).await.unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].skill_name, "skill-system");
        assert_eq!(versions[0].source_tier, SkillTier::Builtin.as_str());
        assert_eq!(sync_skills_once(&manager, &kms).await, 0);
        let prompt = kms_skill_prompt_section(&kms)
            .await
            .expect("KMS skill prompt section");
        assert!(prompt.contains("- skill-system"));
        kms.shutdown();
    }
}
