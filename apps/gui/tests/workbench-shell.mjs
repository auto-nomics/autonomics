// Workbench shell test: activity bar, editor tabs (open/close/activate),
// split, bottom panel, aux toggle, status bar, command palette and the v2
// layout persistence. Run with node tests/workbench-shell.mjs.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1439';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1439', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser;
server.stdout.on('data', chunk => output += chunk); server.stderr.on('data', chunk => output += chunk);
try {
  for (let n = 0; !output.includes(origin); n++) { if (server.exitCode !== null || n > 120) throw new Error(output); await new Promise(r => setTimeout(r, 50)); }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } }); page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', e => { errors.push(e.message); console.error('PAGE ERROR', e.stack); });

  await page.route(`${origin}/api/session`, r => r.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '');
    if (path === '/events') return;
    if (path === '/state') return route.fulfill({ json: { last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: { '/root/alpha': { id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 } }, display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false } } });
    if (path.endsWith('/history')) return route.fulfill({ json: { messages: [] } });
    // Loose bib/wiki/system fixtures: the shell test cares about chrome, not content.
    if (path.startsWith('/bib/')) return route.fulfill({ json: { collections: [], articles: [], total: 0 } });
    if (path === '/wiki/entries' || path.startsWith('/wiki/entries?')) return route.fulfill({ json: { entries: [{ id: 'w1', title: '词条甲', alias: [], tags: [], ctime: 1, mtime: 1 }] } });
    if (path === '/wiki/entries/w1') return route.fulfill({ json: { id: 'w1', title: '词条甲', alias: [], tags: [], content: '# 词条甲\n\n正文。', ctime: 1, mtime: 1 } });
    if (path === '/model-config') return route.fulfill({ json: { models: [], providers: [] } });
    if (path === '/settings') return route.fulfill({ json: {} });
    if (path === '/profiles') return route.fulfill({ json: [] });
    if (path === '/gateway/status') return route.fulfill({ json: { running: true } });
    if (path === '/storage/agents') return route.fulfill({ json: [{ id: 'a2', name: '/root/beta', config_json: { kind: 'researcher' }, created_at: 1, last_active: 1 }] });
    if (path === '/agents' && route.request().method() === 'POST') return route.fulfill({ json: { path: '/root/beta' } });
    return route.fulfill({ status: 404, json: { error: 'Not mocked: ' + path } });
  });
  // Registered last so LIFO precedence holds the SSE stream open (a
  // fall-through to the network would 404 and flip the UI to reconnecting).
  await page.route('**/api/v1/events', () => new Promise(() => {}));
  await page.goto(origin);

  // ── Shell chrome ──────────────────────────────────────────────────────
  await page.locator('main[data-workbench]').waitFor();
  const rail = page.getByRole('navigation', { name: '工作台活动栏' });
  await rail.getByRole('button', { name: 'Bibliography' }).waitFor();
  await rail.getByRole('button', { name: 'Wiki' }).waitFor();
  await rail.getByRole('button', { name: 'Agent', exact: true }).waitFor();
  await rail.getByRole('button', { name: 'CONFIG' }).waitFor();
  await page.locator('aside[data-slot=workbench-sidebar]').waitFor();
  // The hydrate-seeded agent opens as a workbench editor tab; the chat
  // portals into it (no auxiliary bar anymore).
  const agentTab = page.locator('[data-editor-group] [role="tab"][title="/root/alpha"]');
  await agentTab.waitFor();
  await page.getByPlaceholder('输入消息…').waitFor();
  await page.getByText('Gateway 已连接').waitFor();

  // ── Activity bar opens container editors ──────────────────────────────
  await rail.getByRole('button', { name: 'Bibliography' }).click();
  const bibTab = page.getByRole('tab', { name: '文献与证据' });
  await bibTab.waitFor();
  await page.locator('section[aria-label="文献工作区"]').waitFor();
  await page.getByRole('button', { name: 'Bibliography library' }).waitFor();

  await rail.getByRole('button', { name: 'CONFIG' }).click();
  const configTab = page.getByRole('tab', { name: '设置' });
  await configTab.waitFor();
  await assert.doesNotReject(() => bibTab.getAttribute('aria-selected').then(v => assert.equal(v, 'false', 'bibliography tab must deactivate when config activates')));

  // ── Command palette (Ctrl+P) ─────────────────────────────────────────
  await page.keyboard.press('Control+p');
  const palette = page.getByRole('dialog');
  await palette.getByText('Command palette').waitFor();
  await palette.getByPlaceholder('Search commands').fill('knowledge');
  await palette.getByRole('button').first().click();
  // Opening the knowledge base reveals the wiki container (browse view).
  await page.locator('aside[data-slot=workbench-sidebar]').getByRole('group', { name: '知识库工作区' }).waitFor();
  // Ctrl+K opens the wiki search over any editor.
  await page.keyboard.press('Control+k');
  const searchDialog = page.locator('[data-slot="dialog-content"]:visible');
  await searchDialog.waitFor();
  await page.keyboard.press('Escape');
  await searchDialog.waitFor({ state: 'detached' });

  // ── Wiki entries are editor tabs ──────────────────────────────────────
  await page.getByRole('button', { name: /词条甲/ }).first().click();
  const wikiEntryTab = page.getByRole('tab', { name: '词条甲' });
  await wikiEntryTab.waitFor();
  const wikiSection = page.locator('section[aria-label="知识库工作区"]');
  await wikiSection.getByRole('heading', { name: '词条甲' }).first().waitFor();
  await wikiEntryTab.getByRole('button', { name: '关闭 词条甲' }).click();
  await wikiEntryTab.waitFor({ state: 'detached' });

  // ── Split editor ─────────────────────────────────────────────────────
  await bibTab.click(); // bibliography active again
  await page.getByRole('button', { name: '拆分编辑器' }).click();
  const splitTab = page.getByRole('tablist', { name: '分屏编辑器标签' }).getByRole('tab', { name: '文献与证据' });
  await splitTab.waitFor();
  await page.getByRole('button', { name: '关闭分屏' }).click();
  await page.getByRole('tablist', { name: '分屏编辑器标签' }).waitFor({ state: 'detached' });
  await bibTab.click();
  await page.locator('section[aria-label="文献工作区"]').waitFor();

  // ── Bottom panel (Ctrl+J) ────────────────────────────────────────────
  await page.keyboard.press('Control+j');
  const panel = page.getByRole('region', { name: '底部面板' });
  await panel.getByRole('tab', { name: '事件流' }).waitFor();
  await panel.getByRole('tab', { name: /通知/ }).click();
  await panel.getByText('暂无通知').waitFor();
  await page.keyboard.press('Control+j');
  await panel.waitFor({ state: 'hidden' });

  // ── Agent tab close → chat hides; resume brings it back ───────────────
  const composer = page.getByPlaceholder('输入消息…');
  await agentTab.getByRole('button', { name: '关闭 alpha' }).click();
  await composer.waitFor({ state: 'hidden' });
  // With no agent tab left the workspace picker is detached — the resume
  // dialog (palette) is the re-entry path.
  await page.keyboard.press('Control+Shift+p');
  const resumePalette = page.locator('[data-slot="dialog-content"]:visible');
  await resumePalette.getByPlaceholder('Search commands').fill('resume');
  await resumePalette.getByRole('button').first().click();
  const resumeDialog = page.locator('[data-slot="dialog-content"]:visible');
  await resumeDialog.getByRole('button', { name: 'Resume' }).first().click();
  await composer.waitFor({ state: 'visible' });
  await page.locator('[data-editor-group] [role="tab"][title="/root/beta"]').waitFor();

  // ── Layout persistence across a reload ───────────────────────────────
  const persisted = await page.evaluate(() => {
    try { return JSON.parse(localStorage.getItem('autonomics.workbench.v2') ?? 'null'); } catch { return null; }
  });
  assert.ok(persisted && persisted.version === 2, 'v2 layout key must exist');
  assert.ok(persisted.groups.main.editors.some(e => e.kind === 'bibliography'), 'bibliography editor must persist');
  await page.reload();
  // All pre-reload tabs restore (keep-alive bodies remount from persisted
  // state); the active tab is the last activated one (the resumed agent).
  await page.getByRole('tab', { name: '文献与证据' }).waitFor();
  await page.getByRole('tab', { name: 'beta' }).waitFor();
  await composer.waitFor({ state: 'visible' });

  // Keep-alive: editors opened before the reload remount from persisted
  // state, the config tab should still be in the strip.
  await page.getByRole('tab', { name: '设置' }).waitFor();

  await page.screenshot({ path: '/tmp/autonomics-workbench-shell.png' });
  assert.deepEqual(errors, []);
  console.log('workbench-shell: all assertions passed');
} catch (error) { console.error(error); throw error; }
finally { await browser?.close(); server.kill(); }
