pub fn start(manager: Arc<SkillManager>, options: EvolutionOptions) -> SkillControlHandle {
    // Capacity 128: a burst of triggers and explicit RunCycle
    // requests during a slow cycle must not push commands out the
    // back door.
    let (tx, rx) = mpsc::channel::<SkillCommand>(128);
    let (report_tx, _) = broadcast::channel::<Arc<EvolutionReport>>(16);
    let dropped = Arc::new(AtomicU64::new(0));
    let monitor = Arc::new(CycleMonitor::default());

    // Snapshot the introspection copy before the worker owns the rest.
    let policy = options.policy.clone();
    let timer = options.timer;

    let worker = Worker {
        manager,
        options,
        reports: report_tx.clone(),
        monitor: Arc::clone(&monitor),
    };
    tokio::spawn(worker.run(rx));

    let handle = SkillControlHandle::new(tx, dropped, report_tx, policy, timer, monitor);
    handle.trigger(EvolutionTrigger::Startup);
    handle
}

struct Worker {
    manager: Arc<SkillManager>,
    options: EvolutionOptions,
    reports: broadcast::Sender<Arc<EvolutionReport>>,
    monitor: Arc<CycleMonitor>,
}

impl Worker {
    async fn run(self, mut rx: mpsc::Receiver<SkillCommand>) {
        // Consume the interval's immediate first tick — the explicit
        // Startup trigger already performs the boot sweep.
        let mut timer = self.options.timer.map(tokio::time::interval);
        if let Some(interval) = timer.as_mut() {
            interval.tick().await;
        }
        loop {
            // First command of a new iteration: either an inbound
            // command or a timer-driven trigger.
            let first = match &mut timer {
                Some(interval) => {
                    tokio::select! {
                        cmd = rx.recv() => match cmd {
                            Some(cmd) => cmd,
                            None => break,
                        },
                        _ = interval.tick() => SkillCommand::Evolution(EvolutionTrigger::Timer),
                    }
                }
                None => match rx.recv().await {
                    Some(cmd) => cmd,
                    None => break,
                },
            };

            match first {
                SkillCommand::Evolution(trigger) => {
                    // Quiet-window drain: collect triggers into the
                    // batch; remember the first `RunCycle` we see so
                    // it runs after the coalesced cycle, still
                    // skipping coalescing itself.
                    let mut triggers = vec![trigger];
                    self.monitor.coalescing(triggers.len());
                    let mut run_cycle: Option<(
                        EvolutionPolicy,
                        oneshot::Sender<SkillCommandOutcome<EvolutionReport>>,
                    )> = None;
                    let deadline = tokio::time::Instant::now() + self.options.quiet_window;
                    loop {
                        let remain =
                            deadline.saturating_duration_since(tokio::time::Instant::now());
                        if remain.is_zero() {
                            break;
                        }
                        match tokio::time::timeout(remain, rx.recv()).await {
                            Ok(Some(SkillCommand::Evolution(t))) => {
                                triggers.push(t);
                                self.monitor.coalescing(triggers.len());
                            }
                            Ok(Some(cmd @ SkillCommand::RunCycle { .. })) => {
                                // First explicit cycle wins; subsequent
                                // RunCycle commands wait for the next
                                // iteration rather than stacking here.
                                if run_cycle.is_none()
                                    && let SkillCommand::RunCycle { policy, reply } = cmd
                                {
                                    run_cycle = Some((policy, reply));
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }

                    // 1) Coalesced cycle if any triggers accumulated.
                    if !triggers.is_empty() {
                        let labels: Vec<&'static str> =
                            triggers.iter().map(EvolutionTrigger::label).collect();
                        let summary = labels.join(",");
                        // Short-circuit when the observation pool is
                        // empty: there's nothing to cluster, no cycle
                        // body to run, no report to broadcast. Explicit
                        // RunCycle commands still go through the body
                        // below — the operator asked for it.
                        if run_cycle.is_none() && self.manager.observations().await.is_empty() {
                            tracing::debug!(
                                triggers = %summary,
                                "evolution cycle skipped: empty observation pool"
                            );
                            self.monitor.cycle_skipped_empty(&labels);
                            continue;
                        }
                        self.monitor.distilling(&labels);
                        let started = std::time::Instant::now();
                        let outcome = run_evolution_cycle_labeled(
                            &self.manager,
                            &self.options.policy,
                            labels.clone(),
                        )
                        .await;
                        self.monitor.cycle_finished(
                            &labels,
                            started.elapsed(),
                            match &outcome {
                                Ok(_) => None,
                                Err(e) => Some(e.to_string()),
                            },
                        );
                        match outcome {
                            Ok(report) => {
                                if report.acted() {
                                    tracing::info!(
                                        triggers = %summary,
                                        written = report.proposals_written.len(),
                                        installed = report.installed.len(),
                                        deferred = report.deferred,
                                        "evolution cycle"
                                    );
                                }
                                let _ = self.reports.send(Arc::new(report));
                            }
                            Err(e) => {
                                tracing::warn!(triggers = %summary, error = %e, "evolution cycle failed");
                            }
                        }
                    }

                    // 2) Explicit RunCycle (skip coalescing).
                    if let Some((policy, reply)) = run_cycle {
                        let labels = vec!["manual"];
                        self.monitor.distilling(&labels);
                        let started = std::time::Instant::now();
                        let outcome =
                            run_evolution_cycle_labeled(&self.manager, &policy, labels.clone())
                                .await;
                        self.monitor.cycle_finished(
                            &labels,
                            started.elapsed(),
                            match &outcome {
                                Ok(_) => None,
                                Err(e) => Some(e.to_string()),
                            },
                        );
                        let outcome = match outcome {
                            Ok(report) => {
                                let _ = self.reports.send(Arc::new(report.clone()));
                                SkillCommandOutcome::Ok(report)
                            }
                            Err(e) => SkillCommandOutcome::Err(e),
                        };
                        let _ = reply.send(outcome);
                    }
                }
                SkillCommand::RunCycle { policy, reply } => {
                    let labels = vec!["manual"];
                    self.monitor.distilling(&labels);
                    let started = std::time::Instant::now();
                    let outcome =
                        run_evolution_cycle_labeled(&self.manager, &policy, labels.clone()).await;
                    self.monitor.cycle_finished(
                        &labels,
                        started.elapsed(),
                        match &outcome {
                            Ok(_) => None,
                            Err(e) => Some(e.to_string()),
                        },
                    );
                    let outcome = match outcome {
                        Ok(report) => {
                            let _ = self.reports.send(Arc::new(report.clone()));
                            SkillCommandOutcome::Ok(report)
                        }
                        Err(e) => SkillCommandOutcome::Err(e),
                    };
                    let _ = reply.send(outcome);
                }
            }
        }
    }
}
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::manager::SkillManager;

use super::command::{SkillCommand, SkillCommandOutcome};
use super::handle::SkillControlHandle;
use super::model::{EvolutionOptions, EvolutionPolicy, EvolutionReport, EvolutionTrigger};
use super::monitor::CycleMonitor;
use super::workflow::run_evolution_cycle_labeled;
