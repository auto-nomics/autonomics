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
    if (path.startsWith('/bib/')) return route.fulfill({ json: { collections: [], articles: [], sources: [], rules: [], items: [], snapshots: [], next: null, next_cursor: null, scheduler: {}, total: 0 } });
    if (path === '/wiki/graph') {
      const offset = Number(new URL(route.request().url()).searchParams.get('offset') ?? 0);
      const entry = (id, title) => ({ id, title, tags: [], alias: [], ctime: 1, mtime: 1 });
      return route.fulfill({ json: offset === 0
        ? { nodes: [entry('w1', '词条甲'), entry('w2', '词条乙')], edges: [{ source: 'w1', target: 'w2' }, { source: 'w1', target: 'missing' }], next_offset: 2 }
        : { nodes: [entry('w3', '孤立词条')], edges: [], next_offset: null } });
    }
    if (path === '/wiki/search') return route.fulfill({ json: { results: [{ entry_id: 'w1', score: 1 }] } });
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
  assert.equal(await page.locator('main[data-workbench] > header').count(), 0);
  await rail.getByRole('button', { name: '搜索或输入命令' }).click();
  await page.getByRole('dialog').getByText('Command palette').waitFor();
  await page.keyboard.press('Escape');
  await rail.getByRole('button', { name: '折叠侧栏' }).click();
  await page.getByRole('button', { name: '展开侧栏' }).waitFor();
  await rail.getByRole('button', { name: '展开侧栏' }).click();
  await rail.getByRole('button', { name: /色彩主题/ }).waitFor();
  await rail.getByRole('button', { name: '退出登录' }).waitFor();
  await page.locator('aside[data-slot=workbench-sidebar]').waitFor();
  // The desktop sidebar can be resized and restores its width.
  const sidebar = page.locator('aside[data-slot=workbench-sidebar]');
  const resizeSidebar = page.getByRole('separator', { name: '调整侧栏宽度' });
  const beforeWidth = (await sidebar.boundingBox()).width;
  const handleBox = await resizeSidebar.boundingBox();
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(handleBox.x + handleBox.width / 2 + 80, handleBox.y + handleBox.height / 2, { steps: 5 });
  await page.mouse.up();
  assert.ok((await sidebar.boundingBox()).width > beforeWidth + 60, 'sidebar should resize by dragging');
  await page.reload();
  await resizeSidebar.waitFor();
  assert.ok((await sidebar.boundingBox()).width > beforeWidth + 60, 'sidebar width should persist');
  await resizeSidebar.focus();
  await page.keyboard.press('ArrowLeft');
  assert.equal((await sidebar.boundingBox()).width, beforeWidth + 70, 'keyboard should resize sidebar');

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
  const librarySidebar = page.locator('aside[data-slot=workbench-sidebar]');
  await librarySidebar.getByRole('tree', { name: 'Collections' }).waitFor();
  await librarySidebar.getByRole('button', { name: 'Unfiled' }).click();
  await page.getByRole('button', { name: /Unfiled/ }).first().waitFor();
  assert.equal(await page.locator('section[aria-label="文献工作区"]').getByRole('tree', { name: 'Collections' }).count(), 0);
  await librarySidebar.getByRole('button', { name: /All Articles/ }).click();

  await rail.getByRole('button', { name: 'CONFIG' }).click();
  const configTab = page.getByRole('tab', { name: '设置' });
  await configTab.waitFor();
  await assert.doesNotReject(() => bibTab.getAttribute('aria-selected').then(v => assert.equal(v, 'false', 'bibliography tab must deactivate when config activates')));

  // Move tabs within and between groups, then verify persistence.
  const mainTabs = page.getByRole('tablist', { name: '编辑器标签', exact: true });
  await configTab.dragTo(bibTab, { targetPosition: { x: 2, y: 10 } });
  assert.deepEqual(await mainTabs.getByRole('tab').allTextContents(), ['alpha', '设置', '文献与证据']);
  const zone = page.locator('[data-editor-zone="main"]');
  const zoneBox = await zone.boundingBox();
  await configTab.dragTo(zone, { targetPosition: { x: zoneBox.width - 10, y: zoneBox.height / 2 } });
  const secondaryTabs = page.getByRole('tablist', { name: '分屏编辑器标签' });
  await secondaryTabs.getByRole('tab', { name: '设置' }).waitFor();
  await secondaryTabs.getByRole('tab', { name: '设置' }).dragTo(bibTab, { targetPosition: { x: 2, y: 10 } });
  await secondaryTabs.waitFor({ state: 'detached' });
  assert.deepEqual(await mainTabs.getByRole('tab').allTextContents(), ['alpha', '设置', '文献与证据']);
  await page.reload();
  await page.getByRole('tab', { name: '设置' }).waitFor();
  assert.deepEqual(await mainTabs.getByRole('tab').allTextContents(), ['alpha', '设置', '文献与证据']);

  // Research tools open as independent tabs from the activity bar.
  const bibSidebar = page.locator('aside[data-slot=workbench-sidebar]');
  for (const [name, title] of [['Discover', '发现'], ['Snapshots', '快照'], ['Subscriptions', '订阅']]) {
    assert.equal(await bibSidebar.getByRole('button', { name, exact: true }).count(), 0);
    await rail.getByRole('button', { name, exact: true }).click();
    await page.getByRole('tab', { name: title, exact: true }).waitFor();
  }
  await rail.getByRole('button', { name: 'Discover', exact: true }).click();
  assert.equal(await page.getByRole('tab', { name: '发现', exact: true }).getAttribute('aria-selected'), 'true');
  await page.reload();
  for (const title of ['发现', '快照', '订阅']) {
    const tab = page.getByRole('tab', { name: title, exact: true });
    await tab.waitFor();
    await tab.getByRole('button', { name: `关闭 ${title}` }).click();
  }

  // Overflow stays inside the tab strip; menu and wheel reveal hidden tabs.
  await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    for (let i = 0; i < 16; i++) store.openEditor({ kind: 'config', params: { overflowTest: i } });
  });
  const tabScroll = mainTabs.locator('[data-editor-tabs-scroll]');
  await mainTabs.getByLabel('显示所有编辑器标签').waitFor();
  assert.ok(await tabScroll.evaluate(el => el.scrollWidth > el.clientWidth), 'tabs should overflow their own scroll area');
  assert.ok(await tabScroll.evaluate(el => el.scrollLeft > 0), 'newly active tab should be revealed');
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), 'tabs should not widen the window');
  await mainTabs.getByLabel('显示所有编辑器标签').click();
  await mainTabs.getByRole('menuitem', { name: 'alpha' }).click();
  assert.equal(await agentTab.getAttribute('aria-selected'), 'true');
  assert.ok(await tabScroll.evaluate(el => el.scrollLeft < 5), 'selecting a hidden tab should reveal it');
  await tabScroll.hover();
  await page.mouse.wheel(0, 500);
  await page.waitForFunction(el => el.scrollLeft > 0, await tabScroll.elementHandle());
  await page.getByRole('button', { name: '拆分编辑器' }).click();
  await page.getByRole('tablist', { name: '分屏编辑器标签' }).waitFor();
  assert.ok(await tabScroll.evaluate(el => el.scrollWidth > el.clientWidth), 'split group should keep overflow contained');
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), 'split tabs should not widen the window');
  await page.getByRole('button', { name: '关闭分屏' }).click();
  await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    for (const editor of [...store.getWorkbenchState().groups.main.editors]) if (editor.params.overflowTest !== undefined) store.closeEditor(editor.id);
  });

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

  const wikiSidebar = page.locator('aside[data-slot=workbench-sidebar]');
  const wikiSearch = wikiSidebar.getByRole('textbox', { name: '搜索知识库' });
  await wikiSearch.fill('词条');
  await wikiSidebar.getByRole('list', { name: '检索结果' }).getByRole('button', { name: '词条甲' }).waitFor();
  await wikiSidebar.getByRole('button', { name: '清除搜索' }).click();
  await wikiSidebar.getByRole('list', { name: '词条列表' }).waitFor();
  await wikiSidebar.getByRole('button', { name: '浏览词条' }).click();
  assert.equal(await wikiSearch.count(), 0);

  // ── Wiki entries are editor tabs ──────────────────────────────────────
  await page.getByRole('button', { name: /词条甲/ }).first().click();
  const wikiEntryTab = page.getByRole('tab', { name: '词条甲' });
  await wikiEntryTab.waitFor();
  const wikiSection = page.locator('section[aria-label="知识库工作区"]');
  await wikiSection.getByRole('heading', { name: '词条甲' }).first().waitFor();
  await wikiEntryTab.getByRole('button', { name: '关闭 词条甲' }).click();
  await wikiEntryTab.waitFor({ state: 'detached' });

  // Knowledge graph loads every page, renders links and opens nodes.
  await wikiSidebar.getByRole('button', { name: '打开知识图谱' }).click();
  const graph = page.getByRole('region', { name: '知识关系图谱' });
  await graph.getByText('3 个词条 · 1 条链接').waitFor();
  assert.equal(await graph.locator('.wiki-graph-node').count(), 3);
  assert.equal(await graph.locator('.wiki-graph-links line').count(), 1);
  await graph.getByRole('checkbox', { name: '显示孤立词条' }).uncheck();
  assert.equal(await graph.locator('.wiki-graph-node').count(), 2);
  await graph.getByRole('textbox', { name: '搜索图谱词条' }).fill('词条甲');
  assert.equal(await graph.locator('[data-node-id="w2"]').getAttribute('opacity'), '0.15');
  await graph.getByRole('button', { name: '放大图谱' }).click();
  await graph.locator('[data-node-id="w1"]').focus();
  await page.keyboard.press('Enter');
  await page.getByRole('tab', { name: '词条甲' }).waitFor();
  await page.getByRole('tab', { name: '知识图谱' }).click();
  await graph.getByRole('button', { name: '刷新图谱' }).click();
  await graph.getByText('2 个词条 · 1 条链接').waitFor();
  await page.getByRole('tab', { name: '知识图谱' }).getByRole('button', { name: '关闭 知识图谱' }).click();
  await page.getByRole('tab', { name: '词条甲' }).getByRole('button', { name: '关闭 词条甲' }).click();

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
