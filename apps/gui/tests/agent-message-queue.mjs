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

const MOCK_PORT = 14401;
const origin = 'http://127.0.0.1:1438';

// ── Mock gateway ──
// Frames written before the browser attaches are buffered and flushed by
// the test at a chosen point, so they can never race the history load.
const snapshot = {
  last_seq: 0,
  agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }],
  contexts: { '/root/alpha': { id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 } },
  display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
};
let failNext = false;
let releaseSend;
let pushFrame = null;
let flush = null;
const buffered = [];
const mock = http.createServer((req, res) => {
  const path = new URL(req.url, 'http://mock').pathname;
  const json = (body) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
  if (path.endsWith('/messages') && req.method === 'POST') {
    req.resume();
    if (failNext) { failNext = false; res.writeHead(500); return res.end('Send failed'); }
    releaseSend = () => json({});
    return;
  }
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

const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1438', '--strictPort'], {
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
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());

  await page.goto(origin);
  await page.getByPlaceholder('输入消息…').waitFor({ state: 'visible' });
  // The history response (instant mock) has settled; flushing later frames
  // cannot be wiped by a late loadHistory overwrite.
  await page.waitForTimeout(300);
  assert.notEqual(flush, null, 'SSE connection must be established');

  const composer = page.getByPlaceholder('输入消息…');
  const queue = page.getByRole('region', { name: '待处理消息' });
  await composer.fill('First queued message');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await queue.getByText('发送中…').waitFor();
  while (!releaseSend) await new Promise(resolve => setTimeout(resolve, 10));
  releaseSend(); releaseSend = null;
  await queue.getByText('已排队 · 等待 Agent 接收').waitFor();
  await composer.fill('First queued message');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await queue.getByText('发送中…').waitFor();
  while (!releaseSend) await new Promise(resolve => setTimeout(resolve, 10));
  // A new prompt must close the previous response even without an idle event.
  sendFrame({ id: 1, event: 'agent', data: { agent: '/root/alpha', event: { TextDelta: 'Previous answer\n\n' } } });
  await page.locator('.md').getByText('Previous answer', { exact: true }).waitFor();

  // An acknowledgement can beat the HTTP response; remove only one copy.
  sendFrame({ id: 2, event: 'agent', data: { agent: '/root/alpha', event: { UserMessageAcknowledged: 'First queued message' } } });
  await page.waitForFunction(() => document.querySelector('[aria-label="待处理消息"] ol')?.children.length === 1);
  releaseSend(); releaseSend = null;
  await queue.getByText('已排队 · 等待 Agent 接收').waitFor();
  sendFrame({ id: 3, event: 'agent', data: { agent: '/root/alpha', event: { UserMessageAcknowledged: 'First queued message' } } });
  await queue.waitFor({ state: 'detached' });
  sendFrame({ id: 4, event: 'agent', data: { agent: '/root/alpha', event: { TextDelta: 'Next answer\n\n' } } });
  await page.locator('.md').getByText('Next answer', { exact: true }).waitFor();
  assert.equal(await page.locator('.md').getByText('Previous answer', { exact: true }).count(), 1);
  sendFrame({ id: 5, event: 'agent', data: { agent: '/root/alpha', event: { TextDelta: 'continued\n\n' } } });
  await page.locator('.md').getByText('continued', { exact: true }).waitFor();
  assert.equal(await page.locator('.md').filter({ hasText: 'Next answer' }).filter({ hasText: 'continued' }).count(), 1, 'deltas within one response share a row');

  sendFrame({ id: 6, event: 'agent', data: { agent: '/root/alpha', event: 'Done' } });
  await page.getByRole('button', { name: '发送', exact: true }).waitFor();
  failNext = true;
  await composer.fill('Failed message');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await page.getByText('Send failed', { exact: false }).waitFor();
  assert.equal(await queue.count(), 0);
  assert.equal(await composer.inputValue(), 'Failed message');
  assert.deepEqual(errors, []);
  console.log('agent-message-queue: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
  mock.close();
}
