#[cfg(test)]
use super::*;
use crate::SkillManager;
use std::sync::Arc;
use std::time::Duration;

mod workflow_tests {
    use super::*;
    use crate::observation::{
        ObservationInput, ObservationKind, ObservationSource, ObservationStore,
    };

    fn manager_in(tmp: &std::path::Path) -> Arc<SkillManager> {
        Arc::new(SkillManager::new(tmp.join("state")))
    }

    fn observe_three(manager: &SkillManager, error: &str) {
        for body in ["fix a", "fix b", "fix c"] {
            manager
                .record_observation(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: format!("fix for {error}"),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some(error.into()),
                })
                .unwrap();
        }
    }

    #[test]
    fn default_policy_leaves_proposals_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        observe_three(&manager, "boom N");
        let report = run_evolution_cycle(&manager, &EvolutionPolicy::default()).unwrap();
        assert_eq!(report.proposals_written.len(), 1);
        assert!(report.auto_approved.is_empty());
        assert_eq!(report.left_pending, 1);
    }

    #[test]
    fn auto_approve_respects_cap_and_bumps_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        // Two distinct anchors → two pending proposals.
        observe_three(&manager, "boom one");
        observe_three(&manager, "crash two");
        let policy = EvolutionPolicy {
            auto_approve: true,
            max_approvals_per_cycle: 1,
        };
        let generation = manager.generation();
        let report = run_evolution_cycle(&manager, &policy).unwrap();
        assert_eq!(report.auto_approved.len(), 1, "cap enforced");
        assert_eq!(report.left_pending, 1);
        assert_eq!(manager.generation(), generation + 1);
    }

    #[test]
    fn cycle_without_new_observations_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let first = run_evolution_cycle(&manager, &EvolutionPolicy::default()).unwrap();
        assert!(!first.acted());
        let second = run_evolution_cycle(&manager, &EvolutionPolicy::default()).unwrap();
        assert!(!second.acted());
    }

    #[tokio::test]
    async fn service_coalesces_bursts_into_one_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        observe_three(&manager, "storm N");

        let options = EvolutionOptions {
            quiet_window: Duration::from_millis(150),
            timer: None,
            policy: EvolutionPolicy::default(),
        };
        let handle = start(manager.clone(), options);
        let mut reports = handle.subscribe_reports();

        // Wait for the startup-sweep report first.
        let startup = tokio::time::timeout(Duration::from_secs(2), reports.recv())
            .await
            .expect("startup report")
            .expect("channel open");
        assert!(startup.triggers.contains(&"startup"));
        assert_eq!(startup.proposals_written.len(), 1);

        // A burst of manual triggers coalesces into exactly one more
        // report (which is a no-op — the cluster is consumed).
        for i in 0..5 {
            handle.trigger(EvolutionTrigger::Manual {
                by: format!("t{i}"),
            });
        }
        let coalesced = tokio::time::timeout(Duration::from_secs(2), reports.recv())
            .await
            .expect("coalesced report")
            .expect("channel open");
        assert!(
            coalesced
                .triggers
                .iter()
                .filter(|t| **t == "manual")
                .count()
                >= 1,
            "{:?}",
            coalesced.triggers
        );
        assert!(!coalesced.acted());
        assert_eq!(handle.dropped_commands(), 0);
    }

    #[tokio::test]
    async fn observation_recording_fires_the_trigger_path() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let options = EvolutionOptions {
            quiet_window: Duration::from_millis(50),
            timer: None,
            policy: EvolutionPolicy::default(),
        };
        let handle = start(manager.clone(), options.clone());
        let mut reports = handle.subscribe_reports();
        // Drain the startup report.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;

        // The manager must be attached to the service to forward
        // observation events.
        manager.attach_evolution(&handle);
        observe_three(&manager, "hooked N");
        let report = tokio::time::timeout(Duration::from_secs(2), reports.recv())
            .await
            .expect("event-driven report")
            .expect("channel open");
        assert!(
            report.triggers.contains(&"observation"),
            "{:?}",
            report.triggers
        );
        assert_eq!(report.proposals_written.len(), 1);
    }

    #[test]
    fn observation_store_roundtrip_under_manager() {
        // Guards the store path used by the cycle against accidental
        // regressions in manager wiring.
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        assert_eq!(manager.observations().list().len(), 0);
        observe_three(&manager, "x");
        assert_eq!(manager.observations().list().len(), 3);
    }
}

#[cfg(test)]
mod timer_tests {
    use super::*;
    use crate::observation::{ObservationInput, ObservationKind, ObservationSource};

