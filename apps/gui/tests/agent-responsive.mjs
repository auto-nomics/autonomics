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
      contexts: {
        '/root/alpha': { id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 },
        '/root/beta': { id: 's2', title: 'Beta main', created_at: 1, last_active: 3 },
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

  // ── Desktop: agent chat is an editor tab in the unified strip ─────────
  {
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('dialog', dialog => dialog.accept());
    await mockApi(page);
    // Pre-open both agents as view tabs (the persisted open-tab list).
    await page.addInitScript(() => localStorage.setItem('autonomics.openTabs', JSON.stringify(['/root/alpha', '/root/beta'])));

    await page.goto(origin);
    const composer = page.getByPlaceholder('输入消息…');
    await composer.waitFor({ state: 'visible' });
    // The workspace portals into the active agent tab inside the editor area.
    const inEditor = await composer.evaluate(node => Boolean(node.closest('[data-editor-zone]')));
    assert.ok(inEditor, 'workspace renders inside the editor area (unified tabs)');
    const alphaTab = page.locator('[data-editor-group] [role="tab"][title="/root/alpha"]');
    const betaTab = page.locator('[data-editor-group] [role="tab"][title="/root/beta"]');
    await alphaTab.waitFor({ state: 'visible' });
    await betaTab.waitFor({ state: 'visible' });
    assert.equal(await alphaTab.getAttribute('aria-selected'), 'true', 'first agent tab is active');

    // Switching agent tabs swaps the conversation.
    await betaTab.click();
    await assert.doesNotReject(() => page.getByText('beta prompt').waitFor({ state: 'visible' }));

    // Closing the active tab falls back to the remaining one.
    await betaTab.getByRole('button', { name: '关闭 beta' }).click();
    await composer.waitFor({ state: 'visible' });
    await alphaTab.waitFor({ state: 'visible' });
    await page.getByText('alpha prompt').waitFor({ state: 'visible' });

    // Reload: the open-tab list (persisted minus the closed tab) restores.
    await page.reload();
    await composer.waitFor({ state: 'visible' });
    await page.locator('[data-editor-group] [role="tab"][title="/root/alpha"]').waitFor({ state: 'visible' });
    await assert.doesNotReject(() => page.getByText('alpha prompt').waitFor({ state: 'visible' }));

    assert.deepEqual(errors, []);
    await page.close();
  }

  // ── Mobile: the editor area is fullscreen; the sidebar is a drawer ──
  {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('dialog', dialog => dialog.accept());
    await mockApi(page);
    await page.addInitScript(() => localStorage.setItem('autonomics.openTabs', JSON.stringify(['/root/alpha', '/root/beta'])));

    await page.goto(origin);
    const composer = page.getByPlaceholder('输入消息…');
    await composer.waitFor({ state: 'visible' });
    // No FAB/drawer anymore: the chat is a fullscreen editor tab.
    assert.equal(await page.locator('[data-drawer-shell]').count(), 0, 'no drawer shell');
    assert.equal(await page.getByRole('button', { name: '打开 Agent 副窗' }).count(), 0, 'no FAB');
    const inEditor = await composer.evaluate(node => Boolean(node.closest('[data-editor-zone]')));
    assert.ok(inEditor, 'workspace renders in the fullscreen editor area');

    // Agents switch through the editor tab strip.
    await page.getByTitle('/root/beta').click();
    await assert.doesNotReject(() => page.getByText('beta prompt').waitFor({ state: 'visible' }));

    // The sidebar drawer opens via edge swipe; drawer footer carries theme/logout.
    await page.evaluate(() => {
      const touch = (x, y) => new Touch({ identifier: 1, target: document.body, clientX: x, clientY: y });
      const fire = (type, x, y) => document.dispatchEvent(new TouchEvent(type, { touches: [touch(x, y)], bubbles: true, cancelable: true }));
      fire('touchstart', 10, 420); fire('touchmove', 180, 420);
    });
    await page.getByRole('button', { name: '退出登录' }).waitFor({ state: 'visible' });
    await page.getByRole('button', { name: '收起侧栏' }).click();

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
