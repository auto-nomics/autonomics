use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
#[derive(Default)]
struct State {
    active: HashMap<String, (u64, oneshot::Sender<()>)>,
    early: HashMap<String, Instant>,
}
#[derive(Default)]
pub struct Requests {
    state: Mutex<State>,
    next: AtomicU64,
}
pub struct Guard {
    requests: Arc<Requests>,
    id: String,
    ticket: u64,
}
fn valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
}
impl State {
    fn prune(&mut self) {
        self.early
            .retain(|_, at| at.elapsed() < Duration::from_secs(60));
        while self.early.len() >= 1024 {
            let id = self
                .early
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(id, _)| id.clone());
            if let Some(id) = id {
                self.early.remove(&id);
            }
        }
    }
}
impl Requests {
    pub fn start(self: &Arc<Self>, id: &str) -> Result<(Guard, oneshot::Receiver<()>), String> {
        if !valid(id) {
            return Err("Invalid request ID".into());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Request cancellation lock poisoned")?;
        state.prune();
        if state.early.remove(id).is_some() {
            return Err("Request cancelled".into());
        }
        if state.active.contains_key(id) {
            return Err("Request ID is already active".into());
        }
        let ticket = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        state.active.insert(id.into(), (ticket, tx));
        Ok((
            Guard {
                requests: self.clone(),
                id: id.into(),
                ticket,
            },
            rx,
        ))
    }
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        if !valid(id) {
            return Err("Invalid request ID".into());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Request cancellation lock poisoned")?;
        state.prune();
        if let Some((_, tx)) = state.active.remove(id) {
            let _ = tx.send(());
        } else {
            state.early.insert(id.into(), Instant::now());
        }
        Ok(())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.requests.state.lock() {
            if state
                .active
                .get(&self.id)
                .is_some_and(|(ticket, _)| *ticket == self.ticket)
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
    async fn native_cancellation_covers_early_active_duplicate_and_reused_ids() {
        let requests = Arc::new(Requests::default());
        requests.cancel("early").unwrap();
        assert!(requests.start("early").is_err());
        let (guard, rx) = requests.start("active").unwrap();
        assert!(requests.start("active").is_err());
        requests.cancel("active").unwrap();
        rx.await.unwrap();
        let (new, _) = requests.start("active").unwrap();
        drop(guard);
        assert!(requests.start("active").is_err());
        drop(new);
        assert!(requests.start("active").is_ok());
    }
}
