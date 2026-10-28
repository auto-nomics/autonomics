use super::*;

fn command(
    service: &SemanticService,
    action: SemanticMaintenanceAction,
) -> SemanticMaintenanceRequest {
    SemanticMaintenanceRequest {
        action,
        expected_configuration_generation: service.status().configuration_generation,
        request_id: uuid::Uuid::new_v4().to_string(),
    }
}
async fn stop(service: SemanticService) {
    let mut closed = service.state.clone();
    drop(service);
    tokio::time::timeout(Duration::from_secs(5), async {
        while closed.changed().await.is_ok() {}
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn raising_budget_resumes_failed_repair_with_its_sixteen_committed_vectors() {
    let fixture = Fixture::new().await;
    for n in 0..20 {
        fixture.paper(&format!("budget-{n:02}")).await;
    }
    let service = fixture.service().await;
    let mut config = enabled();
    service.configure(config.clone()).await.unwrap();
    wait(&service, SemanticPhase::Ready).await;
    let before = fixture.probe.calls.load(Ordering::SeqCst);
    config.limits.max_vector_bytes = 16 * 8;
    service.configure(config.clone()).await.unwrap();
    wait(&service, SemanticPhase::Failed).await;
    let input = command(&service, SemanticMaintenanceAction::Repair);
    let job = service.maintain(input.clone()).await.unwrap();
    let failed = wait(&service, SemanticPhase::Failed).await;
    assert_eq!(
        failed.maintenance.as_ref().unwrap().request_id,
        job.request_id
    );
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), before + 16);
    config.limits.max_vector_bytes = 20 * 8;
    let epoch = service.configure(config).await.unwrap();
    let ready = wait(&service, SemanticPhase::Ready).await;
    assert_eq!(ready.configuration_generation, epoch);
    assert_eq!(ready.store_generation, job.store_generation.unwrap());
    assert_eq!(
        ready.maintenance.as_ref().unwrap().request_id,
        job.request_id
    );
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), before + 20);
    let progress = ready
        .maintenance
        .as_ref()
        .unwrap()
        .progress
        .as_ref()
        .unwrap();
    assert_eq!(progress.cached_chunks, 16);
    assert_eq!(progress.embedded_chunks, 4);
    assert_eq!(service.maintain(input).await.unwrap().state, "complete");
    stop(service).await;
}

