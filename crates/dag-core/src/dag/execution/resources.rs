//! Local admission control: a weighted CPU/memory budget shared by every
//! task an executor accepts, with async backpressure while capacity is
//! held and release notification on drop.

use tokio::sync::Notify;

use super::contract::TaskResources;
use crate::dag::DagError;

#[derive(Debug)]
struct LocalResourceState {
    cpu_limit: Option<u32>,
    memory_limit_bytes: Option<u64>,
    cpu_used: u64,
    memory_used_bytes: u64,
    generation: u64,
}

#[derive(Debug)]
pub(super) struct LocalResourceBudget {
    state: std::sync::Arc<(std::sync::Mutex<LocalResourceState>, Notify)>,
}

impl Clone for LocalResourceBudget {
    fn clone(&self) -> Self {
        Self {
            state: std::sync::Arc::clone(&self.state),
        }
    }
}

impl LocalResourceBudget {
    pub(super) fn new(cpu_limit: Option<u32>, memory_limit_bytes: Option<u64>) -> Self {
        Self {
            state: std::sync::Arc::new((
                std::sync::Mutex::new(LocalResourceState {
                    cpu_limit,
                    memory_limit_bytes,
                    cpu_used: 0,
                    memory_used_bytes: 0,
                    generation: 0,
                }),
                Notify::new(),
            )),
        }
    }

    fn fits(request: u64, used: u64, limit: Option<u64>) -> Result<bool, DagError> {
        let Some(limit) = limit else {
            return Ok(true);
        };

        let limit = limit as u64;
        if request > limit {
            return Err(DagError::Schedule(format!(
                "task requests {request} but the local executor limit is {limit}"
            )));
        }
        Ok(request.saturating_add(used) <= limit)
    }

    pub(super) async fn acquire(
        &self,
        request: &TaskResources,
    ) -> Result<LocalResourceLease, DagError> {
        let cpus = request.cpus.map(u64::from).unwrap_or(0);
        let memory_bytes = request.memory_bytes.unwrap_or(0);
        loop {
            let observed_generation = {
                let mut state = self.state.0.lock().unwrap();
                let cpu_available =
                    Self::fits(cpus, state.cpu_used, state.cpu_limit.map(u64::from))?;
                let memory_available = Self::fits(
                    memory_bytes,
                    state.memory_used_bytes,
                    state.memory_limit_bytes,
                )?;
                if cpu_available && memory_available {
                    state.cpu_used += cpus;
                    state.memory_used_bytes += memory_bytes;
                    let generation = state.generation;
                    return Ok(LocalResourceLease {
                        state: std::sync::Arc::clone(&self.state),
                        cpus,
                        memory_bytes,
                        generation,
                    });
                }
                state.generation
            };

            let notified = self.state.1.notified();
            let unchanged = {
                let state = self.state.0.lock().unwrap();
                state.generation == observed_generation
            };
            if unchanged {
                notified.await;
            }
        }
    }
}

pub(super) struct LocalResourceLease {
    state: std::sync::Arc<(std::sync::Mutex<LocalResourceState>, Notify)>,
    cpus: u64,
    memory_bytes: u64,
    #[allow(dead_code)]
    generation: u64,
}

impl Drop for LocalResourceLease {
    fn drop(&mut self) {
        {
            let mut state = self.state.0.lock().unwrap();
            state.cpu_used = state.cpu_used.saturating_sub(self.cpus);
            state.memory_used_bytes = state.memory_used_bytes.saturating_sub(self.memory_bytes);
            state.generation += 1;
        }
        self.state.1.notify_waiters();
    }
}
