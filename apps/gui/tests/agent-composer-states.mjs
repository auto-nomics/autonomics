// Run with npm run test:agent-composer. Install a browser with
// npx playwright install chromium, or set PDF_TEST_CHROMIUM to an
// existing Chromium executable.
//
// Covers the composer action/state machine over a live mock gateway:
//   idle        → round send button, no ring, no terminated chip,
//   STREAMING   → send swaps to 终止 (cancel) with a rotating status ring
//                 (sky while streaming); the status chip shows streaming…,
//   click 终止   → POST /agents/{path}/cancel fires and the draft survives,
//   CANCELLED   → send returns, the 「已终止」 chip appears until the next
//                 turn,
//   composer icons → 压缩对话 POSTs /compact, DAG(fork) icon lives in the
//                 composer now (not the tab strip).
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import http from 'node:http';

const MOCK_PORT = 14399;
const origin = 'http://127.0.0.1:1427';

const snapshot = {
  last_seq: 0,
  agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }],
  contexts: { '/root/alpha': { id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 } },
  display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
};
let pushFrame = null;
let flush = null;
const buffered = [];
const posted = { cancel: 0, compact: 0 };
const mock = http.createServer((req, res) => {
  const path = decodeURIComponent(new URL(req.url, 'http://mock').pathname);
  const json = (body) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
  if (path === '/api/session') return json({});
  if (path === '/api/v1/state') return json(snapshot);
  if (path === '/api/v1/agents/a1/sessions/s1/history') return json({ messages: [] });
  if (path === '/api/v1/bib/collections/changes') return json({ revision: 0 });
  if (path === '/api/v1/agents//root/alpha/cancel' && req.method === 'POST') {
    posted.cancel += 1;
    res.writeHead(202); res.end(); return;
  }
  if (path === '/api/v1/agents//root/alpha/compact' && req.method === 'POST') {
    posted.compact += 1;
    res.writeHead(202); res.end(); return;
  }
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
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());

  await page.goto(origin);
  const composer = page.getByPlaceholder('输入消息…');
  await composer.waitFor({ state: 'visible' });
  await page.waitForTimeout(300);
  assert.notEqual(flush, null, 'SSE connection must be established');

  // ── idle: plain send button, nothing else. ──
  const send = page.getByRole('button', { name: '发送' });
  assert.ok(await send.isDisabled(), 'send is disabled with an empty draft');
  assert.equal(await page.getByRole('button', { name: '终止' }).count(), 0, 'no cancel button while idle');
  assert.equal(await page.locator('[data-status-ring]').count(), 0, 'no ring while idle');
  assert.equal(await page.getByText('已终止').count(), 0, 'no terminated chip while idle');

  // ── a turn starts: send becomes cancel with a rotating status ring. ──
  sendFrame({ id: 1, event: 'agent', data: { agent: '/root/alpha', event: { LifecycleChanged: 'STREAMING' } } });
  flush();
  const cancel = page.getByRole('button', { name: '终止' });
  await cancel.waitFor({ state: 'visible' });
  assert.equal(await page.getByRole('button', { name: '发送' }).count(), 0, 'send hides while a turn runs');
  const ring = page.locator('[data-status-ring]');
  await ring.waitFor({ state: 'visible' });
  assert.ok(
    await ring.evaluate((el) => el.classList.contains('border-sky-400')),
    'the streaming ring is sky-colored',
  );
  await page.getByText('streaming…').waitFor({ state: 'visible' });

  // ── cancel: fires the POST and keeps the draft (it is not a submit). ──
  await composer.fill('未完成的草稿');
  await cancel.click();
  await page.waitForTimeout(300);
  assert.equal(posted.cancel, 1, 'cancel POSTed to /agents/{path}/cancel');
  assert.equal(await composer.inputValue(), '未完成的草稿', 'cancel must not clear the draft');

  // ── CANCELLED: send returns, the terminated chip appears. ──
  sendFrame({ id: 2, event: 'agent', data: { agent: '/root/alpha', event: { LifecycleChanged: 'CANCELLED' } } });
  flush();
  await page.getByText('已终止').waitFor({ state: 'visible' });
  assert.equal(await page.getByRole('button', { name: '终止' }).count(), 0, 'cancel button gone after termination');
  assert.equal(await page.locator('[data-status-ring]').count(), 0, 'ring gone after termination');
  assert.ok(!(await send.isDisabled()), 'send is enabled again with a draft');

  // ── composer action icons: compact POSTs, DAG icon lives here now. ──
  await page.getByRole('button', { name: '压缩对话' }).click();
  await page.waitForTimeout(200);
  assert.equal(posted.compact, 1, 'compact POSTed to /agents/{path}/compact');
  assert.equal(await page.getByRole('button', { name: '查看 agent DAG' }).count(), 1, 'DAG (fork) icon is in the composer');

  assert.deepEqual(errors, []);
  console.log('agent-composer-states: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
  mock.close();
}
