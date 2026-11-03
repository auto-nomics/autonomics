// Verify history loads when the context announcement follows agent selection.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1426';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1426', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  for (let attempt = 0; !output.includes(origin); attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  await page.addInitScript(() => {
    const original = window.fetch;
    window.fetch = (input, init) => {
      if (String(input).endsWith('/api/v1/events')) {
        return Promise.resolve(new Response(new ReadableStream({
          start(controller) { window.sendTestEvent = (event) => controller.enqueue(new TextEncoder().encode(event)); },
        }), { headers: { 'Content-Type': 'text/event-stream' } }));
      }
      return original(input, init);
    };
  });
  const stored = { id: 'a2', name: '/root/beta', config_json: {}, created_at: 1, last_active: 1 };
  await page.route('**/api/v1/state', route => route.fulfill({ json: {
    last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: {},
    display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
  } }));
  await page.route('**/api/v1/storage/agents*', route => route.fulfill({ json: [stored] }));
  await page.route('**/api/v1/agents', route => route.fulfill({ json: { path: '/root/beta' } }));
  let releaseHistory;
  let historyRequested;
  const requested = new Promise(resolve => { historyRequested = resolve; });
  const release = new Promise(resolve => { releaseHistory = resolve; });
  await page.route('**/api/v1/agents/a2/sessions/s2/history', async route => {
    historyRequested();
    await release;
    await route.fulfill({ json: [
      { id: 'm1', role: 'assistant', content: [{ type: 'text', text: 'restored answer' }] },
    ] });
  });
  await page.route('**/api/v1/bib/**', route => route.fulfill({ json: { collections: [], articles: [], total: 0 } }));
  await page.goto(origin);
  await page.getByRole('button', { name: 'Search agents' }).click();
  await page.locator('[data-slot="command-item"]').filter({ hasText: '/root/beta' }).click();
  await page.getByRole('button', { name: 'beta', exact: true }).waitFor();
  await page.evaluate(() => window.sendTestEvent(`id: 1\nevent: agent\ndata: ${JSON.stringify({ agent: '/root/beta', event: { ContextList: { context: { id: 's2', created_at: 1, last_active: 1 } } } })}\n\n`));
  await requested;
  await page.evaluate(() => window.sendTestEvent(`id: 2\nevent: agent\ndata: ${JSON.stringify({ agent: '/root/beta', event: { UserMessageAcknowledged: 'live prompt' } })}\n\nid: 3\nevent: agent\ndata: ${JSON.stringify({ agent: '/root/beta', event: { TextDelta: 'live answer' } })}\n\n`));
  // Let the event stream deliver both events while the history is blocked.
  await page.waitForTimeout(100);
  releaseHistory();
  await page.getByText('restored answer').waitFor({ state: 'visible' });
  await page.getByText('live prompt').waitFor({ state: 'visible' });
  await page.getByText('live answer').waitFor({ state: 'visible' });
  assert.equal(await page.getByText('restored answer').count(), 1);
  console.log('agent-resume-history: passed');
} finally {
  await browser?.close();
  server.kill();
}