    /// The timer trigger path: with no events arriving, the periodic
    /// sweep alone produces cycles.
    #[tokio::test]
    async fn timer_sweeps_without_events() {
        // Seed the observation pool so the worker actually runs the
        // cycle body; the empty-pool short-circuit introduced later
        // would otherwise elide the broadcast entirely.
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        for body in ["fix a", "fix b", "fix c"] {
            manager
                .record_observation(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: "fix for boom N".into(),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some("boom at step N".into()),
                })
                .unwrap();
        }
        let options = EvolutionOptions {
            quiet_window: Duration::from_millis(20),
            timer: Some(Duration::from_millis(120)),
            policy: EvolutionPolicy::default(),
        };
        let handle = start(manager, options);
        let mut reports = handle.subscribe_reports();
        // Startup report first…
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        // …then at least one timer report within a generous window.
        let timer_report = tokio::time::timeout(Duration::from_secs(3), reports.recv())
            .await
            .expect("timer report")
            .expect("channel open");
        assert!(
            timer_report.triggers.contains(&"timer"),
            "{:?}",
            timer_report.triggers
        );
    }
}

#[cfg(test)]
mod authoring_tests {
    use super::*;
    use crate::observation::{ObservationInput, ObservationKind, ObservationSource};

    #[test]
    fn auto_approve_never_promotes_agent_authored_proposals() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        let id = manager
            .record_observation(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: "evidence".into(),
                body: "fix".into(),
                node_kind: Some("sql".into()),
                error: Some("boom".into()),
            })
            .unwrap()
            .id;
        manager
            .propose_skill(
                "agent-made",
                "Agent-drafted skill.",
                &[],
                "# Body\n\nSteps.\n",
                std::slice::from_ref(&id),
                "pattern",
            )
            .unwrap();

        let policy = EvolutionPolicy {
            auto_approve: true,
            max_approvals_per_cycle: 5,
        };
        let generation = manager.generation();
        let report = run_evolution_cycle(&manager, &policy).unwrap();
        // Refused: still pending, human-review-only, generation unmoved.
        assert!(report.auto_approved.is_empty());
        assert_eq!(report.left_pending, 1);
        assert_eq!(report.human_review_only, 1);
        assert_eq!(manager.generation(), generation);
        assert_eq!(
            manager.proposals().find("agent-made").unwrap().status,
            crate::proposals::ProposalStatus::Pending
        );
    }
}

/// Coverage for the unified channel: every skill mutation
/// (evolution triggers, proposal approve/reject, explicit cycle)
/// travels through one `mpsc::Sender<SkillCommand>` into the worker
/// task. These tests pin the funnel invariants the design relies on.
#[cfg(test)]
mod channel_funnel {
    use super::*;
    use crate::observation::ObservationInput;
    use crate::proposals::ProposalStatus;

    fn start_service(state_dir: &std::path::Path) -> (Arc<SkillManager>, SkillControlHandle) {
        let manager = Arc::new(SkillManager::new(state_dir));
        let handle = start(
            manager.clone(),
            EvolutionOptions {
                quiet_window: Duration::from_millis(20),
                timer: None,
                policy: EvolutionPolicy::default(),
            },
        );
        (manager, handle)
    }

    /// Three anchored failures on `(node_kind=sql, error=boom)` plus
    /// a `RunCycle` produces one pending proposal. Returns the
    /// proposal name (deterministic slug from `distill`).
    ///
    /// Caller is responsible for draining the Startup sweep
    /// broadcast BEFORE calling this — the Startup trigger's
    /// quiet-window drain would otherwise capture the RunCycle
    /// and create the proposal out from under us.
    async fn seed_pending_proposal(manager: &SkillManager, handle: &SkillControlHandle) -> String {
        for body in ["fix a", "fix b", "fix c"] {
            manager
                .record_observation(ObservationInput {
                    kind: crate::observation::ObservationKind::Failure,
                    source: crate::observation::ObservationSource::Agent,
                    summary: "fix for boom".into(),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some("boom".into()),
                })
                .unwrap();
        }
        let outcome = handle.call_run_cycle(EvolutionPolicy::default()).await;
        let report = match outcome {
            SkillCommandOutcome::Ok(r) => r,
            other => panic!("seed cycle failed: {other:?}"),
        };
        assert_eq!(report.proposals_written.len(), 1);
        report.proposals_written[0].clone()
    }