#[tokio::test]
async fn budget_rebinding_fences_old_owners_and_preserves_explicit_pause() {
    let fixture = Fixture::new().await;
    let (conn, generation) = configured(&fixture).await;
    let (_, mut old) = maintenance::request(
        &conn,
        SemanticMaintenanceRequest {
            action: SemanticMaintenanceAction::Repair,
            expected_configuration_generation: generation,
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .await
    .unwrap();
    assert!(
        maintenance::claim(&conn, &mut old, "old-owner")
            .await
            .unwrap()
    );
    let mut config = enabled();
    config.limits.max_chunks = 123;
    let rebound = maintenance::configure(&conn, config.clone()).await.unwrap();
    let next = rebound.maintenance.unwrap();
    assert_eq!(next.state, "queued");
    assert_eq!(next.store_generation, old.store_generation);
    assert!(
        maintenance::heartbeat(&conn, &old, "old-owner")
            .await
            .is_err()
    );
    assert!(
        maintenance::finish(
            &conn,
            &old,
            "old-owner",
            Some(("old-signature", "old-store"))
        )
        .await
        .is_err()
    );
    let (_, paused) = maintenance::request(
        &conn,
        SemanticMaintenanceRequest {
            action: SemanticMaintenanceAction::Cancel,
            expected_configuration_generation: rebound.generation,
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .await
    .unwrap();
    config.limits.max_chunks = 124;
    let paused_again = maintenance::configure(&conn, config)
        .await
        .unwrap()
        .maintenance
        .unwrap();
    assert_eq!(paused_again.action, SemanticMaintenanceAction::Cancel);
    assert_eq!(paused_again.state, "cancelled");
    assert_eq!(paused_again.store_generation, paused.store_generation);
}

#[tokio::test]
async fn rebuild_reuses_valid_vectors_reports_progress_and_is_idempotent() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    fixture.paper("b").await;
    let service = fixture.service().await;
    service.configure(enabled()).await.unwrap();
    let old = wait(&service, SemanticPhase::Ready).await;
    let before = fixture.probe.calls.load(Ordering::SeqCst);
    let input = command(&service, SemanticMaintenanceAction::Rebuild);
    let accepted = service.maintain(input.clone()).await.unwrap();
    let rebuilt = wait(&service, SemanticPhase::Ready).await;
    assert_ne!(rebuilt.generation, old.generation);
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), before);
    let job = rebuilt.maintenance.unwrap();
    assert_eq!(job.state, "complete");
    let progress = job.progress.unwrap();
    assert_eq!(progress.target_articles, Some(2));
    assert_eq!(progress.processed_articles, 2);
    assert_eq!(progress.embedded_chunks, 0);
    assert_eq!(progress.cached_chunks, 2);
    assert_eq!(progress.stage, "complete");
    let replay = service.maintain(input.clone()).await.unwrap();
    assert_eq!(replay.request_id, accepted.request_id);
    assert_eq!(
        replay.configuration_generation,
        accepted.configuration_generation
    );
    let mut conflict = input;
    conflict.action = SemanticMaintenanceAction::Repair;
    assert!(matches!(
        service.maintain(conflict).await,
        Err(Error::SearchConflict(_))
    ));
    let mut stale = command(&service, SemanticMaintenanceAction::Repair);
    stale.expected_configuration_generation = old.configuration_generation;
    assert!(matches!(
        service.maintain(stale).await,
        Err(Error::SearchConflict(_))
    ));
    stop(service).await;
}

#[tokio::test]
async fn isolated_repair_preserves_corrupt_store_and_survives_restart() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    fixture.paper("b").await;
    let service = fixture.service().await;
    service.configure(enabled()).await.unwrap();
    let old = wait(&service, SemanticPhase::Ready).await;
    let path = fixture
        .path
        .join("vectors.db")
        .to_string_lossy()
        .into_owned();
    let db = turso::Builder::new_local(&path)
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let corrupt = db.connect().unwrap();
    corrupt
        .execute("UPDATE semanticVectors SET checksum='tampered'", ())
        .await
        .unwrap();
    service
        .maintain(command(&service, SemanticMaintenanceAction::Repair))
        .await
        .unwrap();
    let repaired = wait(&service, SemanticPhase::Ready).await;
    assert_ne!(repaired.generation, old.generation);
    assert!(!repaired.store_generation.is_empty());
    assert_eq!(
        repaired
            .maintenance
            .as_ref()
            .unwrap()
            .progress
            .as_ref()
            .unwrap()
            .embedded_chunks,
        2
    );
    let mut rows = corrupt
        .query(
            "SELECT COUNT(*) FROM semanticVectors WHERE checksum='tampered'",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        2,
        "repair must not mutate the old vector store"
    );
    drop(rows);
    drop(corrupt);
    drop(db);
    assert!(std::path::Path::new(&store_path(&path, &repaired.store_generation)).exists());
    let prepared = service.prepare("topic").await.unwrap();
    assert_eq!(prepared.store_generation, repaired.store_generation);
    let calls = fixture.probe.calls.load(Ordering::SeqCst);
    stop(service).await;
    let reopened = fixture.service().await;
    let durable = wait(&reopened, SemanticPhase::Ready).await;
    assert_eq!(durable.generation, repaired.generation);
    assert_eq!(durable.store_generation, repaired.store_generation);
    assert_eq!(
        fixture.probe.calls.load(Ordering::SeqCst),
        calls,
        "restart loads the activated repair without embedding"
    );
    stop(reopened).await;
}

#[tokio::test]
async fn repair_can_replace_a_physically_unreadable_derived_database() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    service.configure(enabled()).await.unwrap();
    wait(&service, SemanticPhase::Ready).await;
    stop(service).await;
    let path = fixture.path.join("vectors.db");
    for suffix in ["-wal", "-tshm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.to_string_lossy()));
    }
    let broken = b"deliberately invalid derived-index database header";
    std::fs::write(&path, broken).unwrap();
    let service = fixture.service().await;
    wait(&service, SemanticPhase::Failed).await;
    service
        .maintain(command(&service, SemanticMaintenanceAction::Repair))
        .await
        .unwrap();
    let restored = wait(&service, SemanticPhase::Ready).await;
    assert_eq!(restored.indexed_chunks, 1);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        broken,
        "broken base is preserved; repair switches a separate store"
    );
    assert_eq!(
        service.prepare("topic").await.unwrap().snapshot.chunks[0].article_id,
        "a"
    );
    stop(service).await;
}

