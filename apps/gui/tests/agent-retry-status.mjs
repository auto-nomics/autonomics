// Run with npm run test:agent-retry-status. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
// Covers live agent-frame rendering: RetryableError retry notices land in
// the transcript, and the host status frame ("running" + last_event
// "retrying (2/5): …") keeps the retrying indicator instead of wiping it.
// Frames flow through a mock gateway (vite proxies /api to it) over one
// open SSE connection — playwright's route fulfill buffers bodies, and a
// closed stream would trigger the reconnect re-hydrate, resetting state.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import http from 'node:http';

const MOCK_PORT = 14399;
const origin = 'http://127.0.0.1:1427';

// ── Mock gateway ──
// Frames written before the browser attaches are buffered and flushed by
// the test at a chosen point, so they can never race the history load.
const snapshot = {
  last_seq: 0,
  agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }],
  sessions: { '/root/alpha': [{ id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 }] },
  display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
};
let pushFrame = null;
let flush = null;
const buffered = [];
const mock = http.createServer((req, res) => {
  const path = new URL(req.url, 'http://mock').pathname;
  const json = (body) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
  if (path === '/api/session') return json({});
  if (path === '/api/v1/state') return json(snapshot);
  if (path === '/api/v1/agents/a1/sessions/s1/history') return json({ messages: [] });
  if (path === '/api/v1/bib/collections/changes') return json({ revision: 0 });
  if (path === '/api/v1/events') {
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
    pushFrame = (frame) => res.write(`id: ${frame.id}\nevent: ${frame.event}\ndata: ${JSON.stringify(frame.data)}\n\n`);
    flush = () => { while (buffered.length) pushFrame(buffered.shift()); };
    req.on('close', () => { pushFrame = null; flush = null; });
    return;
  }
  res.writeHead(404, { 'content-type': 'application/json' });
  res.end('{"error":"not mocked"}');
});

const sendFrame = (frame) => (pushFrame ? pushFrame(frame) : buffered.push(frame));

const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1427', '--strictPort'], {
  stdio: ['ignore', 'pipe', 'pipe'],
  env: { ...process.env, AUTONOMICS_HTTP_API_ADDR: `127.0.0.1:${MOCK_PORT}` },
});
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  mock.listen(MOCK_PORT, '127.0.0.1');
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    if (output.includes(origin)) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());

  await page.goto(origin);
  await page.getByPlaceholder('输入消息…').waitFor({ state: 'visible' });
  // The history response (instant mock) has settled; flushing later frames
  // cannot be wiped by a late loadHistory overwrite.
  await page.waitForTimeout(300);
  assert.notEqual(flush, null, 'SSE connection must be established');

  // Stage 1: a retryable API failure.
  sendFrame({ id: 1, event: 'agent', data: { agent: '/root/alpha', event: { RetryableError: { message: 'rate limited', attempt: 2, max_retries: 5 } } } });
  sendFrame({ id: 2, event: 'agent', data: { agent: '/root/alpha', event: { LifecycleChanged: 'RETRYING' } } });
  // Host projection collapses Retrying into Running; the retry lives only
  // in last_event. The UI must keep "retrying" from this frame.
  sendFrame({ id: 3, event: 'host', data: { kind: 'agent_status_changed', path: '/root/alpha', status: { kind: 'running' }, last_event: 'retrying (2/5): rate limited' } });
  flush();

  // The retry notice renders as an error-class transcript item…
  await assert.doesNotReject(() => page.getByText('第 2/5 次重试: rate limited').waitFor({ state: 'visible' }));
  // …and the status keeps "retrying" although the host frame said running
  // (workspace status row + inspector overview row both show it).
  assert.equal(await page.getByText('retrying', { exact: true }).count(), 2, 'status must read retrying');

  // Stage 2: the request ultimately fails for good.
  sendFrame({ id: 4, event: 'agent', data: { agent: '/root/alpha', event: { Error: 'fatal boom' } } });
  sendFrame({ id: 5, event: 'agent', data: { agent: '/root/alpha', event: { LifecycleChanged: 'ERROR' } } });
  sendFrame({ id: 6, event: 'host', data: { kind: 'agent_status_changed', path: '/root/alpha', status: { kind: 'failed', message: 'fatal boom' }, last_event: 'fatal boom' } });
  // The post-turn list_sessions handshake must not disturb the status.
  sendFrame({ id: 7, event: 'agent', data: { agent: '/root/alpha', event: { SessionList: { sessions: [{ id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 }] } } } });
  flush();

  // The fatal error renders and the status flips to error (both rows).
  await assert.doesNotReject(() => page.getByText('fatal boom').waitFor({ state: 'visible' }));
  await page.waitForTimeout(300);
  assert.equal(await page.getByText('error', { exact: true }).count(), 2, 'status must read error');
  assert.equal(await page.getByText('retrying', { exact: true }).count(), 0, 'retrying must not survive the fatal error');
  assert.equal(await page.locator('pre', { hasText: 'fatal boom' }).count(), 1, 'exactly one fatal error item');

  assert.deepEqual(errors, []);
  console.log('agent-retry-status: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
  mock.close();
}