    /// Happy path: `call_approve` returns `SkillCommandOutcome::Ok`
    /// and bumps the manager's generation (the channel worker
    /// called `manager.approve_proposal` directly).
    #[tokio::test]
    async fn channel_funnel_call_approve_returns_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, handle) = start_service(&tmp.path().join("state"));
        let mut reports = handle.subscribe_reports();
        // Drain the Startup sweep broadcast BEFORE recording
        // observations; otherwise the Startup trigger's quiet-window
        // drain captures our RunCycle and creates the proposal
        // before we can control the cycle order.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let name = seed_pending_proposal(&manager, &handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let generation_before = manager.generation();
        let outcome = handle.call_approve(name.clone()).await;
        match outcome {
            SkillCommandOutcome::Ok(o) => assert_eq!(o.name, name),
            other => panic!("expected Ok, got {other:?}"),
        }
        assert_eq!(
            manager.generation(),
            generation_before + 1,
            "approve bumps the generation"
        );
    }

    /// Happy path: `call_reject` returns the updated proposal with
    /// `Rejected` status.
    #[tokio::test]
    async fn channel_funnel_call_reject_returns_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, handle) = start_service(&tmp.path().join("state"));
        let mut reports = handle.subscribe_reports();
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let name = seed_pending_proposal(&manager, &handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;

        let outcome = handle.call_reject(name.clone()).await;
        match outcome {
            SkillCommandOutcome::Ok(proposal) => {
                assert_eq!(proposal.name, name);
                assert_eq!(proposal.status, ProposalStatus::Rejected);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    /// `call_run_cycle` runs a cycle and replies with the report;
    /// the same report is also broadcast on `subscribe_reports` so
    /// the dashboard live view picks it up.
    #[tokio::test]
    async fn channel_funnel_call_run_cycle_returns_report() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, handle) = start_service(&tmp.path().join("state"));
        let mut reports = handle.subscribe_reports();
        // Drain startup report first.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let name = seed_pending_proposal(&manager, &handle).await;
        // Drain the seed cycle broadcast.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        assert_eq!(name, "sql-boom");
    }

    /// After the channel closes (every handle dropped, worker
    /// exited), the manager's `record_observation` forwarder still
    /// succeeds — the dropped-triggers contract means the
    /// observation is recorded but no evolution trigger fires.
    /// This pins the no-block guarantee on the observation hot
    /// path.
    #[tokio::test]
    async fn channel_funnel_observation_forwarder_does_not_block_after_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, handle) = start_service(&tmp.path().join("state"));
        drop(handle);
        // Let the worker observe the closed channel and exit.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let probe = manager.record_observation(ObservationInput {
            kind: crate::observation::ObservationKind::Failure,
            source: crate::observation::ObservationSource::Agent,
            summary: "probe".into(),
            body: "x".into(),
            node_kind: Some("sql".into()),
            error: Some("boom".into()),
        });
        assert!(
            probe.is_ok(),
            "observation must succeed even with worker gone"
        );
    }

    /// Dropped-command observability: the counter is reachable
    /// through the handle and starts at zero; a successful trigger
    /// does not bump it (the channel has capacity).
    #[tokio::test]
    async fn channel_funnel_dropped_command_counter_starts_at_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let (_manager, handle) = start_service(&tmp.path().join("state"));
        let baseline = handle.dropped_commands();
        handle.trigger(EvolutionTrigger::Manual { by: "t0".into() });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(handle.dropped_commands(), baseline);
    }

    /// The critical UX invariant: a TUI approve click must NOT
    /// wait behind a possibly-slow coalesced evolution cycle. The
    /// worker drains the quiet window first; inline approve/reject
    /// commands are processed BEFORE the cycle runs, so the HTTP
    /// caller never sees the cycle's file I/O.
    ///
    /// We exercise this by setting a 300 ms quiet window, firing
    /// a manual trigger, then queueing an ApproveProposal
    /// mid-drain. With the design, both the approve reply and the
    /// cycle broadcast must arrive within a generous window; the
    /// cycle broadcast's `triggers` field confirms the manual
    /// trigger was coalesced (proves the cycle ran the manual
    /// trigger's batch, not just a no-op).
    #[tokio::test]
    async fn channel_funnel_inline_approve_does_not_block_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        let handle = start(
            manager.clone(),
            EvolutionOptions {
                quiet_window: Duration::from_millis(300),
                timer: None,
                policy: EvolutionPolicy::default(),
            },
        );
        let mut reports = handle.subscribe_reports();
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        let name = seed_pending_proposal(&manager, &handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;

        // Open a fresh quiet-window drain with a manual trigger.
        handle.trigger(EvolutionTrigger::Manual { by: "t0".into() });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let outcome = handle.call_approve(name.clone()).await;
        match outcome {
            SkillCommandOutcome::Ok(_) => {}
            other => panic!("expected Ok, got {other:?}"),
        }
        // The cycle broadcast should arrive within the drain
        // window plus a cycle's worth of file I/O. Generous
        // bound keeps the test stable on busy CI.
        let cycle_broadcast = tokio::time::timeout(Duration::from_secs(2), reports.recv())
            .await
            .expect("cycle broadcast")
            .expect("channel open");
        // The coalesced cycle should have the manual trigger in
        // its triggers list (the worker coalesced the manual
        // trigger into the next cycle, not a fresh cycle).
        assert!(
            cycle_broadcast.triggers.contains(&"manual"),
            "cycle report missing manual trigger: {:?}",
            cycle_broadcast.triggers
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn monitor_reflects_coalesce_distill_idle_phases() {
        // The monitor must transition idle → coalescing → distilling →
        // idle around a run, and stamp a meaningful last-cycle row.
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        // Seed one observation so the startup sweep has something to
        // distill; otherwise the worker short-circuits the cycle
        // body and `cycles_completed` stays at 0.
        manager
            .record_observation(crate::observation::ObservationInput {
                kind: crate::observation::ObservationKind::Failure,
                source: crate::observation::ObservationSource::Agent,
                summary: "fix for boom N".into(),
                body: "fix a".into(),
                node_kind: Some("sql".into()),
                error: Some("boom at step N".into()),
            })
            .unwrap();
        let handle = start(
            manager.clone(),
            EvolutionOptions {
                policy: EvolutionPolicy::default(),
                quiet_window: Duration::from_millis(80),
                timer: None,
            },
        );

        // Startup sweep finishes before we sample, so the monitor
        // should report at least one cycle right out of the gate.
        tokio::time::sleep(Duration::from_millis(120)).await;
        let post_startup = handle.cycle_status();
        assert_eq!(post_startup.phase, CyclePhase::Idle);
        assert!(post_startup.cycles_completed >= 1);

        // Drive a manual cycle through the worker and capture the
        // status just before the worker is parked (and after — we
        // read by polling, so the final state is what matters).
        handle.trigger(EvolutionTrigger::Manual { by: "t1".into() });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let final_status = loop {
            let s = handle.cycle_status();
            if !matches!(s.phase, CyclePhase::Idle) || std::time::Instant::now() >= deadline {
                // Cycle should have advanced the counter past startup.
                if s.cycles_completed >= 2 && s.last_triggers.iter().any(|t| t == "manual") {
                    break s;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(final_status.phase, CyclePhase::Idle);
        assert!(
            final_status.last_triggers.iter().any(|t| t == "manual"),
            "last_triggers missing: {:?}",
            final_status.last_triggers
        );
        assert!(final_status.last_cycle_duration_ms.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_observation_pool_short_circuits_before_merge() {
        // The startup sweep also gets short-circuited when no
        // observations exist; subsequent Manual triggers must not
        // flip the phase into Distilling nor bump cycles_completed.
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(SkillManager::new(tmp.path().join("state")));
        let handle = start(
            manager.clone(),
            EvolutionOptions {
                policy: EvolutionPolicy::default(),
                quiet_window: Duration::from_millis(40),
                timer: None,
            },
        );

        tokio::time::sleep(Duration::from_millis(80)).await;
        let after_startup = handle.cycle_status();
        assert!(
            after_startup.cycle_skipped_empty >= 1,
            "startup sweep against empty pool already short-circuits"
        );

        // A manual trigger against an empty pool must also be a
        // skip, not a Distilling run.
        handle.trigger(EvolutionTrigger::Manual { by: "t0".into() });
        tokio::time::sleep(Duration::from_millis(150)).await;

        let status = handle.cycle_status();
        assert_eq!(status.phase, CyclePhase::Idle);
        assert_eq!(
            status.cycles_completed, 0,
            "cycles_completed stays at 0 when every cycle was skipped"
        );
        assert!(status.cycle_skipped_empty >= 1);
        assert_eq!(
            status.last_skipped_reason.as_deref(),
            Some("empty_pool"),
            "skip reason surfaces the empty-pool signal"
        );
        assert!(
            status.last_triggers.iter().any(|t| t == "manual"),
            "last_triggers records what got skipped: {:?}",
            status.last_triggers
        );
    }
}
