// Real Rust HTTP/SQLite/tools + GUI; only model decisions and unrelated APIs are fixtures.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const binary = process.env.VISUALIZATION_NATIVE_BIN || fileURLToPath(new URL('../../../target/debug/examples/visualization_native', import.meta.url));
assert.ok(existsSync(binary), 'build first: cargo build -p api-server --example visualization_native');
const directory = await mkdtemp(join(tmpdir(), 'autonomics-canvas-native-'));
const native = spawn(binary, [join(directory, 'figures.sqlite')], { stdio: ['ignore', 'pipe', 'pipe'] });
const origin = 'http://127.0.0.1:1448';
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1448', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let nativeOutput = '', viteOutput = '', browser, page;
native.stdout.on('data', chunk => nativeOutput += chunk); native.stderr.on('data', chunk => nativeOutput += chunk);
vite.stdout.on('data', chunk => viteOutput += chunk); vite.stderr.on('data', chunk => viteOutput += chunk);
const until = async (predicate, message) => { const deadline = Date.now() + 60000; while (!await predicate()) { if (Date.now() > deadline) throw new Error(message); await new Promise(resolve => setTimeout(resolve, 50)); } };
try {
  await until(() => { if (native.exitCode !== null || native.signalCode !== null) throw new Error(nativeOutput); return nativeOutput.includes('VISUALIZATION_NATIVE_ADDR='); }, 'native host readiness');
  await until(() => { if (vite.exitCode !== null || vite.signalCode !== null) throw new Error(viteOutput); return viteOutput.includes(origin); }, 'Vite readiness');
  const backend = `http://${nativeOutput.match(/VISUALIZATION_NATIVE_ADDR=([^\s]+)/)[1]}`;
  const call = async (method, path, body) => {
    const response = await fetch(`${backend}${path}`, { method, headers: body === undefined ? {} : { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(30000) });
    const json = await response.json(); assert.ok(response.ok, `${response.status} ${JSON.stringify(json)}`); return json;
  };
  const tool = async input => (await call('POST', '/test/visualization', input)).content;
  const turns = [], vision = [], errors = [];
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  page = await browser.newPage({ viewport: { width: 1600, height: 1000 } }); page.setDefaultTimeout(30000);
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/api/session', route => route.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const url = new URL(route.request().url()); const path = url.pathname.replace('/api/v1', '');
    if (path.startsWith('/visualization/')) { const response = await route.fetch({ url: `${backend}${url.pathname}${url.search}` }); return route.fulfill({ response }); }
    if (path === '/events') return new Promise(() => {});
    if (path === '/state') return route.fulfill({ json: { last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: { '/root/alpha': { id: 's1', title: 'Figure discussion', created_at: 1, last_active: 1 } }, display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false } } });
    if (path.endsWith('/history')) return route.fulfill({ json: { messages: [] } });
    if (path.endsWith('/messages') && route.request().method() === 'POST') {
      try {
        const text = route.request().postDataJSON().text; const request = JSON.parse(text.slice(text.indexOf('{\n')));
        const context = await tool({ action: 'read', project_id: request.project_id, figure_id: request.figure_id });
        const revision = context.metadata.revision;
        if (request.preview_available) { const image = await tool({ action: 'read_preview', project_id: request.project_id, figure_id: request.figure_id, expected_revision: revision }); assert.equal(image[1].type, 'image'); vision.push(image[1].source.media_type); }
        const input = turns.length === 0 ? {
          action: 'sketch', project_id: request.project_id, figure_id: request.figure_id, expected_revision: revision, title: 'Signal pathway',
          document: { message: 'Conceptual signaling example', audience: request.audience, nodes: [{ id: 'ligand', label: 'Ligand', column: 0, row: 0 }, { id: 'receptor', label: 'Receptor', column: 1, row: 0 }, { id: 'response', label: 'Response', column: 2, row: 0 }], edges: [{ from: 'ligand', to: 'receptor', label: 'binds' }, { from: 'receptor', to: 'response', label: 'activates' }], assumptions: ['Conceptual test fixture; no biological mechanism is asserted.'] },
        } : { action: 'propose', project_id: request.project_id, figure_id: request.figure_id, expected_revision: revision, title: 'Clarify ligand', operations: [{ op: 'update', id: 'label-0', changes: { text: 'Ligand A' } }] };
        const proposal = await tool(input); turns.push({ request, proposal }); return route.fulfill({ json: {} });
      } catch (error) { errors.push(String(error)); return route.fulfill({ status: 500, json: { error: String(error) } }); }
    }
    if (path === '/profiles') return route.fulfill({ json: ['researcher'] });
    if (path === '/storage/agents') return route.fulfill({ json: [] });
    if (path === '/model-config') return route.fulfill({ json: { models: [], providers: [] } });
    if (path === '/settings') return route.fulfill({ json: {} });
    if (path === '/gateway/status') return route.fulfill({ json: { running: true } });
    return route.fulfill({ status: 404, json: { error: `Unrelated fixture: ${path}` } });
  });
  await page.goto(origin, { waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.getByRole('navigation', { name: '工作台活动栏' }).getByRole('button', { name: '画布', exact: true }).click();
  await page.getByRole('textbox', { name: '新项目名称' }).fill('Native study');
  await page.getByRole('button', { name: '创建项目', exact: true }).click();
  await page.getByRole('button', { name: '新建画布', exact: true }).click();
  await page.locator('[data-canvas-surface] .excalidraw').waitFor();
  await page.getByRole('textbox', { name: '画布名称' }).fill('Native figure');
  const project = await page.getByRole('combobox', { name: '可视化项目' }).inputValue();
  const figures = await call('GET', `/api/v1/visualization/projects/${project}/figures`); assert.equal(figures.length, 1);
  const figure = figures[0].id; const path = `/api/v1/visualization/projects/${project}/figures/${figure}`;
  const handoff = async (message, count) => {
    await page.getByRole('button', { name: 'Agent 绘图', exact: true }).click();
    await page.getByRole('textbox', { name: '绘图受众' }).fill('Biology researchers');
    await page.getByRole('textbox', { name: '绘图需求' }).fill(message);
    await page.getByRole('button', { name: '交给 Agent', exact: true }).click();
    const draft = page.getByPlaceholder('输入消息…'); await draft.waitFor(); assert.ok((await draft.inputValue()).includes(message));
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await until(() => turns.length === count || errors.length, 'model fixture completion'); assert.deepEqual(errors, []);
    await page.getByRole('tab', { name: 'Native figure', exact: true }).click();
    await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Agent 修改预览' });
    await dialog.getByRole('button', { name: new RegExp(count === 1 ? 'Signal pathway' : 'Clarify ligand') }).click();
    await dialog.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
    await dialog.getByRole('button', { name: '采用提案', exact: true }).click(); await dialog.waitFor({ state: 'hidden' });
    await page.locator('[data-canvas-surface] .excalidraw').waitFor();
  };
  const source = async () => { const download = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 源文件', exact: true }).click(); return JSON.parse(await readFile(await (await download).path(), 'utf8')); };
  await handoff('Draw a ligand, receptor and response pathway.', 1);
  const first = await source(); const firstHead = await call('GET', path);
  assert.equal(first.elements.filter(element => element.type === 'text').length, 5);
  assert.equal(first.elements.filter(element => element.type === 'arrow').length, 2);
  await handoff('Rename Ligand to Ligand A and keep the rest unchanged.', 2);
  const second = await source(); assert.equal(second.elements.find(element => element.id === 'label-0').text, 'Ligand A');
  assert.deepEqual(second.elements.find(element => element.id === 'node-1'), first.elements.find(element => element.id === 'node-1'));
  assert.deepEqual(vision, ['image/png']); assert.equal((await call('GET', `/api/v1/visualization/projects/${project}/figures`)).length, 1);
  const beforeRestore = (await call('GET', path)).metadata.revision;
  await page.getByRole('button', { name: '历史版本', exact: true }).click();
  const history = page.getByRole('dialog', { name: '画布历史' });
  await history.getByRole('button', { name: new RegExp(`^v${firstHead.metadata.revision} ·`) }).click();
  await history.getByRole('button', { name: '恢复为新版本', exact: true }).click(); await history.waitFor({ state: 'hidden' });
  const restored = await call('GET', path); assert.equal(restored.metadata.revision, beforeRestore + 1);
  assert.equal(restored.scene.document.elements.find(element => element.id === 'label-0').text, 'Ligand');
  assert.equal((await call('GET', `${path}/revisions/${beforeRestore}`)).document.elements.find(element => element.id === 'label-0').text, 'Ligand A');
  await page.locator('[data-canvas-surface] .excalidraw').waitFor();
  const download = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 SVG', exact: true }).click();
  assert.ok((await readFile(await (await download).path(), 'utf8')).includes('<svg'));
  await page.screenshot({ path: '/tmp/autonomics-canvas-native.png' }); assert.deepEqual(errors, []);
  console.log('canvas-native: real tools, HTTP, SQLite, preview, local edit, restore and export passed');
} catch (error) {
  await page?.screenshot({ path: "/tmp/autonomics-canvas-native-failure.png" }).catch(() => {});
  throw error;
} finally {
  await browser?.close(); native.kill(); vite.kill();
  await Promise.all([native, vite].map(child => (child.exitCode !== null || child.signalCode !== null) ? Promise.resolve() : new Promise(resolve => child.once('exit', resolve))));
  await rm(directory, { recursive: true, force: true });
}