#[tokio::test]
async fn paused_repair_keeps_completed_batches_and_does_not_block_writes_or_lexical_search() {
    let fixture = Fixture::new().await;
    for n in 0..20 {
        fixture.paper(&format!("a{n:02}")).await;
    }
    let service = fixture.service().await;
    let library = fixture.library(&service).await;
    service.configure(enabled()).await.unwrap();
    synchronized(&service, &library).await;
    let old = service.status();
    fixture.probe.pause_at.store(
        fixture.probe.calls.load(Ordering::SeqCst) + 17,
        Ordering::SeqCst,
    );
    let repair = service
        .maintain(command(&service, SemanticMaintenanceAction::Repair))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.probe.entered.notified())
        .await
        .unwrap();
    assert_eq!(
        service.status().progress.as_ref().unwrap().processed_chunks,
        16
    );
    assert_eq!(service.status().generation, old.generation);
    assert_eq!(
        service.prepare("topic").await.unwrap().snapshot.generation,
        old.generation.clone().unwrap()
    );
    tokio::time::timeout(Duration::from_secs(2), fixture.paper("new"))
        .await
        .unwrap();
    let lexical = tokio::time::timeout(
        Duration::from_secs(2),
        library.search_v2(bib_types::SearchRequestV2 {
            query: "Canonical".into(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(lexical.total, 21);
    service
        .maintain(command(&service, SemanticMaintenanceAction::Cancel))
        .await
        .unwrap();
    let paused = wait(&service, SemanticPhase::Paused).await;
    assert_eq!(paused.store_generation, old.store_generation);
    assert_eq!(paused.generation, old.generation);
    let calls = fixture.probe.calls.load(Ordering::SeqCst);
    drop(library);
    stop(service).await;
    let reopened = fixture.service().await;
    wait(&reopened, SemanticPhase::Paused).await;
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), calls);
    assert!(
        reopened.prepare("topic").await.is_err(),
        "paused restart does not contact a model before resume"
    );
    reopened
        .maintain(command(&reopened, SemanticMaintenanceAction::Resume))
        .await
        .unwrap();
    let resumed = wait(&reopened, SemanticPhase::Ready).await;
    assert_eq!(resumed.indexed_chunks, 21);
    assert_eq!(
        Some(resumed.store_generation.clone()),
        repair.store_generation
    );
    let progress = resumed.maintenance.unwrap().progress.unwrap();
    assert_eq!(progress.cached_chunks, 16);
    assert_eq!(progress.embedded_chunks, 5);
    stop(reopened).await;
}

async fn configured(fixture: &Fixture) -> (Connection, String) {
    let connection = fixture.db.connect().unwrap();
    let generation = uuid::Uuid::new_v4().to_string();
    connection
        .execute(
            "INSERT INTO bibliographySemanticConfig VALUES(1,?1,?2)",
            turso::params![
                serde_json::to_string(&enabled()).unwrap(),
                generation.clone()
            ],
        )
        .await
        .unwrap();
    (connection, generation)
}

#[tokio::test]
async fn maintenance_leases_fence_expired_owners_and_late_activation() {
    let fixture = Fixture::new().await;
    let (conn, generation) = configured(&fixture).await;
    let (_, mut job) = maintenance::request(
        &conn,
        SemanticMaintenanceRequest {
            action: SemanticMaintenanceAction::Resume,
            expected_configuration_generation: generation,
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .await
    .unwrap();
    assert!(
        maintenance::claim(&conn, &mut job, "owner-a")
            .await
            .unwrap()
    );
    assert!(
        !maintenance::claim(&conn, &mut job, "owner-b")
            .await
            .unwrap()
    );
    conn.execute(
        "UPDATE bibliographySemanticMaintenance SET lease_until=0",
        (),
    )
    .await
    .unwrap();
    assert!(
        maintenance::heartbeat(&conn, &job, "owner-a")
            .await
            .is_err()
    );
    job.state = "complete".into();
    assert!(
        maintenance::finish(&conn, &job, "owner-a", Some(("signature", "forbidden")))
            .await
            .is_err()
    );
    assert!(
        maintenance::claim(&conn, &mut job, "owner-b")
            .await
            .unwrap()
    );
    job.state = "failed".into();
    maintenance::finish(&conn, &job, "owner-b", None)
        .await
        .unwrap();
    assert!(
        maintenance::finish(&conn, &job, "owner-a", Some(("signature", "forbidden")))
            .await
            .is_err()
    );
    assert!(active_store(&conn, "signature").await.unwrap().is_empty());
}

#[tokio::test]
async fn artifact_gc_retains_active_live_and_paused_stores() {
    let fixture = Fixture::new().await;
    let (conn, generation) = configured(&fixture).await;
    let input = SemanticMaintenanceRequest {
        action: SemanticMaintenanceAction::Repair,
        expected_configuration_generation: generation,
        request_id: uuid::Uuid::new_v4().to_string(),
    };
    let (_, job) = maintenance::request(&conn, input).await.unwrap();
    let active = uuid::Uuid::new_v4().to_string();
    let live = uuid::Uuid::new_v4().to_string();
    let obsolete = uuid::Uuid::new_v4().to_string();
    let paused = job.store_generation.unwrap();
    let base = fixture
        .path
        .join("vectors.db")
        .to_string_lossy()
        .into_owned();
    for id in [&active, &live, &obsolete, &paused] {
        std::fs::write(store_path(&base, id), b"derived artifact").unwrap();
        let lease = if id == &live {
            chrono::Utc::now().timestamp_millis() + 100_000
        } else {
            0
        };
        conn.execute(
            "INSERT INTO bibliographySemanticArtifacts VALUES(?1,?2)",
            turso::params![id.clone(), lease],
        )
        .await
        .unwrap();
    }
    conn.execute(
        "INSERT INTO bibliographySemanticStores VALUES('active',?1)",
        turso::params![active.clone()],
    )
    .await
    .unwrap();
    maintenance::cleanup(&conn, &base).await.unwrap();
    assert!(!std::path::Path::new(&store_path(&base, &obsolete)).exists());
    for id in [&active, &live, &paused] {
        assert!(std::path::Path::new(&store_path(&base, id)).exists());
    }
}

#[tokio::test]
async fn activation_failure_rolls_back_job_completion() {
    let fixture = Fixture::new().await;
    let (conn, generation) = configured(&fixture).await;
    let (_, mut job) = maintenance::request(
        &conn,
        SemanticMaintenanceRequest {
            action: SemanticMaintenanceAction::Repair,
            expected_configuration_generation: generation,
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .await
    .unwrap();
    assert!(maintenance::claim(&conn, &mut job, "owner").await.unwrap());
    conn.execute("DROP TABLE bibliographySemanticStores", ())
        .await
        .unwrap();
    job.state = "complete".into();
    assert!(
        maintenance::finish(
            &conn,
            &job,
            "owner",
            Some(("signature", job.store_generation.as_deref().unwrap()))
        )
        .await
        .is_err()
    );
    assert_eq!(
        read_maintenance(&conn, &job.configuration_generation)
            .await
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
}

#[tokio::test]
async fn idle_sync_keeps_ready_phase_and_does_not_schedule_model_work() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    service.configure(enabled()).await.unwrap();
    let first = wait(&service, SemanticPhase::Ready).await;
    let calls = fixture.probe.calls.load(Ordering::SeqCst);
    let mut changes = service.state.clone();
    changes.borrow_and_update();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            changes.changed().await.unwrap();
            let status = changes.borrow_and_update().status.clone();
            assert_eq!(status.phase, SemanticPhase::Ready);
            assert_eq!(status.generation, first.generation);
            if status.progress.is_some_and(|progress| {
                progress.target_articles == Some(0) && progress.stage == "complete"
            }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), calls);
    stop(service).await;
}
