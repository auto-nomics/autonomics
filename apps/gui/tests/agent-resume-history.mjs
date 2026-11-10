// Both clients hydrate the sole context before reading the shared history endpoint.
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
  page.setDefaultTimeout(10000);
  await page.route('**/api/v1/**', route => route.fulfill({ json: {} }));
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
    last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: { '/root/alpha': { id: 's1', created_at: 1, last_active: 1 } },
    display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
  } }));
  await page.route('**/api/v1/storage/agents*', route => route.fulfill({ json: [stored] }));
  await page.route('**/api/v1/agents/**/context', route => route.fulfill({ json: { agent_id: 'a2', context: { id: 's2', created_at: 1, last_active: 1 } } }));
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
  await page.getByRole('tab', { name: 'beta', exact: true }).waitFor();
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
  await page.close();

  // Resume must use the selected identity's ID even if a later bounded
  // search omits it. Live agents also already have an ID in /state.
  for (const scenario of ['dormant', 'live', 'reload', 'context-error', 'no-context']) {
    const view = await browser.newPage();
    view.setDefaultTimeout(5000);
    const errors = [];
    view.on('pageerror', error => errors.push(error.message));
    await view.addInitScript(({ reload }) => {
      if (reload) localStorage.setItem('autonomics.openTabs', JSON.stringify(['/root/beta']));
      const original = window.fetch;
      window.fetch = (input, init) => String(input).endsWith('/api/v1/events')
        ? Promise.resolve(new Response(new ReadableStream(), { headers: { 'Content-Type': 'text/event-stream' } }))
        : original(input, init);
    }, { reload: scenario === 'reload' });
    let searches = 0, sessionRequests = 0, contextRequests = 0, historyRequests = 0, activations = 0;
    await view.route('**/api/v1/**', route => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith('/state')) return route.fulfill({ json: {
        last_seq: 0,
        agents: [
          { path: '/root/alpha', agent_id: 'a1', status: 'Idle' },
          ...(scenario === 'live' ? [{ path: '/root/beta', agent_id: 'a2', status: 'Idle' }] : []),
        ],
        contexts: { '/root/alpha': { id: 's1', created_at: 1, last_active: 1 } },
      } });
      if (url.pathname.endsWith('/storage/agents')) {
        if (url.searchParams.has('q')) {
          searches++;
          return route.fulfill({ json: scenario === 'reload' ? [stored] : [] });
        }
        return route.fulfill({ json: [stored] });
      }
      if (url.pathname.endsWith('/storage/agents/a2/sessions')) {
        sessionRequests++;
        return route.fulfill({ status: 500, json: { error: 'clients must not select from historical sessions' } });
      }
      if (url.pathname.endsWith('/context')) {
        contextRequests++;
        return scenario === 'context-error'
          ? route.fulfill({ status: 500, json: { error: 'context storage unavailable' } })
          : route.fulfill({ json: { agent_id: 'a2', context: scenario === 'no-context' ? null : { id: 's2', created_at: 1, last_active: 1 } } });
      }
      if (url.pathname.endsWith('/agents/a2/sessions/s2/history')) {
        historyRequests++;
        return route.fulfill({ json: [
          { id: 'u1', role: 'user', content: [{ type: 'text', text: 'previous question' }] },
          { id: 'm1', role: 'assistant', content: [{ type: 'text', text: 'previous answer' }] },
        ] });
      }
      if (route.request().method() === 'POST' && url.pathname.endsWith('/agents')) activations++;
      return route.fulfill({ json: {} });
    });
    await view.goto(origin);
    if (scenario !== 'reload') {
      await view.getByRole('button', { name: 'Search agents' }).click();
      await view.locator('[data-slot="command-item"]').filter({ hasText: 'beta' }).click();
    }
    if (scenario === 'context-error') {
      await view.getByText(/加载历史失败:.*context storage unavailable/).waitFor();
      assert.equal(historyRequests, 0);
    } else if (scenario === 'no-context') {
      await view.locator('[data-agent-workspace="/root/beta"]').getByText('This session has no messages yet').waitFor();
      await view.waitForTimeout(100);
      assert.equal(historyRequests, 0);
    } else {
      await view.getByText('previous question', { exact: true }).waitFor();
      await view.getByText('previous answer', { exact: true }).waitFor();
      assert.equal(historyRequests, 1, `${scenario}: history loads once without a context event`);
    }
    assert.equal(searches, 0, `${scenario}: hydration never searches identities`);
    assert.equal(sessionRequests, 0, `${scenario}: hydration never selects from historical sessions`);
    assert.equal(contextRequests, 1, `${scenario}: hydrate through the same context endpoint as the TUI`);
    assert.equal(activations, 0, 'reading history must preserve lazy activation');
    assert.deepEqual(errors, []);
    await view.close();
  }
  console.log('agent-resume-history: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
}
