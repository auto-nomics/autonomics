//! Learning-rate schedulers — step, cosine, plateau, warmup-cosine.
//!
//! Pure Rust (no Burn dependency). The training loop calls `lr()` each epoch
//! to get the current learning rate, which is then applied to the Burn
//! optimizer.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SchedulerConfig {
    None,
    Step {
        step_size: usize,
        gamma: f64,
    },
    Cosine {
        max_epochs: usize,
    },
    Plateau {
        factor: f64,
        patience: usize,
        min_lr: f64,
    },
    WarmupCosine {
        warmup_steps: usize,
        max_steps: usize,
    },
}

pub struct Scheduler {
    config: SchedulerConfig,
    base_lr: f64,
    current_lr: f64,
    bad_epochs: usize,
    best_metric: Option<f64>,
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

    pub fn lr(&self) -> f64 {
        self.current_lr
    }

    pub fn step(&mut self) -> f64 {
        self.step_count += 1;
        self.update_lr();
        self.current_lr
    }

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
                self.current_lr =
                    self.base_lr * 0.5 * (1.0 + (std::f64::consts::PI * progress).cos());
            }
            SchedulerConfig::Plateau {
                factor,
                patience,
                min_lr,
            } => {
                if self.bad_epochs >= *patience {
                    self.current_lr = (self.current_lr * factor).max(*min_lr);
                    self.bad_epochs = 0;
                }
            }
            SchedulerConfig::WarmupCosine {
                warmup_steps,
                max_steps,
            } => {
                if self.step_count <= *warmup_steps {
                    self.current_lr =
                        self.base_lr * (self.step_count as f64 / *warmup_steps as f64);
                } else {
                    let cosine_progress = (self.step_count - warmup_steps) as f64
                        / (*max_steps - warmup_steps) as f64;
                    let cosine_progress = cosine_progress.min(1.0);
                    self.current_lr =
                        self.base_lr * 0.5 * (1.0 + (std::f64::consts::PI * cosine_progress).cos());
                }
            }
        }
    }

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
