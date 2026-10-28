// Run with npm run test:responsive. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1427';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1427', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
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

  const mockApi = async page => {
    await page.route(`${origin}/api/session`, route => route.fulfill({ json: {} }));
    // Hold the event stream open so hydration runs exactly once.
    await page.route('**/api/v1/events', () => new Promise(() => {}));
    await page.route('**/api/v1/state', route => route.fulfill({ json: {
      last_seq: 0,
      agents: [
        { path: '/root/alpha', agent_id: 'a1', status: 'Idle' },
        { path: '/root/beta', agent_id: 'a2', status: 'Streaming' },
      ],
      sessions: {
        '/root/alpha': [{ id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 }],
        '/root/beta': [{ id: 's2', title: 'Beta main', created_at: 1, last_active: 3 }],
      },
      display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
    } }));
    await page.route('**/api/v1/agents/a1/sessions/s1/history', route => route.fulfill({ json: { messages: [
      { id: 'm1', role: 'user', content: [{ type: 'text', text: 'alpha prompt' }] },
    ] } }));
    await page.route('**/api/v1/agents/a2/sessions/s2/history', route => route.fulfill({ json: { messages: [
      { id: 'm3', role: 'user', content: [{ type: 'text', text: 'beta prompt' }] },
    ] } }));
    await page.route('**/api/v1/bib/**', route => route.fulfill({ json: { collections: [], articles: [], total: 0 } }));
  };

  // ── Desktop: three resizable panels ──────────────────────────────────
  {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('dialog', dialog => dialog.accept());
    await mockApi(page);

    await page.goto(origin);
    const composer = page.getByPlaceholder('输入消息；Ctrl+R 搜索 prompt 历史');
    await composer.waitFor({ state: 'visible' });
    assert.equal(await page.locator('[data-group]').count(), 1, 'desktop renders one resizable group');
    assert.equal(await page.locator('[data-separator]').count(), 2, 'desktop renders two handles');
    const dockedInFloat = await composer.evaluate(node => Boolean(node.closest('.fixed')));
    assert.equal(dockedInFloat, false, 'workspace must stay docked on desktop');

    const panelWidth = () => page.locator('[data-panel]#agents').evaluate(node => node.getBoundingClientRect().width);
    const inspectorWidth = () => page.locator('[data-panel]#inspector').evaluate(node => node.getBoundingClientRect().width);
    const left0 = await panelWidth();
    const right0 = await inspectorWidth();
    assert.ok(Math.abs(left0 - 300) <= 12, `agents panel should default near 300px, got ${left0}`);
    assert.ok(Math.abs(right0 - 280) <= 12, `inspector panel should default near 280px, got ${right0}`);

    // Drag the left handle: the agents panel follows the pointer.
    const handle = page.locator('[data-separator]').first();
    const hb = await handle.boundingBox();
    await page.mouse.move(hb.x + hb.width / 2, hb.y + hb.height / 2);
    await page.mouse.down();
    await page.mouse.move(hb.x + hb.width / 2 + 80, hb.y + hb.height / 2, { steps: 6 });
    await page.mouse.up();
    const left1 = await panelWidth();
    assert.ok(Math.abs(left1 - (left0 + 80)) <= 10, `agents panel must grow by ~80px, got ${left1} from ${left0}`);
    const stored = await page.evaluate(() => localStorage.getItem('autonomics.agent.layout'));
    assert.ok(stored && JSON.parse(stored).agents > 0, 'dragged layout must persist');

    // Collapse via the tab-bar toggle and expand back to the saved size.
    await page.getByRole('button', { name: 'Toggle left sidebar' }).click();
    await page.waitForTimeout(120);
    assert.ok(await panelWidth() < 2, 'toggle must collapse the agents panel');
    await page.getByRole('button', { name: 'Toggle left sidebar' }).click();
    await page.waitForTimeout(120);
    const left2 = await panelWidth();
    assert.ok(Math.abs(left2 - left1) <= 10, `re-expand must restore the size, got ${left2} vs ${left1}`);

    // Reload: the dragged layout survives.
    await page.reload();
    await composer.waitFor({ state: 'visible' });
    const left3 = await panelWidth();
    assert.ok(Math.abs(left3 - left1) <= 10, `layout must survive a reload, got ${left3} vs ${left1}`);

    assert.deepEqual(errors, []);
    await page.close();
  }

  // ── Mobile: drawers replace the panels ───────────────────────────────
  {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('dialog', dialog => dialog.accept());
    await mockApi(page);

    await page.goto(origin);
    const composer = page.getByPlaceholder('输入消息；Ctrl+R 搜索 prompt 历史');
    await composer.waitFor({ state: 'visible' });
    assert.equal(await page.locator('[data-group]').count(), 0, 'mobile must not render the resizable group');
    assert.equal(await page.locator('[data-slot=sheet-content]').count(), 0, 'no drawer initially');

    // The left toggle opens the agents drawer; picking an agent closes it.
    await page.getByRole('button', { name: 'Toggle left sidebar' }).click();
    const sheet = page.locator('[data-slot=sheet-content]').first();
    await sheet.waitFor({ state: 'visible' });
    await sheet.getByTitle('/root/beta').click();
    await sheet.waitFor({ state: 'detached' });
    await assert.doesNotReject(() => page.getByText('beta prompt').waitFor({ state: 'visible' }));

    // The right toggle opens the inspector drawer; Escape closes it.
    await page.getByRole('button', { name: 'Toggle right sidebar' }).click();
    const inspector = page.locator('[data-slot=sheet-content]').first();
    await inspector.waitFor({ state: 'visible' });
    await assert.doesNotReject(() => inspector.getByText('Actions').waitFor({ state: 'visible' }));
    await page.keyboard.press('Escape');
    await inspector.waitFor({ state: 'detached' });

    const dockedInFloat = await composer.evaluate(node => Boolean(node.closest('.fixed')));
    assert.equal(dockedInFloat, false, 'workspace must stay docked on mobile');
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    assert.ok(overflow <= 0, `page must not scroll horizontally, overflow ${overflow}px`);

    assert.deepEqual(errors, []);
    await page.close();
  }

  console.log('agent-responsive: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
}
