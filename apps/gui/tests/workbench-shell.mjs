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
  const errors = []; let spawnedAgent;
  page.on('pageerror', e => { errors.push(e.message); console.error('PAGE ERROR', e.stack); });

  await page.route(`${origin}/api/session`, r => r.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '');
    if (path === '/events') return;
    if (path === '/state') return route.fulfill({ json: { last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: { '/root/alpha': { id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 } }, display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false } } });
    if (path.endsWith('/history')) return route.fulfill({ json: { messages: [] } });
    // Loose bib/wiki/system fixtures: the shell test cares about chrome, not content.
    if (path.startsWith('/bib/')) return route.fulfill({ json: { collections: [], articles: [], sources: [], rules: [], items: [], snapshots: [], next: null, next_cursor: null, scheduler: {}, total: 0 } });
    if (path === '/wiki/graph') throw new Error('Local graph must not request the global graph');
    if (path === '/wiki/entries/w1/links') return route.fulfill({ json: { entry: { id: 'w1', title: '词条甲' }, out_links: [{ id: 'w2', title: '词条乙' }], in_links: [{ id: 'w3', title: '词条丙' }] } });
    if (path === '/wiki/entries/w2/links') return route.fulfill({ json: { entry: { id: 'w2', title: '词条乙' }, out_links: [{ id: 'w4', title: '词条丁' }], in_links: ['w1'] } });
    if (path === '/wiki/entries/w3/links') return route.fulfill({ json: { entry: { id: 'w3', title: '词条丙' }, out_links: ['w1'], in_links: [{ id: 'w4', title: '词条丁' }] } });
    if (path === '/wiki/entries/w4/links') return route.fulfill({ json: { entry: { id: 'w4', title: '词条丁' }, out_links: ['w1', 'w3', { id: 'w5', title: '词条戊' }], in_links: ['w2'] } });
    if (path === '/wiki/search') return route.fulfill({ json: { results: [{ entry_id: 'w1', score: 1 }] } });
    if (path === '/wiki/entries' || path.startsWith('/wiki/entries?')) return route.fulfill({ json: { entries: [{ id: 'w1', title: '词条甲', alias: [], tags: [], ctime: 1, mtime: 1 }] } });
    if (path === '/wiki/entries/w2') return route.fulfill({ json: { id: 'w2', title: '词条乙', alias: [], tags: [], content: '# 词条乙', ctime: 1, mtime: 1 } });
    if (path === '/wiki/entries/w3') return route.fulfill({ json: { id: 'w3', title: '词条丙', alias: [], tags: [], content: '# 词条丙', ctime: 1, mtime: 1 } });
    if (path === '/wiki/entries/w1') return route.fulfill({ json: { id: 'w1', title: '词条甲', alias: [], tags: [], content: '# 词条甲\n\n正文。', ctime: 1, mtime: 1 } });
    if (path === '/model-config') return route.fulfill({ json: { models: [], providers: [] } });
    if (path === '/settings') return route.fulfill({ json: {} });
    if (path === '/profiles') return route.fulfill({ json: ['researcher', 'developer', 'browser', 'curator'] });
    if (path === '/gateway/status') return route.fulfill({ json: { running: true } });
    if (path === '/storage/agents') return route.fulfill({ json: [{ id: 'a2', name: '/root/beta', config_json: { kind: 'researcher' }, created_at: 1, last_active: 1 }] });
    if (path === '/agents' && route.request().method() === 'POST') { spawnedAgent = route.request().postDataJSON(); return route.fulfill({ json: { path: '/root/beta' } }); }
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

  // Agent sidebar exposes list/detail tabs and the existing create/resume flows.
  await rail.getByRole('button', { name: 'Agent', exact: true }).click();
  const agentSidebar = page.getByRole('group', { name: 'Agent 工作区' });
  const agentListTab = agentSidebar.getByRole('tab', { name: 'Agent 列表' });
  const agentDetailTab = agentSidebar.getByRole('tab', { name: 'Agent 详情' });
  await agentListTab.waitFor();
  await agentSidebar.getByRole('button', { name: '打开 Agent /root/alpha' }).waitFor();
  await agentDetailTab.click();
  await agentSidebar.getByText('Overview', { exact: true }).waitFor();
  await agentDetailTab.focus();
  await page.keyboard.press('ArrowLeft');
  assert.equal(await agentListTab.getAttribute('aria-selected'), 'true');
  await agentSidebar.getByRole('textbox', { name: '搜索 Agent' }).fill('missing');
  await agentSidebar.getByText('没有匹配的 Agent。').waitFor();
  await agentSidebar.getByRole('textbox', { name: '搜索 Agent' }).fill('');
  await agentSidebar.getByRole('button', { name: '创建 Agent' }).click();
  await page.getByRole('dialog').getByText('New agent', { exact: true }).waitFor();
  await page.keyboard.press('Escape');
  await agentSidebar.getByRole('button', { name: '恢复 Agent' }).click();
  await page.getByRole('dialog').getByText('Resume persisted agent').waitFor();
  await page.keyboard.press('Escape');
  await agentSidebar.getByRole('button', { name: '创建 Agent' }).click();
  const spawnDialog = page.getByRole('dialog');
  await spawnDialog.getByPlaceholder('agent name').fill('beta');
  await spawnDialog.getByRole('combobox', { name: 'Agent profile' }).selectOption('developer');
  await spawnDialog.getByRole('button', { name: 'Spawn' }).click();
  await agentSidebar.getByRole('button', { name: '打开 Agent /root/beta' }).waitFor();
  assert.equal(spawnedAgent.profile, 'developer');
  await page.locator('[data-editor-group] [role="tab"][title="/root/beta"]').getByRole('button', { name: '关闭 beta' }).click();
  await agentSidebar.getByRole('button', { name: '打开 Agent /root/alpha' }).click();
  await agentTab.waitFor();

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

  // Nested horizontal and vertical groups survive reload and can be rearranged.
  await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    for (let i = 0; i < 3; i++) store.openEditor({ kind: 'config', params: { gridTest: i } });
  });
  const mainGroup = page.locator('[data-editor-group="main"]');
  const mainZoneGrid = mainGroup.locator('[data-editor-zone]');
  let gridBox = await mainZoneGrid.boundingBox();
  await mainGroup.getByRole('tab', { name: '设置' }).last().dragTo(mainZoneGrid, { targetPosition: { x: gridBox.width - 5, y: gridBox.height / 2 } });
  const secondGroup = page.locator('[data-editor-group="secondary"]');
  await secondGroup.waitFor();
  const secondZone = secondGroup.locator('[data-editor-zone]');
  gridBox = await secondZone.boundingBox();
  await mainGroup.getByRole('tab', { name: '设置' }).last().dragTo(secondZone, { targetPosition: { x: gridBox.width / 2, y: gridBox.height - 5 } });
  assert.equal(await page.locator('[data-editor-group]').count(), 3, 'nested drop creates a third group');
  assert.equal(await page.getByRole('separator', { name: '调整分屏高度' }).count(), 1);
  const extraGroup = page.locator('[data-editor-group^="group-"]');
  gridBox = await mainZoneGrid.boundingBox();
  await extraGroup.getByRole('button', { name: '拖动编辑器组' }).dragTo(mainZoneGrid, { targetPosition: { x: gridBox.width / 2, y: 5 } });
  assert.equal(await page.locator('[data-editor-group]').count(), 3, 'moving a group preserves all groups');
  await page.reload();
  await page.waitForFunction(() => document.querySelectorAll('[data-editor-group]').length === 3);
  const layout = await page.evaluate(() => JSON.parse(localStorage.getItem('autonomics.workbench.v2')).editorLayout);
  assert.ok(layout.first && layout.second, 'nested layout is persisted');
  await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    for (const [, group] of store.allGroups(store.getWorkbenchState().groups)) for (const editor of group.editors) if (editor.params.gridTest !== undefined) store.closeEditor(editor.id);
  });
  await page.waitForFunction(() => document.querySelectorAll('[data-editor-group]').length === 1);

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

  // A local graph needs an open wiki entry and never loads the global graph.
  await wikiSidebar.getByRole('button', { name: '打开知识图谱' }).click();
  const graph = page.getByRole('region', { name: '知识关系图谱' });
  await graph.getByText('请先打开知识库词条').waitFor();
  await page.getByRole('tab', { name: '知识图谱' }).getByRole('button', { name: '关闭 知识图谱' }).click();

  await page.getByRole('button', { name: /词条甲/ }).first().click();
  const wikiEntryTab = page.getByRole('tab', { name: '词条甲' });
  await wikiEntryTab.waitFor();
  const wikiSection = page.locator('section[aria-label="知识库工作区"]');
  await wikiSection.getByRole('heading', { name: '词条甲' }).first().waitFor();
  await wikiSidebar.getByRole('button', { name: '打开知识图谱' }).click();
  await graph.getByText('中心：词条甲 · 3 个词条 · 2 条链接').waitFor();
  assert.equal(await graph.locator('.wiki-graph-node').count(), 3);
  assert.equal(await graph.locator('.wiki-graph-links line').count(), 2);
  const graphLabel = graph.locator('[data-node-id="w2"] text');
  assert.equal(await graphLabel.getAttribute('text-anchor'), 'middle');
  assert.ok(Number(await graphLabel.getAttribute('y')) > 0, 'labels should appear below nodes');
  await graph.getByRole('checkbox', { name: '显示所有节点标签' }).uncheck();
  assert.equal(await graphLabel.getAttribute('visibility'), 'hidden');
  await graph.getByRole('checkbox', { name: '显示所有节点标签' }).check();
  const graphCanvas = graph.locator('svg[role="group"]');
  await graphCanvas.hover({ position: { x: 10, y: 10 } });
  await page.mouse.wheel(0, 1800);
  await page.waitForFunction(el => Number(el.getAttribute('opacity')) === 0, await graphLabel.elementHandle());
  assert.equal(await graphLabel.getAttribute('visibility'), 'hidden');
  await page.mouse.wheel(0, -1800);
  await page.waitForFunction(el => Number(el.getAttribute('opacity')) > 0, await graphLabel.elementHandle());
  await graph.getByRole('checkbox', { name: '显示所有节点标签' }).uncheck();
  await graph.getByRole('checkbox', { name: '显示入链' }).uncheck();
  await graph.getByText('中心：词条甲 · 2 个词条 · 1 条链接').waitFor();
  assert.equal(await graph.locator('[data-node-id="w3"]').count(), 0);
  await graph.getByRole('checkbox', { name: '显示入链' }).check();
  await graph.getByText('中心：词条甲 · 3 个词条 · 2 条链接').waitFor();
  await graph.getByRole('button', { name: '图谱设置' }).click();
  await graph.getByRole('button', { name: '图谱设置' }).click();
  assert.equal(await graph.locator('[data-node-id="w2"] text').getAttribute('visibility'), 'hidden');
  const repulsion = graph.getByRole('slider', { name: '节点排斥力' });
  const distance = graph.getByRole('slider', { name: '链接距离' });
  await repulsion.fill('220');
  await distance.fill('105');
  assert.equal(await repulsion.inputValue(), '220');
  assert.equal(await distance.inputValue(), '105');
  assert.equal(await graph.locator('.wiki-graph-node').count(), 3, 'adjusting forces preserves the graph');
  const graphDepth = graph.getByRole('combobox', { name: '图谱关系层级' });
  assert.equal(await graphDepth.inputValue(), '1');
  await graphDepth.selectOption('2');
  await graph.getByText('中心：词条甲 · 4 个词条 · 4 条链接').waitFor();
  assert.equal(await graph.locator('[data-node-id="w4"]').count(), 1, 'cycles and convergent paths should not duplicate nodes');
  await graphDepth.selectOption('3');
  await graph.getByText('中心：词条甲 · 5 个词条 · 6 条链接').waitFor();
  await graphDepth.selectOption('1');
  await graph.getByText('中心：词条甲 · 3 个词条 · 2 条链接').waitFor();
  await graph.getByRole('textbox', { name: '搜索图谱词条' }).fill('词条甲');
  assert.equal(await graph.locator('[data-node-id="w2"]').getAttribute('opacity'), '0.15');
  await graph.getByRole('button', { name: '放大图谱' }).click();
  await graph.locator('[data-node-id="w2"]').focus();
  await page.keyboard.press('Enter');
  await page.getByRole('tab', { name: '词条乙' }).waitFor();
  await page.getByRole('tab', { name: '知识图谱' }).click();
  await graph.getByText('中心：词条乙 · 3 个词条 · 2 条链接').waitFor();
  await graph.getByRole('button', { name: '刷新图谱' }).click();
  await graph.getByText('中心：词条乙 · 3 个词条 · 2 条链接').waitFor();
  await page.getByRole('button', { name: '拆分编辑器' }).click();
  await page.getByRole('tablist', { name: '编辑器标签', exact: true }).getByRole('tab', { name: '词条甲' }).click();
  await graph.getByText('中心：词条甲 · 3 个词条 · 2 条链接').waitFor();
  await page.getByRole('tablist', { name: '编辑器标签', exact: true }).getByRole('tab', { name: '词条乙' }).click();
  await graph.getByText('中心：词条乙 · 3 个词条 · 2 条链接').waitFor();
  await page.getByRole('button', { name: '关闭分屏' }).click();
  await page.getByRole('tab', { name: '知识图谱' }).getByRole('button', { name: '关闭 知识图谱' }).click();
  await page.getByRole('tab', { name: '词条乙' }).getByRole('button', { name: '关闭 词条乙' }).click();
  await wikiEntryTab.getByRole('button', { name: '关闭 词条甲' }).click();

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

  // Keyboard focus across split groups must update the shared editor
  // context, while focusing a workbench view must retain that target.
  const focusFixture = await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    const context = await import('/src/app/workbench/active-editor-context.ts');
    const before = context.activeEditorContext(store.getWorkbenchState());
    store.splitEditor();
    return { previousGroup: before.groupId };
  });
  const otherGroup = page.locator(`[data-editor-group]:not([data-editor-group="${focusFixture.previousGroup}"])`).last();
  await otherGroup.waitFor();
  // Set focus without a pointer event, as Tab navigation does.
  await page.locator(`[data-editor-group="${focusFixture.previousGroup}"] [role="tab"]`).first().focus();
  const focusedContext = await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    const { activeEditorContext } = await import('/src/app/workbench/active-editor-context.ts');
    return activeEditorContext(store.getWorkbenchState()).groupId;
  });
  assert.equal(focusedContext, focusFixture.previousGroup, 'keyboard focus activates editor group');
  await page.keyboard.press('F6');
  assert.equal(await page.evaluate(id => Boolean(document.activeElement?.closest(`[data-editor-group="${id}"]`)), focusFixture.previousGroup), false, 'F6 moves focus to another workbench region');
  await page.keyboard.press('Shift+F6');
  assert.equal(await page.evaluate(id => Boolean(document.activeElement?.closest(`[data-editor-group="${id}"]`)), focusFixture.previousGroup), true, 'Shift+F6 returns to the previous region');
  await rail.getByRole('button', { name: 'CONFIG' }).focus();
  assert.equal(await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    return store.getWorkbenchState().groups.active;
  }), focusedContext, 'workbench view focus retains editor target');
  await rail.getByRole('button', { name: 'Wiki', exact: true }).click();
  await sidebar.getByRole('button', { name: '视图位置', exact: true }).click();
  await page.getByRole('menuitemradio', { name: '底部面板', exact: true }).click();
  const bottom = page.getByRole('region', { name: '底部面板' });
  await bottom.getByRole('tab', { name: '知识库', exact: true }).waitFor();
  assert.equal(await bottom.getByRole('tab', { name: '知识库', exact: true }).getAttribute('aria-selected'), 'true');
  await page.reload();
  await bottom.getByRole('tab', { name: '知识库', exact: true }).waitFor();
  await bottom.getByRole('button', { name: '视图位置', exact: true }).click();
  await page.getByRole('menuitemradio', { name: '主侧边栏', exact: true }).click();
  assert.equal(await bottom.getByRole('tab', { name: '知识库', exact: true }).count(), 0);
  await sidebar.getByRole('button', { name: '视图位置', exact: true }).click();
  await page.getByRole('menuitemradio', { name: '辅助侧边栏', exact: true }).click();
  const secondary = page.getByRole('complementary', { name: '辅助侧边栏', exact: true });
  await secondary.getByRole('region', { name: '知识库', exact: true }).waitFor();
  await secondary.getByRole('button', { name: '隐藏辅助侧边栏', exact: true }).click();
  await secondary.waitFor({ state: 'hidden' });
  await rail.getByRole('button', { name: '搜索或输入命令' }).click();
  await page.getByRole('dialog').getByText('Toggle secondary side bar', { exact: true }).click();
  await secondary.waitFor({ state: 'visible' });
  const secondaryResize = secondary.getByRole('separator', { name: '调整辅助侧边栏宽度' });
  const initialSecondaryWidth = (await secondary.boundingBox()).width;
  await secondaryResize.focus();
  await page.keyboard.press('ArrowLeft');
  assert.equal((await secondary.boundingBox()).width, initialSecondaryWidth + 10);
  await page.reload();
  await secondary.getByRole('region', { name: '知识库', exact: true }).waitFor();
  assert.equal((await secondary.boundingBox()).width, initialSecondaryWidth + 10, 'secondary sidebar width persists');
  await secondary.getByRole('button', { name: '视图位置', exact: true }).click();
  await page.getByRole('menuitemradio', { name: '主侧边栏', exact: true }).click();
  await secondary.waitFor({ state: 'detached' });
  const guardedClose = await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    const registry = await import('/src/app/workbench/editor-registry.ts');
    let veto = true;
    registry.registerEditorKind({ kind: 'guard-fixture', title: () => 'Unsaved fixture', body: () => null, mount: 'keep-alive', confirmLeave: () => veto });
    const id = store.openEditor({ kind: 'guard-fixture', params: {} });
    const rejected = !store.closeEditor(id);
    const kept = store.allGroups(store.getWorkbenchState().groups).some(([, group]) => group.editors.some(editor => editor.id === id));
    veto = false;
    const accepted = store.closeEditor(id);
    return { rejected, kept, accepted };
  });
  assert.deepEqual(guardedClose, { rejected: true, kept: true, accepted: true }, 'all programmatic closes enforce the editor guard');
  await rail.getByRole('button', { name: 'Agent', exact: true }).click();
  await sidebar.getByRole('button', { name: '视图位置', exact: true }).click();
  await page.getByRole('menuitemradio', { name: '底部面板', exact: true }).click();
  await bottom.getByRole('button', { name: '创建 Agent', exact: true }).click();
  await page.getByRole('dialog').waitFor();
  await page.keyboard.press('Escape');
  await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    store.setPanelTab('events');
  });
  assert.equal(await bottom.getByRole('tab', { name: '事件流', exact: true }).getAttribute('aria-selected'), 'true', 'legacy panel commands select their registered view');
  await bottom.getByRole('tab', { name: 'Agent', exact: true }).click();
  await bottom.getByRole('button', { name: '视图位置', exact: true }).click();
  await page.getByRole('menuitemradio', { name: '辅助侧边栏', exact: true }).click();
  await secondary.getByRole('button', { name: '创建 Agent', exact: true }).click();
  await page.getByRole('dialog').waitFor();
  await page.keyboard.press('Escape');
  await rail.getByRole('button', { name: '搜索或输入命令' }).click();
  await page.getByRole('dialog').getByText('Reset view locations', { exact: true }).click();
  await secondary.waitFor({ state: 'detached' });
  await sidebar.getByRole('button', { name: '创建 Agent', exact: true }).waitFor();
  assert.equal(await page.evaluate(() => localStorage.getItem('autonomics.workbench.views.v1')), null, 'reset clears persisted view positions');
  await page.evaluate(async () => {
    const registry = await import('/src/app/workbench/view-registry.ts');
    window.disposeDynamicView = registry.registerWorkbenchView({ id: 'dynamic-view-fixture', title: '动态辅助视图', defaultLocation: 'secondarySidebar', body: () => null });
  });
  await secondary.getByRole('region', { name: '动态辅助视图', exact: true }).waitFor();
  await rail.getByRole('button', { name: '动态辅助视图', exact: true }).waitFor();
  await secondary.getByRole('button', { name: '视图位置', exact: true }).click();
  await page.getByRole('menuitemradio', { name: '主侧边栏', exact: true }).click();
  await sidebar.getByText('动态辅助视图', { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('autonomics.workbench.v2')).sidebar.container), 'dynamic-view-fixture', 'custom view becomes the active primary container');
  await page.evaluate(() => window.disposeDynamicView());
  await rail.getByRole('button', { name: '动态辅助视图', exact: true }).waitFor({ state: 'detached' });
  await secondary.waitFor({ state: 'detached' });
  const safeReplacement = await page.evaluate(async () => {
    const registry = await import('/src/app/workbench/view-registry.ts');
    const disposeOld = registry.registerWorkbenchView({ id: 'replace-fixture', title: 'Old', defaultLocation: 'panel', body: () => null });
    const disposeNew = registry.registerWorkbenchView({ id: 'replace-fixture', title: 'New', defaultLocation: 'panel', body: () => null });
    disposeOld();
    const kept = registry.workbenchView('replace-fixture')?.title === 'New';
    disposeNew();
    return kept && !registry.workbenchView('replace-fixture');
  });
  assert.equal(safeReplacement, true, 'disposing an old contribution preserves its replacement');
  await rail.getByRole('button', { name: 'Agent', exact: true }).click();
  await sidebar.getByRole('textbox', { name: '搜索 Agent', exact: true }).fill('retained view state');
  await sidebar.getByLabel('拖动视图', { exact: true }).dragTo(bottom);
  await bottom.getByRole('tab', { name: 'Agent', exact: true }).waitFor();
  assert.equal(await bottom.getByRole('textbox', { name: '搜索 Agent', exact: true }).inputValue(), 'retained view state', 'moving a view preserves component state');
  await bottom.getByRole('tab', { name: 'Agent', exact: true }).dragTo(sidebar);
  await sidebar.getByRole('button', { name: '创建 Agent', exact: true }).waitFor();
  assert.equal(await sidebar.getByRole('textbox', { name: '搜索 Agent', exact: true }).inputValue(), 'retained view state', 'moving back preserves component state');
  await page.evaluate(async () => {
    const layout = await import('/src/app/workbench/view-layout.ts');
    const store = await import('/src/app/workbench/workbench-store.ts');
    layout.moveWorkbenchView('notices', 'primarySidebar');
    store.bumpUnreadNotices(3);
  });
  await page.waitForFunction(() => JSON.parse(localStorage.getItem('autonomics.workbench.v2')).sidebar.container === 'notices');
  assert.equal(await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    return store.getWorkbenchState().panel.unreadNotices;
  }), 0, 'visible notices clear unread state after moving to the sidebar');
  await sidebar.locator('[data-view-host="agents"]').waitFor({ state: 'detached' });
  assert.equal(await sidebar.locator('[data-view-host]').count(), 1, 'switching views detaches the previous host without stacking content');
  await page.screenshot({ path: '/tmp/autonomics-workbench-shell.png' });
  assert.deepEqual(errors, []);
  console.log('workbench-shell: all assertions passed');
} catch (error) { console.error(error); throw error; }
finally { await browser?.close(); server.kill(); }
