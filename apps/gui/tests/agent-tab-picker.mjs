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
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
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
    sessions: {
      '/root/alpha': [{ id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 }],
      '/root/beta': [{ id: 's2', title: 'Beta main', created_at: 1, last_active: 3 }],
      '/root/gamma': [{ id: 's3', title: 'Gamma main', created_at: 1, last_active: 4 }],
    },
    display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
  } }));
  for (const [index, [id, answer]] of [['a1', 'alpha answer'], ['a2', 'beta answer'], ['a3', 'gamma answer']].entries()) {
    await page.route(`**/api/v1/agents/${id}/sessions/s${index + 1}/history`, route => route.fulfill({ json: { messages: [
      { id: `m-${id}`, role: 'assistant', content: [{ type: 'text', text: answer }] },
    ] } }));
  }
  await page.route('**/api/v1/bib/**', route => route.fulfill({ json: { collections: [], articles: [], total: 0 } }));

  await page.goto(origin);
  await page.getByPlaceholder('输入消息；Ctrl+R 搜索 prompt 历史').waitFor({ state: 'visible' });
  await page.getByText('alpha answer').waitFor({ state: 'visible' });

  const search = page.getByRole('button', { name: 'Search agents' });
  assert.equal(await search.getAttribute('aria-expanded'), 'false', 'picker starts closed');

  // Open: the tab strip is replaced by the picker input + agent list.
  await search.click();
  const pickerInput = page.locator('[data-slot="command-input"]');
  await pickerInput.waitFor({ state: 'visible' });
  assert.equal(await search.getAttribute('aria-expanded'), 'true', 'search button reports expanded');
  assert.equal(await page.getByRole('button', { name: 'alpha', exact: true }).count(), 0, 'tab strip leaves the bar while picking');
  const items = page.locator('[data-slot="command-item"]');
  assert.equal(await items.count(), 3, 'one item per agent');

  // The active agent carries a check marker.
  assert.equal(await items.filter({ hasText: 'alpha' }).getAttribute('data-checked'), 'true', 'active agent is marked');

  // Filtering narrows the list; Enter switches to the match and closes.
  await pickerInput.fill('bet');
  assert.equal(await items.count(), 1, 'filter narrows to beta');
  await pickerInput.press('Enter');
  await page.getByText('beta answer').waitFor({ state: 'visible' });
  await pickerInput.waitFor({ state: 'detached' });
  assert.equal(await page.getByRole('button', { name: 'beta', exact: true }).count(), 1, 'tab strip returns after picking');

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

  assert.deepEqual(errors, []);
  console.log('agent-tab-picker: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
}
