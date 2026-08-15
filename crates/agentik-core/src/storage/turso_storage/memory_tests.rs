use agentik_sdk::types::messages::Message;
use uuid::Uuid;

use super::TursoAgentStorage;
use crate::memory::MemoryStage1Record;
use crate::memory::{MEMORY_SCOPE_ID, MemoryStore};
use crate::message_ext::AgentMessageExt;
use crate::storage::AgentStorage;

fn stage1(session_id: Uuid, source_hash: &str, raw_memory: &str) -> MemoryStage1Record {
    MemoryStage1Record {
        session_id,
        source_hash: source_hash.to_string(),
        raw_memory: raw_memory.to_string(),
        rollout_summary: "summary".to_string(),
        rollout_slug: Some("test-session".to_string()),
        status: "succeeded".to_string(),
        generated_at: crate::memory::now_ms(),
        lease_until: 0,
    }
}

#[tokio::test]
async fn memory_stage1_claims_are_lease_and_hash_idempotent() {
    let store = TursoAgentStorage::open_in_memory().await.unwrap();
    let session_id = Uuid::new_v4();
    let lease_until = crate::memory::now_ms() + 3_600_000;

    assert!(
        store
            .claim_stage1(MEMORY_SCOPE_ID, session_id, "hash-a", lease_until)
            .await
            .unwrap()
    );
    assert!(
        !store
            .claim_stage1(MEMORY_SCOPE_ID, session_id, "hash-a", lease_until)
            .await
            .unwrap()
    );
    store
        .complete_stage1(MEMORY_SCOPE_ID, stage1(session_id, "hash-a", "raw"))
        .await
        .unwrap();
    assert!(
        !store
            .claim_stage1(MEMORY_SCOPE_ID, session_id, "hash-a", lease_until)
            .await
            .unwrap()
    );
    assert!(
        store
            .claim_stage1(MEMORY_SCOPE_ID, session_id, "hash-b", lease_until)
            .await
            .unwrap()
    );
    store
        .complete_stage1(MEMORY_SCOPE_ID, stage1(session_id, "hash-b", "raw-b"))
        .await
        .unwrap();

    let rows = store
        .list_stage1_outputs(MEMORY_SCOPE_ID, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn memory_phase2_lock_is_singleton_and_source_idempotent() {
    let store = TursoAgentStorage::open_in_memory().await.unwrap();
    let lease_until = crate::memory::now_ms() + 3_600_000;

    assert!(
        store
            .claim_phase2(MEMORY_SCOPE_ID, "input-a", lease_until)
            .await
            .unwrap()
    );
    assert!(
        !store
            .claim_phase2(MEMORY_SCOPE_ID, "input-a", lease_until)
            .await
            .unwrap()
    );
    store
        .complete_phase2(
            MEMORY_SCOPE_ID,
            "input-a",
            Vec::new(),
            "v1\n\ntest",
            Vec::new(),
            Vec::new(),
        )
        .await
        .unwrap();
    assert!(
        !store
            .claim_phase2(MEMORY_SCOPE_ID, "input-a", lease_until)
            .await
            .unwrap()
    );
    assert!(
        store
            .claim_phase2(MEMORY_SCOPE_ID, "input-b", lease_until)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn list_session_records_includes_idle_end_time() {
    let store = TursoAgentStorage::open_in_memory().await.unwrap();
    let agent_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    store.start_session(agent_id, session_id).await.unwrap();
    store
        .append_message(session_id, &Message::user("remember this"))
        .await
        .unwrap();
    store.end_session(session_id).await.unwrap();

    let records = store.list_session_records(agent_id).await.unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].ended_at.is_some());
}
