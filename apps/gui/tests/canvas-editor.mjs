// Canvas editor integration: drawing, images, history, persistence and export.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1447';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1447', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
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
  await page.addInitScript(() => { delete window.showOpenFilePicker; });
  await page.goto(origin);

  const rail = page.getByRole('navigation', { name: '工作台活动栏' });
  await rail.getByRole('button', { name: '画布', exact: true }).click();
  await page.getByRole('button', { name: '新建画布', exact: true }).click();
  await page.locator('[data-canvas-surface] .excalidraw').waitFor();
  const surface = page.locator('[data-canvas-surface]');
  const bounds = await surface.boundingBox();
  const count = () => page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    return store.allGroups(store.getWorkbenchState().groups).flatMap(([, group]) => group.editors).find(editor => editor.kind === 'canvas')?.state?.objectCount ?? 0;
  });
  await page.getByTestId('toolbar-rectangle').locator('..').click();
  await page.mouse.move(bounds.x + 360, bounds.y + 220);
  await page.mouse.down(); await page.mouse.move(bounds.x + 550, bounds.y + 340, { steps: 8 }); await page.mouse.up();
  await page.waitForFunction(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    return store.allGroups(store.getWorkbenchState().groups).flatMap(([, group]) => group.editors).find(editor => editor.kind === 'canvas')?.state?.objectCount === 1;
  });
  assert.equal(await count(), 1);
  await page.keyboard.press('ControlOrMeta+Z');
  await page.waitForTimeout(150); assert.equal(await count(), 0, 'undo removes the rectangle');
  await page.keyboard.press('ControlOrMeta+Shift+Z');
  await page.waitForTimeout(150); assert.equal(await count(), 1, 'redo restores the rectangle');
  const imageData = await page.evaluate(() => {
    const canvas = document.createElement('canvas'); canvas.width = 64; canvas.height = 48;
    const context = canvas.getContext('2d'); context.fillStyle = '#d93838'; context.fillRect(0, 0, 64, 48);
    return canvas.toDataURL('image/png').split(',')[1];
  });
  const chooserPromise = page.waitForEvent('filechooser');
  await page.getByTestId('toolbar-image').locator('..').click();
  const chooser = await chooserPromise;
  await chooser.setFiles({ name: 'sample.png', mimeType: 'image/png', buffer: Buffer.from(imageData, 'base64') });
  await page.mouse.click(bounds.x + 650, bounds.y + 300);
  await page.waitForTimeout(300); assert.equal(await count(), 2, 'image is inserted as an editable object');
  await page.getByRole('textbox', { name: '画布名称' }).fill('基础画布测试');
  await page.getByRole('button', { name: '保存画布', exact: true }).click();
  await page.getByRole('button', { name: '基础画布测试', exact: true }).waitFor();
  await page.waitForFunction(() => !document.querySelector('[role="tab"][data-dirty="true"]'));
  const sourcePromise = page.waitForEvent('download');
  await page.getByRole('button', { name: '导出 源文件', exact: true }).click();
  const sourceDownload = await sourcePromise;
  const { readFile } = await import('node:fs/promises');
  const source = JSON.parse(await readFile(await sourceDownload.path(), 'utf8'));
  assert.equal(source.elements.filter(element => !element.isDeleted).length, 2);
  assert.equal(source.elements.find(element => element.type === 'rectangle').roughness, 0, 'default shapes have straight strokes');
  assert.equal(Object.keys(source.files).length, 1, 'source export includes image bytes');
  const svgPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: '导出 SVG', exact: true }).click();
  const svg = await svgPromise;
  assert.ok((await readFile(await svg.path(), 'utf8')).includes('<svg'));
  const pngPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: '导出 PNG', exact: true }).click();
  const png = await pngPromise;
  assert.equal((await readFile(await png.path())).subarray(1, 4).toString(), 'PNG');
  await page.reload();
  await page.getByRole('textbox', { name: '画布名称' }).waitFor();
  await page.waitForFunction(() => document.querySelector('input[aria-label="画布名称"]')?.value === '基础画布测试');
  await page.waitForTimeout(400); assert.equal(await count(), 2, 'saved objects and images restore after reload');
  await page.getByRole('tab', { name: '基础画布测试', exact: true }).getByRole('button', { name: '关闭 基础画布测试' }).click();
  await page.getByRole('button', { name: '基础画布测试', exact: true }).click();
  await page.getByRole('textbox', { name: '画布名称' }).waitFor();
  await page.waitForTimeout(400); assert.equal(await count(), 2, 'sidebar reopens a saved canvas');
  await page.screenshot({ path: '/tmp/autonomics-canvas-editor.png' });
  assert.deepEqual(errors, []);
  console.log('canvas-editor: all assertions passed');
} finally { await browser?.close(); server.kill(); }
