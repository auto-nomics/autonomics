// Observation windows bound reporting intervals, never the lifetime of a live model.
export async function observeLiveAgent({ poll, isAlive, windowMs, onWindow, onRetry = () => {}, intervalMs = 100, now = Date.now, sleep = ms => new Promise(resolve => setTimeout(resolve, ms)) }) {
  if (!Number.isSafeInteger(windowMs) || windowMs < 1) throw new Error('invalid observation window');
  const started = now();
  let deadline = started + windowMs;
  let windows = 0;
  while (true) {
    if (!isAlive()) throw new Error('agent host stopped during observation');
    try {
      if (await poll()) return;
    } catch (error) {
      // Retry transport failures only; terminal Agent states and assertions propagate.
      if (error?.code === 'ERR_ASSERTION' || !/ECONNRESET|ECONNREFUSED|ETIMEDOUT|fetch failed|socket hang up|Timeout \d+ms exceeded/i.test(String(error))) throw error;
      await onRetry({ elapsed_ms: now() - started, error: String(error) });
    }
    if (now() >= deadline) {
      await onWindow({ elapsed_ms: now() - started, windows: ++windows });
      deadline = now() + windowMs;
    }
    await sleep(intervalMs);
  }
}
