// Run with npm run test:agent-tab-picker. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
// Covers the agent tab bar's search button and the inline agent picker.
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
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    if (output.includes(origin)) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = []; const popups = []; const sends = []; let stored = [];
  page.on('popup', popup => popups.push(popup));
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());

  const agentState = (path, id, status) => ({ path, agent_id: id, status });
  await page.route(`${origin}/api/session`, route => route.fulfill({ json: {} }));
  await page.route('**/api/v1/events', () => new Promise(() => {}));
  await page.route('**/api/v1/state', route => route.fulfill({ json: {
    last_seq: 0,
    agents: [
      agentState('/root/alpha', 'a1', 'Idle'),
      agentState('/root/beta', 'a2', 'Streaming'),
      agentState('/root/gamma', 'a3', 'Idle'),
    ],
    contexts: {
      '/root/alpha': { id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 },
      '/root/beta': { id: 's2', title: 'Beta main', created_at: 1, last_active: 3 },
      '/root/gamma': { id: 's3', title: 'Gamma main', created_at: 1, last_active: 4 },
    },
    display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
  } }));
  for (const [index, [id, answer]] of [['a1', 'alpha answer'], ['a2', 'beta answer'], ['a3', 'gamma answer']].entries()) {
    await page.route(`**/api/v1/agents/${id}/sessions/s${index + 1}/history`, route => route.fulfill({ json: { messages: [
      { id: `m-${id}`, role: 'assistant', content: [{ type: 'text', text: answer }] },
    ] } }));
  }
  await page.route('**/api/v1/storage/agents**', route => route.fulfill({ json: stored }));
  await page.route('**/api/v1/agents', route => route.fulfill({ json: { path: '/root/delta' } }));
  await page.route('**/api/v1/agents/**/messages', route => { sends.push({ url: route.request().url(), body: route.request().postDataJSON() }); return route.fulfill({ json: {} }); });
  await page.route('**/api/v1/bib/**', route => route.fulfill({ json: { collections: [], articles: [], total: 0 } }));

  await page.goto(origin);
  await page.getByPlaceholder('输入消息…').waitFor({ state: 'visible' });
  await page.getByText('alpha answer').waitFor({ state: 'visible' });

  // Viewport tabs: only the initially opened agent renders as a tab —
  // live-but-unopened agents (beta, gamma) do NOT auto-open tabs. Tabs
  // live in the workbench editor strip; title = full path.
  const tab = (name) => page.locator('[data-editor-group] [role="tab"][title="/root/' + name + '"]');
  await tab('alpha').waitFor({ state: 'visible' });
  assert.equal(await tab('beta').count(), 0, 'live agent without an open view has no tab');

  const search = page.getByRole('button', { name: 'Search agents' });
  assert.equal(await search.getAttribute('aria-expanded'), 'false', 'picker starts closed');

  // Open: the tab strip is replaced by the picker input + agent list.
  await search.click();
  const pickerInput = page.locator('[data-slot="command-input"]');
  await pickerInput.waitFor({ state: 'visible' });
  assert.equal(await search.getAttribute('aria-expanded'), 'true', 'search button reports expanded');
  // The workspace picker overlays the workspace header; workbench editor
  // tabs (the unified strip) stay mounted beside it.
  assert.equal(await tab('alpha').count(), 1, 'workbench tabs stay while picking');
  const items = page.locator('[data-slot="command-item"]');
  // One new-agent entry + the open tab (alpha) + the two live-unopened
  // agents (beta, gamma); no stored identities (page mocked empty).
  assert.equal(await items.count(), 4, 'new-agent + open + live-unopened rows');

  // The active agent carries a check marker.
  assert.equal(await items.filter({ hasText: 'alpha' }).getAttribute('data-checked'), 'true', 'active agent is marked');

  // Filtering narrows the list; Enter switches to the match, opens its
  // tab, and closes the picker.
  await pickerInput.fill('bet');
  assert.equal(await items.count(), 1, 'filter narrows to beta');
  await pickerInput.press('Enter');
  await page.getByText('beta answer').waitFor({ state: 'visible' });
  await pickerInput.waitFor({ state: 'detached' });
  await tab('beta').waitFor({ state: 'visible' });
  assert.equal(await page.getByRole('tab', { name: 'beta', exact: true }).count(), 1, 'tab strip returns after picking');

  // Reopen: clicking an item directly also switches and closes.
  await search.click();
  await page.locator('[data-slot="command-item"]').filter({ hasText: 'gamma' }).click();
  await page.getByText('gamma answer').waitFor({ state: 'visible' });
  await page.locator('[data-slot="command-input"]').waitFor({ state: 'detached' });

  // Escape closes without switching.
  await search.click();
  await page.locator('[data-slot="command-input"]').fill('alpha');
  await page.locator('[data-slot="command-input"]').press('Escape');
  await page.locator('[data-slot="command-input"]').waitFor({ state: 'detached' });
  await page.getByText('gamma answer').waitFor({ state: 'visible' });

  // Two agent groups must render their own transcripts and independent drafts.
  await page.getByRole('button', { name: '拆分编辑器' }).click();
  const main = page.locator('[data-editor-group="main"]');
  const secondary = page.locator('[data-editor-group="secondary"]');
  await secondary.getByText('gamma answer').waitFor();
  await main.getByText('beta answer').waitFor();
  const betaComposer = main.getByPlaceholder('输入消息…');
  const gammaComposer = secondary.getByPlaceholder('输入消息…');
  await betaComposer.fill('beta draft');
  await gammaComposer.fill('gamma draft');
  assert.equal(await betaComposer.inputValue(), 'beta draft');
  assert.equal(await gammaComposer.inputValue(), 'gamma draft');
  const betaSend = page.waitForRequest(request => request.method() === 'POST' && decodeURIComponent(request.url()).includes('/root/beta/messages'));
  await betaComposer.press('Control+Enter');
  await betaSend;
  await page.waitForFunction(() => document.querySelector('[data-editor-group="main"] textarea')?.value === '');
  assert.ok(sends.some(item => decodeURIComponent(item.url).includes('/root/beta/messages') && item.body.text === 'beta draft'));
  assert.equal(await gammaComposer.inputValue(), 'gamma draft', 'sending from one group must not alter the other draft');

  // Selecting an already open agent in the other group focuses that group,
  // while the current group remains visible.
  await secondary.getByRole('button', { name: 'Search agents' }).click();
  await secondary.locator('[data-slot="command-item"]').filter({ hasText: 'alpha' }).click();
  await main.getByText('alpha answer').waitFor();
  await secondary.getByText('gamma answer').waitFor();
  assert.equal(await gammaComposer.inputValue(), 'gamma draft');

  // Resuming from the picker opens in the current workbench group, without
  // opening an external browser tab.
  stored = [{ id: 'a4', name: '/root/delta', config_json: { kind: 'researcher' }, created_at: 1, last_active: 1 }];
  await secondary.getByRole('button', { name: 'Search agents' }).click();
  await secondary.locator('[data-slot="command-item"]').filter({ hasText: 'delta' }).click();
  await secondary.getByRole('tab', { name: 'delta' }).waitFor();
  await main.getByText('alpha answer').waitFor();
  assert.equal(popups.length, 0, 'picker must not open browser tabs');
  assert.equal(page.context().pages().length, 1);

  assert.deepEqual(errors, []);
  console.log('agent-tab-picker: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
}
