//! Learning-rate schedulers — step, cosine, plateau, warmup-cosine.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SchedulerConfig {
    None,
    Step { step_size: usize, gamma: f64 },
    Cosine { max_epochs: usize },
    Plateau { factor: f64, patience: usize, min_lr: f64 },
    WarmupCosine { warmup_steps: usize, max_steps: usize },
}

pub struct Scheduler {
    config: SchedulerConfig,
    base_lr: f64,
    current_lr: f64,
    /// For plateau: epochs without improvement.
    bad_epochs: usize,
    /// For plateau: best metric seen so far.
    best_metric: Option<f64>,
    /// Global step counter.
    step_count: usize,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig, base_lr: f64) -> Self {
        let current_lr = base_lr;
        Self {
            config,
            base_lr,
            current_lr,
            bad_epochs: 0,
            best_metric: None,
            step_count: 0,
        }
    }

    /// Current learning rate.
    pub fn lr(&self) -> f64 {
        self.current_lr
    }

    /// Advance one epoch/step and return the new LR.
    pub fn step(&mut self) -> f64 {
        self.step_count += 1;
        self.update_lr();
        self.current_lr
    }

    /// For plateau scheduler: report the current epoch's metric.
    pub fn report_metric(&mut self, metric: f64, mode: SchedulerMode) {
        let improved = match (&self.best_metric, mode) {
            (None, _) => true,
            (Some(best), SchedulerMode::Min) => metric < *best,
            (Some(best), SchedulerMode::Max) => metric > *best,
        };
        if improved {
            self.best_metric = Some(metric);
            self.bad_epochs = 0;
        } else {
            self.bad_epochs += 1;
        }
    }

    fn update_lr(&mut self) {
        match &self.config {
            SchedulerConfig::None => {}
            SchedulerConfig::Step { step_size, gamma } => {
                if self.step_count % step_size == 0 && self.step_count > 0 {
                    self.current_lr *= gamma;
                }
            }
            SchedulerConfig::Cosine { max_epochs } => {
                let progress = self.step_count as f64 / *max_epochs as f64;
                let progress = progress.min(1.0);
                self.current_lr = self.base_lr * 0.5 * (1.0 + (std::f64::consts::PI * progress).cos());
            }
            SchedulerConfig::Plateau { factor, patience, min_lr } => {
                if self.bad_epochs >= *patience {
                    self.current_lr = (self.current_lr * factor).max(*min_lr);
                    self.bad_epochs = 0; // reset after reduction
                }
            }
            SchedulerConfig::WarmupCosine { warmup_steps, max_steps } => {
                if self.step_count <= *warmup_steps {
                    // Linear warmup.
                    self.current_lr = self.base_lr * (self.step_count as f64 / *warmup_steps as f64);
                } else {
                    let cosine_progress =
                        (self.step_count - warmup_steps) as f64 / (*max_steps - warmup_steps) as f64;
                    let cosine_progress = cosine_progress.min(1.0);
                    self.current_lr = self.base_lr * 0.5 * (1.0 + (std::f64::consts::PI * cosine_progress).cos());
                }
            }
        }
    }

    /// Reset internal state for a new training run.
    pub fn reset(&mut self) {
        self.current_lr = self.base_lr;
        self.bad_epochs = 0;
        self.best_metric = None;
        self.step_count = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerMode {
    Min,
    Max,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_scheduler() {
        let mut sched = Scheduler::new(
            SchedulerConfig::Step { step_size: 10, gamma: 0.5 },
            0.1,
        );
        for _ in 0..10 {
            sched.step();
        }
        // After 10 steps, lr should be halved.
        assert!((sched.lr() - 0.05).abs() < 1e-10);
    }

    #[test]
    fn test_cosine_scheduler() {
        let mut sched = Scheduler::new(
            SchedulerConfig::Cosine { max_epochs: 100 },
            0.1,
        );
        // At step 0: lr ≈ base_lr.
        // After half the epochs: lr ≈ base_lr/2.
        sched.step();
        let lr_start = sched.lr();
        for _ in 1..50 {
            sched.step();
        }
        let lr_mid = sched.lr();
        assert!(lr_mid < lr_start, "lr should decrease");
        assert!(lr_mid > 0.0);
        // After all epochs: lr ≈ 0.
        for _ in 50..100 {
            sched.step();
        }
        assert!(sched.lr() < 0.001);
    }

    #[test]
    fn test_warmup_cosine() {
        let mut sched = Scheduler::new(
            SchedulerConfig::WarmupCosine { warmup_steps: 5, max_steps: 10 },
            0.1,
        );
        // During warmup, lr increases linearly.
        sched.step();
        let lr_1 = sched.lr();
        assert!(lr_1 > 0.0 && lr_1 < 0.1);
        for _ in 1..5 {
            sched.step();
        }
        // After warmup: lr should be near base_lr.
        assert!((sched.lr() - 0.1).abs() < 0.02, "after warmup lr = {}", sched.lr());
    }

    #[test]
    fn test_plateau_scheduler() {
        let mut sched = Scheduler::new(
            SchedulerConfig::Plateau { factor: 0.5, patience: 3, min_lr: 1e-6 },
            0.1,
        );
        // Report increasing losses (worse).
        for i in 0..6 {
            sched.report_metric(1.0 + i as f64 * 0.1, SchedulerMode::Min);
            sched.step();
        }
        // After 3 bad epochs + step, lr should reduce.
        assert!(sched.lr() < 0.1, "lr should decrease after plateau, got {}", sched.lr());
    }
}
