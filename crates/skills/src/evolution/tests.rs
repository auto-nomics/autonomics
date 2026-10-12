#[cfg(test)]
use super::*;
use crate::SkillManager;
use std::sync::Arc;
use std::time::Duration;

mod workflow_tests {
    use super::*;
    use evolution_core::observation::{ObservationInput, ObservationKind, ObservationSource};

    fn manager_in(tmp: &std::path::Path) -> Arc<SkillManager> {
        Arc::new(SkillManager::new(tmp.join("state")))
    }

    async fn observe_three(manager: &SkillManager, error: &str) {
        for body in ["fix a", "fix b", "fix c"] {
            manager
                .observation_hub()
                .record(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: format!("fix for {error}"),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some(error.into()),
                })
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn cycle_installs_what_it_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        observe_three(&manager, "boom N").await;
        let report = run_evolution_cycle(&manager, &EvolutionPolicy::default())
            .await
            .unwrap();
        assert_eq!(report.proposals_written.len(), 1);
        assert_eq!(
            report.installed.len(),
            1,
            "no review gate: applied in-cycle"
        );
        assert_eq!(report.deferred, 0);
        let name = &report.installed[0];
        assert!(
            tmp.path()
                .join("state/skills")
                .join(name)
                .join("SKILL.md")
                .is_file(),
            "skill must be live in the global tier"
        );
    }

    #[tokio::test]
    async fn install_cap_defers_overflow_and_bumps_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        // Two distinct anchors → two installs wanted.
        observe_three(&manager, "boom one").await;
        observe_three(&manager, "crash two").await;
        let policy = EvolutionPolicy {
            max_installs_per_cycle: 1,
        };
        let generation = manager.generation();
        let report = run_evolution_cycle(&manager, &policy).await.unwrap();
        assert_eq!(report.installed.len(), 1, "cap enforced");
        assert_eq!(report.deferred, 1);
        assert_eq!(manager.generation(), generation + 1);

        // The next cycle applies the deferred proposal.
        let followup = run_evolution_cycle(&manager, &EvolutionPolicy::default())
            .await
            .unwrap();
        assert_eq!(followup.installed.len(), 1, "deferred overflow lands");
    }

    #[tokio::test]
    async fn cycle_without_new_observations_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        let first = run_evolution_cycle(&manager, &EvolutionPolicy::default())
            .await
            .unwrap();
        assert!(!first.acted());
        let second = run_evolution_cycle(&manager, &EvolutionPolicy::default())
            .await
            .unwrap();
        assert!(!second.acted());
    }

    #[tokio::test]
    async fn service_coalesces_bursts_into_one_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        observe_three(&manager, "storm N").await;

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
    async fn observation_store_roundtrip_under_manager() {
        // Guards the store path used by the cycle against accidental
        // regressions in manager wiring.
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_in(tmp.path());
        assert_eq!(manager.observations().await.len(), 0);
        observe_three(&manager, "x").await;
        assert_eq!(manager.observations().await.len(), 3);
    }
}

#[cfg(test)]
mod timer_tests {
    use super::*;
    use evolution_core::observation::{ObservationInput, ObservationKind, ObservationSource};

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
                .observation_hub()
                .record(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: "fix for boom N".into(),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some("boom at step N".into()),
                })
                .await
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
    use evolution_core::observation::{ObservationInput, ObservationKind, ObservationSource};

    /// A legacy pending proposal authored by the removed
    /// `skill_propose` channel is consumed without installing — the
    /// model never wrote the library, and it does not start now.
    #[tokio::test]
    async fn legacy_agent_proposals_are_consumed_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let manager = Arc::new(SkillManager::new(state.clone()));
        // Hand-write a legacy agent-authored manifest; no code path
        // creates these anymore.
        let dir = state.join("skill-proposals/agent-made");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: agent-made\ndescription: Agent-drafted skill.\n---\n# Body\n\nSteps.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("proposal.toml"),
            "name = 'agent-made'\nstatus = 'pending'\ncluster_hash = 'c-agent'\ncreated_at = 1\nupdated_at = 1\nauthored_by = 'agent'\n",
        )
        .unwrap();

        let generation = manager.generation();
        let report = run_evolution_cycle(&manager, &EvolutionPolicy::default())
            .await
            .unwrap();
        assert!(report.installed.is_empty());
        assert_eq!(manager.generation(), generation);
        assert!(
            !state.join("skills/agent-made").exists(),
            "agent draft must not land in the library"
        );
        // Consumed: the cluster never re-proposes, and the manifest is
        // marked processed by hash.
        assert!(manager.proposals().processed_clusters().contains("c-agent"));
    }
}

/// Coverage for the unified channel: every skill command
/// (evolution triggers, explicit cycles) travels through one
/// `mpsc::Sender<SkillCommand>` into the worker task. These tests
/// pin the funnel invariants the design relies on.
#[cfg(test)]
mod channel_funnel {
    use super::*;
    use crate::proposals::ProposalStatus;
    use evolution_core::observation::ObservationInput;

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
    /// a `RunCycle` produces one installed skill. Returns its name
    /// (deterministic slug from `distill`).
    ///
    /// Caller is responsible for draining the Startup sweep
    /// broadcast BEFORE calling this — the Startup trigger's
    /// quiet-window drain would otherwise capture the RunCycle
    /// and create the proposal out from under us.
    async fn seed_installed_skill(manager: &SkillManager, handle: &SkillControlHandle) -> String {
        for body in ["fix a", "fix b", "fix c"] {
            manager
                .observation_hub()
                .record(ObservationInput {
                    kind: evolution_core::observation::ObservationKind::Failure,
                    source: evolution_core::observation::ObservationSource::Agent,
                    summary: "fix for boom".into(),
                    body: body.into(),
                    node_kind: Some("sql".into()),
                    error: Some("boom".into()),
                })
                .await
                .unwrap();
        }
        let outcome = handle.call_run_cycle(EvolutionPolicy::default()).await;
        let report = match outcome {
            SkillCommandOutcome::Ok(r) => r,
            other => panic!("seed cycle failed: {other:?}"),
        };
        assert_eq!(report.installed.len(), 1);
        report.installed[0].clone()
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
        let name = seed_installed_skill(&manager, &handle).await;
        // Drain the seed cycle broadcast.
        let _ = tokio::time::timeout(Duration::from_secs(2), reports.recv()).await;
        assert_eq!(name, "sql-boom");
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
            .observation_hub()
            .record(evolution_core::observation::ObservationInput {
                kind: evolution_core::observation::ObservationKind::Failure,
                source: evolution_core::observation::ObservationSource::Agent,
                summary: "fix for boom N".into(),
                body: "fix a".into(),
                node_kind: Some("sql".into()),
                error: Some("boom at step N".into()),
            })
            .await
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
