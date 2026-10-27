//! Request cancellation is independent of the storage/search queues so a
//! waiting HTTP request can close its actor reply without waiting its turn.
use crate::{Error, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Default)]
struct State {
    active: HashMap<String, Arc<watch::Sender<bool>>>,
    cancelled: HashMap<String, Instant>,
}
#[derive(Default)]
pub struct SearchCancellationRegistry {
    state: Mutex<State>,
}
pub struct SearchCancellationGuard {
    registry: Arc<SearchCancellationRegistry>,
    id: String,
    signal: Arc<watch::Sender<bool>>,
    receiver: watch::Receiver<bool>,
}
fn validate(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
    {
        return Err(Error::InvalidSearchQuery(
            "request ID must contain 1–128 ASCII letters, digits, '-', '_' or '.'".into(),
        ));
    }
    Ok(())
}
impl State {
    fn prune(&mut self) {
        self.cancelled
            .retain(|_, at| at.elapsed() < Duration::from_secs(60));
        while self.cancelled.len() >= 1024 {
            let oldest = self
                .cancelled
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.cancelled.remove(&id);
            }
        }
    }
}
impl SearchCancellationRegistry {
    pub fn register(self: &Arc<Self>, id: &str) -> Result<SearchCancellationGuard> {
        validate(id)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Unknown("search cancellation lock poisoned".into()))?;
        state.prune();
        if state.cancelled.remove(id).is_some() {
            return Err(Error::SearchCancelled);
        }
        if state.active.contains_key(id) {
            return Err(Error::InvalidSearchQuery(
                "request ID is already active".into(),
            ));
        }
        let (signal, receiver) = watch::channel(false);
        let signal = Arc::new(signal);
        state.active.insert(id.into(), signal.clone());
        Ok(SearchCancellationGuard {
            registry: self.clone(),
            id: id.into(),
            signal,
            receiver,
        })
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        validate(id)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Unknown("search cancellation lock poisoned".into()))?;
        state.prune();
        if let Some(signal) = state.active.remove(id) {
            signal.send_replace(true);
        } else {
            state.cancelled.insert(id.into(), Instant::now());
        }
        Ok(())
    }
}
impl SearchCancellationGuard {
    pub async fn cancelled(&mut self) {
        if *self.receiver.borrow() {
            return;
        }
        let _ = self.receiver.changed().await;
    }
}
impl Drop for SearchCancellationGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.registry.state.lock() {
            if state
                .active
                .get(&self.id)
                .is_some_and(|signal| Arc::ptr_eq(signal, &self.signal))
            {
                state.active.remove(&self.id);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_handles_early_delivery_duplicates_reuse_and_cleanup() {
        let registry = Arc::new(SearchCancellationRegistry::default());
        registry.cancel("early").unwrap();
        assert!(matches!(
            registry.register("early"),
            Err(Error::SearchCancelled)
        ));
        let mut first = registry.register("same").unwrap();
        assert!(registry.register("same").is_err());
        registry.cancel("same").unwrap();
        first.cancelled().await;
        let second = registry.register("same").unwrap();
        drop(first);
        assert!(
            registry.register("same").is_err(),
            "old cleanup must not remove a reused ID"
        );
        drop(second);
        assert!(registry.register("same").is_ok());
        for i in 0..1100 {
            registry.cancel(&format!("unused-{i}")).unwrap();
        }
        assert!(registry.state.lock().unwrap().cancelled.len() <= 1024);
    }
}
