//! Async terminal/render event loop.

use super::terminal::PANIC_OCCURRED;
use super::*;

/// Fixed-rate render clock driven by a dedicated OS thread.
///
/// `tokio::time::interval` only fires when a runtime worker advances the
/// time driver. While every worker is busy with CPU-bound or blocking DAG
/// work (container-node input staging, output hashing), the timer wheel
/// stalls — the interval stops firing and the screen freezes, even though
/// terminal input still reaches the main thread. This ticker sends each
/// tick through a bounded mpsc, so the sender wakes the `block_on`'d main
/// thread directly, with no runtime worker involved.
pub(super) struct RenderTicker {
    tick_rx: tokio::sync::mpsc::Receiver<()>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RenderTicker {
    fn start() -> Self {
        const TICK: std::time::Duration = std::time::Duration::from_millis(16);
        let (tick_tx, tick_rx) = tokio::sync::mpsc::channel::<()>(1);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("tui-render-tick".into())
            .spawn(move || {
                // First tick completes immediately (guaranteeing the initial
                // frame; the constructor sets `dirty = true`). After that,
                // capacity-1 + `try_send` reproduces `MissedTickBehavior::
                // Skip`: ticks arriving while the loop is busy are dropped
                // instead of piling up.
                let _ = tick_tx.try_send(());
                while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                    let _ = tick_tx.try_send(());
                }
            })
            .expect("failed to spawn render tick thread");
        Self {
            tick_rx,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for RenderTicker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl App {
    /// Async main loop driven by `tokio::select!` with a fixed-rate render tick.
    ///
    /// **Design**: state mutation and rendering are strictly separated.
    ///
    /// - **Event branches** (terminal, gateway, app) only mutate state and
    ///   set the `dirty` flag. They never render.
    /// - **Render tick** fires at a fixed interval (~60 fps) from a
    ///   dedicated OS thread (independent of tokio worker availability) and
    ///   only redraws when `dirty` is true *or* the agent is active
    ///   (animation frames). When idle and no events arrive, the loop parks
    ///   on `select!` and consumes zero CPU.
    ///
    /// The gateway branch replaces the three in-process host channels of
    /// the fat-client loop (agent events, host commands, host events):
    /// the daemon owns the host; the TUI sees the same events over SSE.
    pub(super) async fn run_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> std::io::Result<()> {
        use crossterm::event::EventStream;
        use tokio_stream::StreamExt;

        let mut event_stream = EventStream::new();

        // Fixed-rate render clock on a dedicated thread; see [`RenderTicker`].
        let mut ticker = RenderTicker::start();

        loop {
            if self.should_quit {
                break Ok(());
            }

            tokio::select! {
                biased;

                // ── Terminal input (keys, mouse, paste, resize) ──
                maybe_event = event_stream.next() => {
                    match maybe_event {
                        Some(Ok(event)) => {
                            let delta = self.handle_event(&event);
                            if delta != 0 {
                                self.apply_scroll_delta(delta);
                            }
                            // Most terminal events mutate state or at least require
                            // a redraw (e.g. resize). Setting dirty unconditionally
                            // is simpler and the worst case is one extra frame.
                            self.dirty = true;
                        }
                        Some(Err(e)) => {
                            // crossterm read error — log so it's not silently
                            // swallowed (the old code dropped `Err` entirely).
                            tracing::warn!(error = %e, "crossterm event read error");
                        }
                        None => {
                            // Stream exhausted (background thread died).
                            // Recreate immediately.
                            tracing::warn!("crossterm EventStream ended; recreating");
                            event_stream = EventStream::new();
                        }
                    }
                }

                // ── Fixed-rate render tick (dedicated thread) ──
                // Second in the biased order so event floods cannot
                // starve rendering.
                tick = ticker.tick_rx.recv() => {
                    if tick.is_none() {
                        // Ticker thread died; without ticks nothing would
                        // ever draw again. Exit cleanly instead of spinning
                        // on a closed channel.
                        tracing::error!("render tick thread terminated; exiting");
                        self.should_quit = true;
                        break Ok(());
                    }
                    // If a background thread panicked, the global panic
                    // hook has already restored the terminal. Stop drawing
                    // immediately so we don't write TUI escape codes onto
                    // the restored terminal and garble the panic output.
                    if PANIC_OCCURRED.load(std::sync::atomic::Ordering::SeqCst) {
                        self.should_quit = true;
                        break Ok(());
                    }

                    let active_status = self.state.active_status();
                    let show_animation = active_status.is_active()
                        || active_status == AgentStatus::Waiting;
                    if !show_animation {
                        self.clear_cancel_pending();
                    } else {
                        let ts = self.state.active_tab_state_mut();
                        ts.frame = ts.frame.wrapping_add(1);
                    }

                    let agent_active = active_status.is_active()
                        || active_status == AgentStatus::Waiting;
                    if self.dirty || agent_active {
                        terminal.draw(|f| self.render(f))?;
                        self.dirty = false;
                    }
                }

                // ── Gateway frames (agent events / host events / notices) ──
                maybe_frame = self.gateway_rx.recv() => {
                    match maybe_frame {
                        Some(frame) => {
                            self.handle_gateway_frame(frame);
                            self.dirty = true;
                        }
                        None => {
                            // The pump only ends when the channel closes —
                            // i.e. the pump task gave up. Without it no
                            // agent updates can arrive; exit cleanly.
                            tracing::error!("gateway event pump terminated; exiting");
                            self.should_quit = true;
                        }
                    }
                }

                // ── App internal events ──
                maybe_app = self.app_event_rx.recv() => {
                    match maybe_app {
                        Some(event) => self.handle_app_event(event),
                        None => self.should_quit = true,
                    }
                    self.dirty = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RenderTicker;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ticker_delivers_under_worker_saturation() {
        // Peg every worker with a CPU-bound task that never yields — the
        // condition under which a tokio interval stops firing.
        let pegged: Vec<_> = (0..2)
            .map(|_| {
                tokio::spawn(async {
                    let end = std::time::Instant::now() + Duration::from_millis(300);
                    while std::time::Instant::now() < end {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        // Give the workers a moment to pick the pegged tasks up, so the
        // saturation is in effect before the discriminating assertion.
        std::thread::sleep(Duration::from_millis(50));

        let mut ticker = RenderTicker::start();
        assert!(ticker.tick_rx.recv().await.is_some()); // first tick is immediate
        // The discriminating assertion: a *later* tick must still arrive
        // while all workers are saturated (an interval-based tick would not).
        assert!(
            tokio::time::timeout(Duration::from_secs(2), ticker.tick_rx.recv())
                .await
                .is_ok()
        );
        for task in pegged {
            task.await.unwrap();
        }
    }
}
