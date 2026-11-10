// Real Rust HTTP/SQLite/tools + GUI; only model decisions and unrelated APIs are fixtures.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
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
  const turns = [], vision = [], markupVision = [], errors = [], fontRequests = [];
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  page = await browser.newPage({ viewport: { width: 1600, height: 1000 } }); page.setDefaultTimeout(30000);
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/*.woff2*', route => {
    const url = new URL(route.request().url()); fontRequests.push(url.href);
    return url.origin === origin ? route.continue() : route.abort('blockedbyclient');
  });
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
        for (const review of request.markups || []) {
          const blocks = await tool({ action: 'read_markup', project_id: request.project_id, figure_id: request.figure_id, markup_id: review.id, expected_revision: revision });
          assert.equal(blocks[1].type, 'image'); const caption = JSON.parse(blocks[0].text);
          assert.equal(caption.at_revision, review.at_revision); markupVision.push(caption);
        }
        const input = turns.length === 0 ? {
          action: 'sketch', project_id: request.project_id, figure_id: request.figure_id, expected_revision: revision, title: 'Signal pathway',
          document: { message: 'Conceptual signaling example', audience: request.audience, layout: 'horizontal', nodes: [{ id: 'ligand', label: 'Ligand' }, { id: 'receptor', label: 'Receptor' }, { id: 'response', label: 'Response' }], edges: [{ from: 'ligand', to: 'receptor', label: 'binds' }, { from: 'receptor', to: 'response', label: 'activates' }], assumptions: ['Conceptual test fixture; no biological mechanism is asserted.'] },
        } : { action: 'propose', project_id: request.project_id, figure_id: request.figure_id, expected_revision: revision, title: 'Clarify ligand', markup_ids: (request.markups || []).map(item => item.id), operations: [{ op: 'update', id: 'label-0', changes: { text: 'Ligand A' } }] };
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
    assert.equal(await page.locator('.editor-body-occluded [data-canvas-surface]').count(), 1);
    assert.deepEqual(await page.locator('.editor-body-occluded').evaluateAll(bodies => bodies.flatMap(body => [...body.querySelectorAll('*')].filter(element => getComputedStyle(element).visibility === 'visible' && element.getClientRects().length).map(element => element.tagName))), [], 'inactive canvas descendants must remain visually hidden');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await until(() => turns.length === count || errors.length, 'model fixture completion'); assert.deepEqual(errors, []);
    await page.getByRole('tab', { name: 'Native figure', exact: true }).click();
    await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Agent 修改预览' });
    await dialog.getByRole('button', { name: new RegExp(count === 1 ? 'Signal pathway' : 'Clarify ligand') }).click();
    await dialog.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
    if (count === 2) {
      await dialog.getByRole('region', { name: '提案关联标记' }).getByText('Rename the ligand; the marks illustrate review intent.', { exact: true }).waitFor();
      const complete = dialog.getByRole('checkbox', { name: '采用时同时完成关联标记（1 组）', exact: true });
      assert.equal(await complete.isChecked(), false, 'completion requires an explicit user choice');
      assert.equal((await call('GET', `${path}/markups`))[0].status, 'open', 'proposal alone leaves the review open');
      await complete.check();
    }
    await dialog.getByRole('button', { name: '采用提案', exact: true }).click(); await dialog.waitFor({ state: 'hidden' });
    await page.locator('[data-canvas-surface] .excalidraw').waitFor();
  };
  const source = async () => { const download = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 源文件', exact: true }).click(); return JSON.parse(await readFile(await (await download).path(), 'utf8')); };
  await handoff('Draw a ligand, receptor and response pathway.', 1);
  const first = await source(); const firstHead = await call('GET', path);
  assert.equal(first.elements.filter(element => element.type === 'text').length, 5);
  assert.equal(first.elements.filter(element => element.type === 'arrow').length, 2);
  await page.getByRole('button', { name: '草图标记', exact: true }).click();
  const markup = page.getByRole('dialog', { name: '草图标记与审阅' });
  await markup.getByRole('button', { name: '新建标记', exact: true }).click();
  const ink = markup.getByRole('application', { name: '草图标记区域' }); await ink.waitFor();
  await markup.getByRole('region', { name: '草图标记预览' }).locator('.excalidraw').waitFor();
  const bounds = await ink.boundingBox();
  const stroke = async (mode, x, y, dx, dy) => {
    await markup.getByRole('button', { name: mode, exact: true }).click();
    await page.mouse.move(bounds.x + x, bounds.y + y); await page.mouse.down();
    await page.mouse.move(bounds.x + x + dx, bounds.y + y + dy, { steps: 10 }); await page.mouse.up();
  };
  const firstX = bounds.width / 2 - 400, middleY = bounds.height / 2;
  await stroke('补画', firstX - 50, middleY - 160, 100, 50);
  await stroke('划除', firstX - 40, middleY - 40, 80, 80);
  await stroke('强调', firstX + 740, middleY - 60, 120, 120);
  await stroke('补画', firstX + 200, middleY - 180, 50, 40);
  await markup.getByRole('button', { name: '撤销笔迹', exact: true }).click();
  page.once('dialog', dialog => dialog.dismiss());
  await markup.getByRole('button', { name: '关闭标记', exact: true }).click();
  await ink.waitFor();
  await markup.getByRole('textbox', { name: '标记修改说明' }).fill('Rename the ligand; the marks illustrate review intent.');
  await markup.getByRole('button', { name: '保存草图标记', exact: true }).click();
  await markup.getByRole('img', { name: '已保存的草图标记' }).waitFor();
  const reviews = await call('GET', `${path}/markups`); assert.equal(reviews.length, 1); assert.equal(reviews[0].stroke_count, 3);
  assert.equal(reviews[0].at_revision, firstHead.metadata.revision);
  assert.equal((await call('GET', path)).metadata.revision, firstHead.metadata.revision, 'review ink does not create an artwork revision');
  const savedReview = await call('GET', `${path}/markups/${reviews[0].id}`);
  assert.deepEqual(savedReview.strokes.map(item => item.kind), ['draw', 'cross', 'emphasis']);
  assert.ok(savedReview.dataURL.startsWith('data:image/png;base64,'));
  const inkPixels = await page.evaluate(async url => {
    const image = new Image(); image.src = url; await image.decode();
    const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
    const ctx = canvas.getContext('2d'); ctx.drawImage(image, 0, 0); const bytes = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    const counts = { red: 0, blue: 0, orange: 0 };
    for (let i = 0; i < bytes.length; i += 4) { const [r, g, b] = bytes.slice(i, i + 3); if (r > 170 && g < 90 && b < 90) counts.red++; if (b > 130 && r < 70 && g > 70 && g < 150) counts.blue++; if (r > 170 && g > 85 && g < 160 && b < 60) counts.orange++; }
    return counts;
  }, savedReview.dataURL);
  for (const [color, count] of Object.entries(inkPixels)) assert.ok(count > 30, `${color} review ink appears in the composite preview`);
  const cross = savedReview.strokes[1].points;
  const ligand = first.elements.find(element => element.id === 'node-0');
  assert.ok(cross[0][0] < ligand.x + ligand.width / 2 && cross[1][0] > ligand.x + ligand.width / 2, 'crossing the visible ligand retains scene coordinates');
  assert.ok(cross[0][1] < ligand.y + ligand.height / 2 && cross[1][1] > ligand.y + ligand.height / 2);
  await page.screenshot({ path: '/tmp/autonomics-canvas-markup.png' });
  await markup.getByRole('button', { name: '关闭标记', exact: true }).click();
  assert.deepEqual((await source()).elements, first.elements, 'review drawing never enters editable artwork');
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.locator('[data-canvas-surface] .excalidraw').waitFor();
  await page.getByRole('button', { name: '草图标记', exact: true }).click();
  await markup.getByRole('button', { name: '查看标记', exact: true }).click();
  await markup.getByRole('img', { name: '已保存的草图标记' }).waitFor();
  await markup.getByRole('button', { name: '关闭标记', exact: true }).click();
  await handoff('Rename Ligand to Ligand A and keep the rest unchanged.', 2);
  const second = await source(); assert.equal(second.elements.find(element => element.id === 'label-0').text, 'Ligand A');
  const untouched = element => { const { updated, versionNonce, ...content } = element; return content; };
  // Excalidraw repairs bound-element bookkeeping on reload, refreshing these
  // volatile fields. Compare persisted objects in full and rendered content separately.
  assert.deepEqual(untouched(second.elements.find(element => element.id === 'node-1')), untouched(first.elements.find(element => element.id === 'node-1')));
  const afterLocalEdit = await call('GET', path);
  assert.deepEqual(afterLocalEdit.scene.document.elements.find(element => element.id === 'node-1'), firstHead.scene.document.elements.find(element => element.id === 'node-1'), 'agent changes preserve the entire unrelated persisted object');
  assert.equal((await call('GET', `${path}/markups`))[0].status, 'resolved', 'adoption explicitly completes its associated review');
  assert.equal(markupVision.length, 1); assert.equal(markupVision[0].historical, false);
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
  const segmentHits = (a, b, box) => {
    let lo = 0, hi = 1;
    for (const [axis, min, max] of [[0, box.x + .01, box.x + box.width - .01], [1, box.y + .01, box.y + box.height - .01]]) {
      const delta = b[axis] - a[axis];
      if (Math.abs(delta) < 1e-9) { if (a[axis] < min || a[axis] > max) return false; }
      else { const t1 = (min - a[axis]) / delta, t2 = (max - a[axis]) / delta; lo = Math.max(lo, Math.min(t1, t2)); hi = Math.min(hi, Math.max(t1, t2)); }
    }
    return lo < hi;
  };
  const nodes = [{ id: 'a', label: 'Ligand' }, { id: 'b', label: 'Receptor complex with intracellular signaling components', shape: 'ellipse' }, { id: 'c', label: 'Parallel response' }, { id: 'd', label: 'Outcome' }];
  const edges = [{ from: 'a', to: 'b', label: 'binds' }, { from: 'a', to: 'c', label: 'activates' }, { from: 'b', to: 'd', label: 'stimulates' }, { from: 'c', to: 'd', label: 'contributes' }, { from: 'a', to: 'd', label: 'explicit cross-layer relation' }];
  const cases = [
    { name: 'vertical', layout: 'vertical', nodes, edges },
    { name: 'feedback', layout: 'horizontal', nodes, edges: [...edges, { from: 'd', to: 'a', label: 'negative feedback', inhibitory: true }, { from: 'a', to: 'b', label: 'second explicit relation' }] },
    { name: 'font-metrics', layout: 'horizontal', nodes: [{ id: 'a', label: 'Treatment input' }, { id: 'b', label: 'IL-6 receptor signaling' }], edges: [{ from: 'a', to: 'b', label: 'AV αβ γδ' }] },
    { name: 'multilingual', layout: 'vertical', nodes: [{ id: 'a', label: '细胞膜受体复合物与下游转录调控因子之间的相互作用以及细胞内信号传导机制', shape: 'ellipse' }, { id: 'b', label: 'WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW' }], edges: [{ from: 'a', to: 'b', label: '明确的受体调控（α/β）' }] },
  ];
  for (const item of cases) {
    const title = `Layout ${item.name}`;
    const created = await tool({ action: 'sketch', project_id: project, title, document: { message: 'Layout test fixture with explicit conceptual relations', audience: 'Researchers', layout: item.layout, nodes: item.nodes, edges: item.edges, assumptions: ['Layout fixture only; biological meaning requires review.'] } });
    assert.ok(created.id);
    await page.getByRole('button', { name: '刷新画布列表', exact: true }).click();
    await page.getByRole('button', { name: title, exact: true }).click();
    await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
    const scene = await source();
    assert.ok(scene.elements.filter(e => e.type === 'text').every(e => e.fontFamily === 9), 'generated labels use the pinned local font');
    const storedForFont = await call('GET', `/api/v1/visualization/projects/${project}/figures/${created.id}`);
    for (const label of storedForFont.scene.document.elements.filter(e => e.type === 'text' && e.containerId?.startsWith('edge-') && /^[\x20-\x7e\u0370-\u03ff\u2e80-\u9fff\uf900-\ufaff\ufe10-\ufe4f\uff00-\uffef]+$/.test(e.text.replace(/\n/g, '')))) {
      const measured = await page.evaluate(async ({ text, size }) => {
        await document.fonts.load(`${size}px "Liberation Sans"`);
        const context = document.createElement('canvas').getContext('2d'); context.font = `${size}px "Liberation Sans"`;
        return Math.max(...text.split('\n').map(line => context.measureText(line).width));
      }, { text: label.text, size: label.fontSize });
      assert.ok(Math.abs(measured - label.width) < 1, `${item.name}: font metric ${measured} agrees with backend width ${label.width}`);
    }
    const boxes = scene.elements.filter(element => ['rectangle', 'ellipse'].includes(element.type));
    const arrows = scene.elements.filter(element => element.type === 'arrow');
    assert.equal(boxes.length, item.nodes.length); assert.equal(arrows.length, item.edges.length);
    assert.equal(boxes[0].customData.scientificPlan.layout, item.layout);
    assert.equal(boxes[0].customData.layoutInfo.font_sha256, createHash('sha256').update(await readFile('public/excalidraw/fonts/Liberation/LiberationSans-Regular.woff2')).digest('hex'));
    for (const box of boxes) {
      const label = scene.elements.find(element => element.type === 'text' && element.containerId === box.id);
      assert.ok(label.x >= box.x - 1 && label.y >= box.y - 1 && label.x + label.width <= box.x + box.width + 1 && label.y + label.height <= box.y + box.height + 1, `${item.name}: restored label remains inside its node`);
    }
    for (const arrow of arrows) {
      const points = arrow.points.map(point => [arrow.x + point[0], arrow.y + point[1]]);
      for (const box of boxes.filter(box => ![arrow.startBinding?.elementId, arrow.endBinding?.elementId].includes(box.id))) {
        for (let index = 1; index < points.length; index++) assert.ok(!segmentHits(points[index - 1], points[index], box), `${item.name}: restored route ${arrow.id} avoids unrelated ${box.id}`);
      }
    }
    const surface = await page.locator('[data-canvas-surface]:visible').boundingBox();
    const zoom = Number.parseFloat(await page.locator('[data-canvas-surface] .excalidraw:visible .reset-zoom-button').innerText()) / 100;
    const points = scene.elements.flatMap(element => element.type === 'arrow' ? element.points.map(p => [element.x + p[0], element.y + p[1]]) : [[element.x, element.y], [element.x + element.width, element.y + element.height]]);
    const width = Math.max(...points.map(p => p[0])) - Math.min(...points.map(p => p[0]));
    const height = Math.max(...points.map(p => p[1])) - Math.min(...points.map(p => p[1]));
    assert.ok(width * (zoom - .01) <= surface.width - 100 && height * (zoom - .01) <= surface.height - 140, `${item.name}: initial viewport fits the complete figure`);
    if (item.name === 'feedback') {
      const recordPath = `/api/v1/visualization/projects/${project}/figures/${created.id}`;
      const head = await call('GET', recordPath);
      const originalNode = head.scene.document.elements.find(element => element.id === 'node-0');
      const originalLabel = head.scene.document.elements.find(element => element.id === 'edge-label-5');
      const proposal = await tool({ action: 'propose', project_id: project, figure_id: created.id, expected_revision: head.metadata.revision, title: 'Move feedback endpoint', operations: [{ op: 'update', id: 'node-0', changes: { x: originalNode.x + 40, y: originalNode.y + 30 } }] });
      const candidate = (await call('GET', `${recordPath}/proposals`)).find(entry => entry.id === proposal.id);
      const candidateLabel = candidate.document.elements.find(element => element.id === 'edge-label-5');
      assert.deepEqual([candidateLabel.x, candidateLabel.y], [originalLabel.x, originalLabel.y], 'real agent tool keeps the feedback label on its fixed middle segment');
      await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: 'Agent 修改预览' });
      await dialog.getByRole('button', { name: /Move feedback endpoint/ }).click();
      await dialog.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
      await dialog.getByRole('button', { name: '采用提案', exact: true }).click(); await dialog.waitFor({ state: 'hidden' });
      await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
      const adopted = await call('GET', recordPath);
      assert.equal(adopted.scene.document.elements.find(element => element.id === 'node-0').x, originalNode.x + 40);
    }
    await page.screenshot({ path: `/tmp/autonomics-layout-${item.name}.png` });
    const exported = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 SVG', exact: true }).click();
    const exportedSvg = await readFile(await (await exported).path(), 'utf8');
    assert.ok(exportedSvg.includes('<svg'));
    if (item.name === 'multilingual') {
      assert.ok(exportedSvg.includes('细胞') && /data:font\/woff2;base64,/.test(exportedSvg));
      await writeFile('/tmp/autonomics-cjk-font-export.svg', exportedSvg);
    }
  }
  assert.deepEqual(errors, []);
  const plannedFigure = await tool({ action: 'create', project_id: project, title: 'Planning study', document: { type: 'excalidraw', version: 2, elements: [], files: {} } });
  const plannedPath = `/api/v1/visualization/projects/${project}/figures/${plannedFigure.id}`;
  const initialBrief = { message: 'Explain a conceptual ligand and receptor interaction', audience: 'Biology researchers', layout: 'horizontal', nodes: [{ id: 'ligand', label: 'Ligand' }, { id: 'receptor', label: 'Receptor' }], edges: [{ from: 'ligand', to: 'receptor', label: 'binds' }], assumptions: ['Conceptual example; no specific pathway is asserted.'], questions: [{ id: 'specificity', question: 'Should a specific receptor or a generic receptor be shown?', answer: '' }] };
  const savedBrief = await tool({ action: 'save_plan', project_id: project, figure_id: plannedFigure.id, expected_revision: 1, planning_version: 0, document: initialBrief });
  assert.equal(savedBrief.status, 'needs_clarification');
  await page.getByRole('button', { name: '刷新画布列表', exact: true }).click();
  await page.getByRole('button', { name: 'Planning study', exact: true }).click();
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  await page.getByRole('button', { name: '绘图计划', exact: true }).click();
  const planningDialog = page.getByRole('dialog', { name: '科研绘图计划' });
  await planningDialog.getByRole('textbox', { name: '问题答案 1', exact: true }).waitFor();
  assert.equal(await planningDialog.getByRole('button', { name: '生成草图提案', exact: true }).isEnabled(), false);
  assert.equal(await planningDialog.getByRole('textbox', { name: '澄清问题 1', exact: true }).inputValue(), initialBrief.questions[0].question);
  const answer = 'Use a generic receptor; do not name a pathway.';
  await planningDialog.getByRole('textbox', { name: '问题答案 1', exact: true }).fill(answer);
  page.once('dialog', dialog => dialog.dismiss()); await planningDialog.getByRole('button', { name: '关闭计划', exact: true }).click();
  assert.equal(await planningDialog.getByRole('textbox', { name: '问题答案 1', exact: true }).inputValue(), answer);
  await planningDialog.getByRole('button', { name: '保存绘图计划', exact: true }).click();
  await planningDialog.getByRole('status').filter({ hasText: '可生成草图' }).waitFor();
  assert.equal((await call('GET', plannedPath)).metadata.revision, 1, 'clarification does not mutate artwork');
  const readyBrief = await tool({ action: 'read_plan', project_id: project, figure_id: plannedFigure.id });
  assert.equal(readyBrief.version, 2); assert.equal(readyBrief.document.questions[0].answer, answer);
  assert.equal((await call('GET', `${plannedPath}/planning/revisions/1`)).document.questions[0].answer, '');
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  await page.getByRole('button', { name: '绘图计划', exact: true }).click();
  await planningDialog.getByRole('textbox', { name: '问题答案 1', exact: true }).waitFor();
  assert.equal(await planningDialog.getByRole('textbox', { name: '问题答案 1', exact: true }).inputValue(), answer);
  await page.screenshot({ path: '/tmp/autonomics-planning-ready.png', animations: 'disabled' });
  await planningDialog.getByRole('button', { name: '生成草图提案', exact: true }).click();
  const plannedPreview = page.getByRole('dialog', { name: 'Agent 修改预览' });
  await plannedPreview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  await plannedPreview.getByRole('region', { name: '修改前画布' }).locator('.excalidraw').waitFor();
  assert.equal((await call('GET', plannedPath)).metadata.revision, 1, 'planned sketch remains a proposal until adopted');
  await page.screenshot({ path: '/tmp/autonomics-planning-proposal.png' });
  await plannedPreview.getByRole('button', { name: '采用提案', exact: true }).click(); await plannedPreview.waitFor({ state: 'hidden' });
  const planned = await call('GET', plannedPath); assert.equal(planned.metadata.revision, 2);
  assert.equal(planned.scene.document.elements[0].customData.planningSource.document.questions[0].answer, answer);
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const plannedSource = await source(); const bindingArrow = plannedSource.elements.find(element => element.type === 'arrow');
  const bindingLabel = plannedSource.elements.find(element => element.type === 'text' && element.containerId === bindingArrow.id);
  assert.ok(bindingLabel.width < bindingArrow.width / 2, 'restored short caption leaves a visible arrow body');
  await page.getByRole('button', { name: '绘图计划', exact: true }).click();
  await planningDialog.getByText('画布版本已变化。请核对计划与当前图稿，保存后再生成草图。', { exact: true }).waitFor();
  assert.equal(await planningDialog.getByRole('button', { name: '生成草图提案', exact: true }).isEnabled(), false);
  await planningDialog.getByRole('button', { name: '关闭计划', exact: true }).click();
  await page.screenshot({ path: '/tmp/autonomics-planning-canvas.png' });
  await page.getByRole('button', { name: 'Agent 绘图', exact: true }).click();
  await page.getByRole('textbox', { name: '绘图需求' }).fill('Continue discussing the saved scientific plan.');
  await page.getByRole('button', { name: '交给 Agent', exact: true }).click();
  const plannedComposer = page.getByPlaceholder('输入消息…'); await plannedComposer.waitFor();
  const plannedDraft = await plannedComposer.inputValue();
  const plannedContext = JSON.parse(plannedDraft.slice(plannedDraft.indexOf('{\n')));
  assert.equal(plannedContext.figure_id, plannedFigure.id); assert.equal(plannedContext.planning.version, 2); assert.equal(plannedContext.planning.based_on_revision, 1);
  assert.ok(plannedDraft.includes('read_plan')); assert.deepEqual(errors, []);
  const panelFigure = await tool({ action: 'create', project_id: project, title: 'Panel comparison', document: { type: 'excalidraw', version: 2, elements: [], files: {} } });
  const panelPath = `/api/v1/visualization/projects/${project}/figures/${panelFigure.id}`;
  await tool({ action: 'save_plan', project_id: project, figure_id: panelFigure.id, expected_revision: 1, planning_version: 0, document: {
    message: 'Compare two explicitly supplied conceptual conditions', audience: 'Researchers', layout: 'horizontal',
    nodes: [{ id: 'a', label: 'Control input' }, { id: 'b', label: 'Control output' }, { id: 'c', label: 'Treatment input' }, { id: 'd', label: 'Treatment output' }],
    edges: [{ from: 'a', to: 'b', label: 'conceptual relation' }, { from: 'c', to: 'd', label: 'explicit comparison' }], assumptions: ['Synthetic comparison; no scientific claim.'],
  } });
  await page.getByRole('tab', { name: 'Planning study', exact: true }).click();
  await page.getByRole('button', { name: '刷新画布列表', exact: true }).click();
  await page.getByRole('button', { name: 'Panel comparison', exact: true }).click();
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  await page.getByRole('button', { name: '绘图计划', exact: true }).click();
  await planningDialog.getByRole('textbox', { name: '核心图意', exact: true }).waitFor();
  await planningDialog.getByRole('button', { name: '添加面板', exact: true }).click();
  await planningDialog.getByRole('textbox', { name: '面板标题 1', exact: true }).fill('A · Control');
  await planningDialog.getByRole('button', { name: '添加面板', exact: true }).click();
  await planningDialog.getByRole('textbox', { name: '面板标题 2', exact: true }).fill('B · Treatment');
  await planningDialog.getByRole('combobox', { name: '组件面板 3', exact: true }).selectOption('p2');
  await planningDialog.getByRole('combobox', { name: '组件面板 4', exact: true }).selectOption('p2');
  await planningDialog.getByRole('button', { name: '添加关系', exact: true }).click();
  await planningDialog.getByRole('combobox', { name: '关系来源 3', exact: true }).selectOption('b');
  await planningDialog.getByRole('combobox', { name: '关系目标 3', exact: true }).selectOption('c');
  await planningDialog.getByRole('textbox', { name: '关系说明 3', exact: true }).fill('carry forward');
  await planningDialog.getByRole('button', { name: '保存绘图计划', exact: true }).click();
  await planningDialog.getByRole('status').filter({ hasText: '计划 v2' }).waitFor();
  assert.deepEqual((await call('GET', `${panelPath}/planning`)).document.nodes.map(n => n.panel), ['p1', 'p1', 'p2', 'p2']);
  await planningDialog.getByRole('button', { name: '关闭计划', exact: true }).click();
  await page.getByRole('button', { name: '绘图计划', exact: true }).click();
  await planningDialog.getByRole('textbox', { name: '面板标题 2', exact: true }).waitFor();
  assert.equal(await planningDialog.getByRole('textbox', { name: '面板标题 2', exact: true }).inputValue(), 'B · Treatment');
  await planningDialog.getByRole('button', { name: '生成草图提案', exact: true }).click();
  const panelPreview = page.getByRole('dialog', { name: 'Agent 修改预览' });
  await panelPreview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  assert.equal((await call('GET', panelPath)).scene.document.elements.length, 0);
  await page.screenshot({ path: '/tmp/autonomics-panels-proposal.png' });
  await panelPreview.getByRole('button', { name: '采用提案', exact: true }).click(); await panelPreview.waitFor({ state: 'hidden' });
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const panelScene = await source();
  assert.equal(panelScene.elements.filter(e => e.type === 'frame').length, 2);
  assert.equal(panelScene.elements.find(e => e.id === 'node-2').frameId, 'panel-1');
  assert.equal(panelScene.elements.find(e => e.id === 'panel-title-0').text, 'A · Control');
  assert.equal(panelScene.elements.find(e => e.id === 'label-2').text, 'Treatment input', 'font metrics avoid premature word wrapping');
  const crossArrow = panelScene.elements.find(e => e.id === 'edge-2'); assert.ok(crossArrow);
  assert.equal(crossArrow.frameId, null); assert.equal(crossArrow.startBinding.elementId, 'node-1'); assert.equal(crossArrow.endBinding.elementId, 'node-2');
  const crossLabel = panelScene.elements.find(e => e.id === 'edge-label-2'); assert.ok(crossLabel);
  for (const box of panelScene.elements.filter(e => ['rectangle', 'ellipse'].includes(e.type))) {
    const points = crossArrow.points.map(p => [crossArrow.x + p[0], crossArrow.y + p[1]]);
    for (let i = 1; i < points.length; i++) assert.ok(!segmentHits(points[i - 1], points[i], box), 'restored cross-panel route avoids all nodes');
    assert.ok(crossLabel.x + crossLabel.width <= box.x || crossLabel.x >= box.x + box.width || crossLabel.y + crossLabel.height <= box.y || crossLabel.y >= box.y + box.height, 'restored cross-panel caption avoids nodes');
  }
  const edgeEnd = (edge, last) => { const p = last ? edge.points.at(-1) : edge.points[0]; return [edge.x + p[0], edge.y + p[1]]; };
  const panelHead = await call('GET', panelPath); const frame = panelHead.scene.document.elements.find(e => e.id === 'panel-0');
  const panelMove = await tool({ action: 'propose', project_id: project, figure_id: panelFigure.id, expected_revision: panelHead.metadata.revision, title: 'Move control panel', operations: [{ op: 'update', id: frame.id, changes: { x: frame.x + 30, y: frame.y + 40 } }] });
  const movedPanel = (await call('GET', `${panelPath}/proposals`)).find(p => p.id === panelMove.id);
  for (const before of panelHead.scene.document.elements) {
    const after = movedPanel.document.elements.find(e => e.id === before.id);
    if (before.frameId === frame.id || before.id === frame.id) {
      assert.equal(after.x, before.x + 30); assert.equal(after.y, before.y + 40);
    } else if (before.id === 'edge-2') {
      assert.deepEqual(edgeEnd(after, false), edgeEnd(before, false).map((v, i) => v + [30, 40][i]));
      assert.deepEqual(edgeEnd(after, true), edgeEnd(before, true));
    } else if (before.id === 'edge-label-2') {
      assert.equal(after.text, before.text);
    } else assert.deepEqual(after, before);
  }
  await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
  await panelPreview.getByRole('button', { name: /Move control panel/ }).click();
  await panelPreview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  await panelPreview.getByRole('button', { name: '采用提案', exact: true }).click(); await panelPreview.waitFor({ state: 'hidden' });
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const movedSource = await source();
  assert.equal(movedSource.elements.find(e => e.id === 'node-0').x, panelScene.elements.find(e => e.id === 'node-0').x + 30);
  const frameHandle = page.locator('[data-canvas-surface]:visible [id$="-frame-name-panel-0"]');
  await frameHandle.waitFor(); const handleBox = await frameHandle.boundingBox();
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2); await page.mouse.down();
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2 + 60, { steps: 10 }); await page.mouse.up();
  const dragged = await source(); const oldFrame = movedSource.elements.find(e => e.id === 'panel-0');
  const draggedFrame = dragged.elements.find(e => e.id === 'panel-0');
  const dragX = draggedFrame.x - oldFrame.x, dragY = draggedFrame.y - oldFrame.y;
  assert.ok(dragY > 30, 'native frame handle moves the panel interactively');
  for (const before of movedSource.elements) {
    const after = dragged.elements.find(e => e.id === before.id);
    if (before.id === 'edge-2') {
      for (const last of [false, true]) {
        const key = last ? 'endBinding' : 'startBinding';
        assert.equal(after[key].elementId, before[key].elementId);
        const node = dragged.elements.find(e => e.id === after[key].elementId);
        const point = edgeEnd(after, last);
        const distance = Math.hypot(Math.max(node.x - point[0], 0, point[0] - node.x - node.width), Math.max(node.y - point[1], 0, point[1] - node.y - node.height));
        assert.ok(distance >= after[key].gap / 2 && distance <= after[key].gap * 2 + 1, 'native binding remains just outside its actual node boundary');
      }
      continue;
    }
    if (before.id === 'edge-label-2') { assert.equal(after.text, before.text); continue; }
    const belongs = before.frameId === 'panel-0' || before.id === 'panel-0';
    assert.ok(Math.abs(after.x - before.x - (belongs ? dragX : 0)) < .01);
    assert.ok(Math.abs(after.y - before.y - (belongs ? dragY : 0)) < .01);
    assert.equal(after.frameId, before.frameId);
  }
  await page.getByRole('button', { name: '保存画布', exact: true }).click();
  await until(async () => (await call('GET', panelPath)).scene.document.elements.find(e => e.id === 'panel-0').y === draggedFrame.y, 'save interactive panel movement');
  await page.getByRole('button', { name: '重新加载服务端版本', exact: true }).click();
  const reloadedPanel = await source();
  assert.equal(reloadedPanel.elements.find(e => e.id === 'panel-0').y, draggedFrame.y);
  assert.equal(reloadedPanel.elements.find(e => e.id === 'node-0').frameId, 'panel-0');
  const beforeRouting = await call('GET', panelPath);
  const currentArrow = beforeRouting.scene.document.elements.find(e => e.id === 'edge-2');
  const [start, end] = [edgeEnd(currentArrow, false), edgeEnd(currentArrow, true)];
  const blocker = { ...beforeRouting.scene.document.elements.find(e => e.id === 'node-0'), id: 'routing-obstacle', x: (start[0] + end[0]) / 2 - 30, y: (start[1] + end[1]) / 2 - 25, width: 60, height: 50, frameId: null, boundElements: [], customData: { fixture: true }, backgroundColor: '#fff3bf' };
  await tool({ action: 'apply', project_id: project, figure_id: panelFigure.id, expected_revision: beforeRouting.metadata.revision, operations: [{ op: 'add', element: blocker }] });
  await page.getByRole('button', { name: '重新加载服务端版本', exact: true }).click();
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const routingHead = await call('GET', panelPath);
  const existingPending = (await call('GET', `${panelPath}/proposals`)).filter(p => p.status === 'pending'); assert.equal(existingPending.length, 0);
  const invalidReroute = await fetch(`${backend}${panelPath}/proposals`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ expected_revision: routingHead.metadata.revision, summary: 'Invalid internal connector', operations: [{ op: 'reroute', id: 'edge-0' }] }) });
  assert.equal(invalidReroute.status, 400); assert.equal((await call('GET', `${panelPath}/proposals`)).filter(p => p.status === 'pending').length, 0);
  await page.getByRole('button', { name: '整理跨面板连线', exact: true }).click();
  await panelPreview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  const routingCandidates = (await call('GET', `${panelPath}/proposals`)).filter(p => p.status === 'pending'); assert.equal(routingCandidates.length, 1);
  const routingCandidate = routingCandidates[0];
  assert.equal((await call('GET', panelPath)).metadata.revision, routingHead.metadata.revision, 'rerouting is a proposal until adopted');
  for (const element of routingHead.scene.document.elements.filter(e => !['edge-2', 'edge-label-2'].includes(e.id))) assert.deepEqual(routingCandidate.document.elements.find(e => e.id === element.id), element, `reroute preserves unrelated ${element.id}`);
  await page.screenshot({ path: '/tmp/autonomics-panels-reroute-proposal.png' });
  await panelPreview.getByRole('button', { name: '采用提案', exact: true }).click(); await panelPreview.waitFor({ state: 'hidden' });
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const routedScene = await source(); const routedArrow = routedScene.elements.find(e => e.id === 'edge-2');
  const routedPoints = routedArrow.points.map(p => [routedArrow.x + p[0], routedArrow.y + p[1]]);
  for (const obstacle of routedScene.elements.filter(e => ['rectangle', 'ellipse'].includes(e.type))) {
    for (let i = 1; i < routedPoints.length; i++) assert.ok(!segmentHits(routedPoints[i - 1], routedPoints[i], obstacle), `restored reroute avoids ${obstacle.id}`);
  }
  assert.equal(routedScene.elements.find(e => e.id === 'edge-label-2').text, reloadedPanel.elements.find(e => e.id === 'edge-label-2').text);
  await page.screenshot({ path: '/tmp/autonomics-panels.png' });
  const panelExport = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 SVG', exact: true }).click();
  const panelSvg = await readFile(await (await panelExport).path(), 'utf8'); assert.ok(panelSvg.includes('Control') && panelSvg.includes('Treatment'));
  if (process.env.VISUALIZATION_BIOICONS_LIVE === '1') {
    const beforeVector = await source();
    await page.getByRole('button', { name: '矢量素材', exact: true }).click();
    const vectorPanel = page.getByRole('complementary', { name: 'BioIcons 矢量素材', exact: true });
    await vectorPanel.getByRole('textbox', { name: 'BioIcons 搜索词', exact: true }).fill('receptor');
    await vectorPanel.getByRole('button', { name: '搜索', exact: true }).click();
    const vectorCard = vectorPanel.locator('article').first();
    await vectorCard.getByRole('button', { name: '预览矢量素材', exact: true }).click({ timeout: 60000 });
    await vectorCard.getByRole('img').waitFor({ timeout: 60000 });
    await vectorCard.getByRole('button', { name: '插入矢量素材', exact: true }).click();
    await until(async () => (await source()).elements.some(e => e.type === 'image' && e.customData?.source === 'BioIcons'), 'insert live BioIcons SVG');
    const withVector = await source(); const vectorImage = withVector.elements.find(e => e.type === 'image' && e.customData?.source === 'BioIcons');
    assert.ok(vectorImage.customData.asset.license); assert.ok(vectorImage.customData.asset.author); assert.equal(vectorImage.customData.sha256.length, 64); assert.equal(vectorImage.customData.source_sha256.length, 64);
    for (const box of beforeVector.elements.filter(e => ['rectangle', 'ellipse', 'frame', 'text'].includes(e.type))) assert.ok(vectorImage.x + vectorImage.width <= box.x || vectorImage.x >= box.x + box.width || vectorImage.y + vectorImage.height <= box.y || vectorImage.y >= box.y + box.height, 'insert vector into whitespace without obscuring existing objects');
    assert.equal(createHash('sha256').update(Buffer.from(withVector.files[vectorImage.fileId].dataURL.split(',')[1], 'base64')).digest('hex'), vectorImage.customData.sha256, 'editor preserves content-addressed SVG bytes at insertion');
    assert.equal(withVector.files[vectorImage.fileId].mimeType, 'image/svg+xml'); assert.ok(withVector.files[vectorImage.fileId].dataURL.startsWith('data:image/svg+xml;base64,'));
    for (const element of beforeVector.elements) assert.deepEqual(untouched(withVector.elements.find(e => e.id === element.id)), untouched(element), 'vector insertion preserves existing objects');
    await page.getByRole('button', { name: '保存画布', exact: true }).click();
    await until(async () => (await call('GET', panelPath)).scene.document.files[vectorImage.fileId]?.mimeType === 'image/svg+xml', 'save vector source');
    await page.getByRole('button', { name: '素材编辑', exact: true }).click();
    const paletteDialog = page.getByRole('dialog', { name: '矢量素材编辑', exact: true });
    await paletteDialog.getByRole('img', { name: '配色前的矢量素材', exact: true }).waitFor();
    const colorHead = await call('GET', panelPath);
    const sourceColor = vectorImage.customData.colors.find(c => c !== '#000000'); assert.ok(sourceColor);
    const targetColor = sourceColor === '#22aa66' ? '#3344cc' : '#22aa66';
    const colorsRequest = { element_id: vectorImage.id, expected_revision: colorHead.metadata.revision, colors: { [sourceColor]: targetColor } };
    const staleColor = await fetch(`${backend}${panelPath}/vector-colors`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ ...colorsRequest, expected_revision: colorHead.metadata.revision - 1 }) }); assert.equal(staleColor.status, 409);
    const invalidColor = await fetch(`${backend}${panelPath}/vector-colors`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ ...colorsRequest, colors: { '#nothex': targetColor } }) }); assert.equal(invalidColor.status, 400);
    assert.equal((await call('GET', `${panelPath}/proposals`)).filter(p => p.status === 'pending').length, 0);
    await paletteDialog.getByLabel(`替换颜色 ${sourceColor}`, { exact: true }).evaluate((input, color) => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, color);
      input.dispatchEvent(new Event('input', { bubbles: true })); input.dispatchEvent(new Event('change', { bubbles: true }));
    }, targetColor);
    await paletteDialog.getByRole('button', { name: '预览配色提案', exact: true }).click();
    await panelPreview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
    const colorProposals = (await call('GET', `${panelPath}/proposals`)).filter(p => p.status === 'pending'); assert.equal(colorProposals.length, 1);
    const colorProposal = colorProposals[0]; assert.equal((await call('GET', panelPath)).metadata.revision, colorHead.metadata.revision);
    for (const element of colorHead.scene.document.elements.filter(e => e.id !== vectorImage.id)) assert.deepEqual(colorProposal.document.elements.find(e => e.id === element.id), element, 'palette proposal preserves unrelated objects');
    const coloredImage = colorProposal.document.elements.find(e => e.id === vectorImage.id); assert.ok(coloredImage.customData.colors.includes(targetColor));
    for (const key of ['x', 'y', 'width', 'height', 'angle', 'groupIds', 'frameId', 'scale', 'crop']) assert.deepEqual(coloredImage[key], vectorImage[key]);
    assert.equal(coloredImage.customData.source_sha256, vectorImage.customData.source_sha256); assert.deepEqual(coloredImage.customData.asset, vectorImage.customData.asset);
    assert.notEqual(coloredImage.fileId, vectorImage.fileId); assert.equal(colorProposal.document.files[vectorImage.fileId].dataURL, withVector.files[vectorImage.fileId].dataURL);
    await page.screenshot({ path: '/tmp/autonomics-bioicons-palette-proposal.png' });
    await panelPreview.getByRole('button', { name: '采用提案', exact: true }).click(); await panelPreview.waitFor({ state: 'hidden' });
    await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
    const coloredHead = await call('GET', panelPath); assert.equal(coloredHead.metadata.revision, colorHead.metadata.revision + 1);
    assert.ok(coloredHead.scene.document.elements.find(e => e.id === vectorImage.id).customData.modifications.some(m => m.kind === 'recolor'));

    await page.getByRole('button', { name: '重新加载服务端版本', exact: true }).click();
    await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
    const vectorReloaded = await source(); assert.deepEqual(vectorReloaded.files[coloredImage.fileId].dataURL, colorProposal.document.files[coloredImage.fileId].dataURL);
    const originalColorRevision = await call('GET', `${panelPath}/revisions/${colorHead.metadata.revision}`);
    assert.equal(originalColorRevision.document.files[vectorImage.fileId].dataURL, withVector.files[vectorImage.fileId].dataURL, 'immutable history retains the original vector colour');
    assert.equal(vectorReloaded.elements.find(e => e.id === vectorImage.id).fileId, coloredImage.fileId);
    assert.equal(createHash('sha256').update(Buffer.from(vectorReloaded.files[coloredImage.fileId].dataURL.split(',')[1], 'base64')).digest('hex'), coloredImage.customData.sha256, 'reloaded SVG bytes agree with recorded hash');
    assert.ok(Buffer.from(vectorReloaded.files[coloredImage.fileId].dataURL.split(',')[1], 'base64').toString('utf8').includes(targetColor));
    const vectorDownload = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 SVG', exact: true }).click();
    const vectorSvg = await readFile(await (await vectorDownload).path(), 'utf8'); assert.ok(vectorSvg.includes('data:image/svg+xml;base64,'), 'figure SVG preserves the vector image rather than baking it into PNG');
    const embeddedSources = await page.evaluate(text => {
      const document = new DOMParser().parseFromString(text, 'image/svg+xml');
      return JSON.parse(document.querySelector('#autonomics-asset-sources').textContent);
    }, vectorSvg);
    assert.equal(embeddedSources.schema, 'autonomics.figure.asset-sources/v1'); assert.equal(embeddedSources.assets.length, 1);
    assert.equal(embeddedSources.assets[0].element_id, vectorImage.id); assert.equal(embeddedSources.assets[0].recorded_sha256, coloredImage.customData.sha256);
    assert.equal(embeddedSources.assets[0].actual_sha256, coloredImage.customData.sha256); assert.equal(embeddedSources.assets[0].hash_matches_recorded, true);
    assert.ok(embeddedSources.assets[0].authors.includes(vectorImage.customData.asset.author)); assert.ok(embeddedSources.assets[0].licenses.includes(vectorImage.customData.asset.license));
    assert.ok(embeddedSources.assets[0].modifications.some(change => change.kind === 'recolor'));
    const pngDownload = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 PNG', exact: true }).click();
    const pngBytes = await readFile(await (await pngDownload).path()); let pngOffset = 8, pngSources;
    while (pngOffset + 12 <= pngBytes.length) { const length = pngBytes.readUInt32BE(pngOffset), type = pngBytes.subarray(pngOffset + 4, pngOffset + 8).toString();
      if (type === 'iTXt') { const data = pngBytes.subarray(pngOffset + 8, pngOffset + 8 + length); if (data.subarray(0, 'autonomics.asset-sources'.length).toString() === 'autonomics.asset-sources') pngSources = JSON.parse(data.subarray('autonomics.asset-sources'.length + 5).toString('utf8')); }
      pngOffset += length + 12;
    }
    assert.ok(pngSources); assert.deepEqual(pngSources.assets, embeddedSources.assets);
    assert.ok(await page.evaluate(async data => { const blob = new Blob([Uint8Array.from(atob(data), c => c.charCodeAt(0))], { type: 'image/png' }); const url = URL.createObjectURL(blob); try { const image = new Image(); image.src = url; await image.decode(); return image.naturalWidth > 0 && image.naturalHeight > 0; } finally { URL.revokeObjectURL(url); } }, pngBytes.toString('base64')), 'PNG metadata preserves browser image decoding');
    const savedTitle = await page.getByRole('textbox', { name: '画布名称', exact: true }).inputValue();
    await page.getByRole('textbox', { name: '画布名称', exact: true }).fill('Unsaved attribution snapshot');
    const sourceDownload = page.waitForEvent('download'); await page.getByRole('button', { name: '导出素材来源', exact: true }).click();
    const sourceManifest = JSON.parse(await readFile(await (await sourceDownload).path(), 'utf8'));
    assert.equal(sourceManifest.title, 'Unsaved attribution snapshot'); assert.equal(sourceManifest.unsaved_changes, true);
    assert.deepEqual(sourceManifest.assets, embeddedSources.assets); assert.equal(sourceManifest.based_on_revision, coloredHead.metadata.revision);
    assert.equal((await call('GET', panelPath)).metadata.title, savedTitle, 'source export does not save an unfinished edit');
    await page.getByRole('textbox', { name: '画布名称', exact: true }).fill(savedTitle);
    assert.ok(await page.evaluate(async () => {
      const { embedSvgSources } = await import('/src/features/canvas/canvas-source-manifest.ts');
      const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      const manifest = { title: '</metadata><script>throw new Error("injected")</script>' };
      embedSvgSources(svg, manifest);
      const parsed = new DOMParser().parseFromString(svg.outerHTML, 'image/svg+xml');
      return parsed.querySelectorAll('script').length === 0 && JSON.parse(parsed.querySelector('metadata').textContent).title === manifest.title;
    }), 'source metadata is text rather than executable SVG markup');
    await page.getByRole('button', { name: 'Agent 绘图', exact: true }).click();
    await page.getByRole('textbox', { name: '绘图需求', exact: true }).fill('Review the sources of this figure without editing it.');
    await page.getByRole('button', { name: '交给 Agent', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('textarea[placeholder="输入消息…"]')?.value.includes('Review the sources of this figure without editing it.'));
    const sourceDraft = await page.getByPlaceholder('输入消息…').inputValue();
    const sourceContext = JSON.parse(sourceDraft.slice(sourceDraft.lastIndexOf('{\n  "project_id"')));
    assert.equal(sourceContext.asset_sources.total, 1); assert.equal(sourceContext.asset_sources.assets[0].actual_sha256, coloredImage.customData.sha256);
    assert.ok(sourceContext.asset_sources.assets[0].authors.includes(vectorImage.customData.asset.author));
    assert.equal(sourceContext.asset_sources.assets[0].mime_type, 'image/svg+xml');
    await page.getByRole('tab', { name: savedTitle, exact: true }).click();
    const surface = page.locator('[data-canvas-surface] .excalidraw:visible');
    const surfaceBox = await surface.boundingBox();
    await page.mouse.click(surfaceBox.x + surfaceBox.width * .8, surfaceBox.y + surfaceBox.height * .8);
    await page.keyboard.press('v'); await page.keyboard.press('Control+a');
    await page.getByRole('button', { name: '素材编辑', exact: true }).click();
    await paletteDialog.getByRole('button', { name: '读取内部路径', exact: true }).click();
    await paletteDialog.getByRole('combobox', { name: '素材路径', exact: true }).waitFor();
    const partHead = await call('GET', panelPath);
    const partInfo = await call('GET', `${panelPath}/vector-parts?element_id=${vectorImage.id}&expected_revision=${partHead.metadata.revision}`);
    assert.ok(partInfo.parts.length > 1);
    const chosen = partInfo.parts.find(part => part.stroke !== 'none'); assert.ok(chosen);
    await paletteDialog.getByRole('combobox', { name: '素材路径', exact: true }).selectOption(String(chosen.index - 1));
    await paletteDialog.getByRole('checkbox', { name: '修改路径描边', exact: true }).check();
    await paletteDialog.getByLabel('路径描边颜色', { exact: true }).evaluate(input => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, '#8844aa');
      input.dispatchEvent(new Event('input', { bubbles: true })); input.dispatchEvent(new Event('change', { bubbles: true }));
    });
    const handle = paletteDialog.locator('[aria-label="拖动路径平移"]');
    const handleBox = await handle.boundingBox();
    const rootDelta = await handle.evaluate(element => { const inverse = element.ownerSVGElement.getScreenCTM().inverse(); return [inverse.a * 9 + inverse.c * 5, inverse.b * 9 + inverse.d * 5]; });
    await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2); await page.mouse.down();
    await page.mouse.move(handleBox.x + handleBox.width / 2 + 9, handleBox.y + handleBox.height / 2 + 5, { steps: 6 }); await page.mouse.up();
    assert.ok(Math.abs(Number(await paletteDialog.getByLabel('路径平移 X', { exact: true }).inputValue()) - rootDelta[0]) < .01);
    assert.ok(Math.abs(Number(await paletteDialog.getByLabel('路径平移 Y', { exact: true }).inputValue()) - rootDelta[1]) < .01);
    await paletteDialog.getByLabel('路径平移 X', { exact: true }).fill('12');
    await paletteDialog.getByLabel('路径平移 Y', { exact: true }).fill('-8');
    await page.screenshot({ path: '/tmp/autonomics-bioicons-part-selection.png' });
    const badPart = await fetch(`${backend}${panelPath}/vector-parts`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ element_id: vectorImage.id, expected_revision: partHead.metadata.revision, sha256: 'stale-hash', edits: [{ part_id: chosen.part_id, stroke: '#8844aa' }] }) });
    assert.equal(badPart.status, 400);
    await paletteDialog.getByRole('button', { name: '预览路径修改提案', exact: true }).click();
    await panelPreview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
    const partProposals = (await call('GET', `${panelPath}/proposals`)).filter(item => item.status === 'pending'); assert.equal(partProposals.length, 1);
    assert.equal((await call('GET', panelPath)).metadata.revision, partHead.metadata.revision);
    for (const element of partHead.scene.document.elements.filter(e => e.id !== vectorImage.id)) assert.deepEqual(partProposals[0].document.elements.find(e => e.id === element.id), element);
    await panelPreview.getByRole('button', { name: '采用提案', exact: true }).click(); await panelPreview.waitFor({ state: 'hidden' });
    await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
    const updatedPartHead = await call('GET', panelPath);
    const updatedInfo = await call('GET', `${panelPath}/vector-parts?element_id=${vectorImage.id}&expected_revision=${updatedPartHead.metadata.revision}`);
    assert.equal(updatedInfo.parts.length, partInfo.parts.length);
    for (let index = 0; index < partInfo.parts.length; index++) {
      const before = partInfo.parts[index], after = updatedInfo.parts[index];
      assert.equal(after.stroke, before.index === chosen.index ? '#8844aa' : before.stroke);
      assert.equal(after.fill, before.fill);
      for (let axis = 0; axis < 4; axis++) assert.ok(Math.abs(after.bounds[axis] - before.bounds[axis] - (before.index === chosen.index && axis < 2 ? [12, -8][axis] : 0)) < .001);
    }
    await page.getByRole('button', { name: '重新加载服务端版本', exact: true }).click();
    const partReloaded = await source(); const partImage = partReloaded.elements.find(e => e.id === vectorImage.id);
    assert.equal(createHash('sha256').update(Buffer.from(partReloaded.files[partImage.fileId].dataURL.split(',')[1], 'base64')).digest('hex'), partImage.customData.sha256);
    const partSourcesDownload = page.waitForEvent('download'); await page.getByRole('button', { name: '导出素材来源', exact: true }).click();
    const partManifest = JSON.parse(await readFile(await (await partSourcesDownload).path(), 'utf8'));
    const styleChange = partManifest.assets[0].modifications.findLast(change => change.kind === 'path_edit');
    assert.equal(styleChange.edits[0].part_id, chosen.part_id); assert.equal(styleChange.edits[0].stroke, '#8844aa'); assert.deepEqual(styleChange.edits[0].translate, [12, -8]);
    assert.deepEqual(partManifest.assets[0].authors, embeddedSources.assets[0].authors);
    console.log('canvas-native: per-path style selection, SVG-coordinate dragging and translation, isolated proposal, hash guard, adoption, reload and attribution export passed');
    const geometrySurface = await surface.boundingBox();
    await page.mouse.click(geometrySurface.x + geometrySurface.width * .8, geometrySurface.y + geometrySurface.height * .8);
    await page.keyboard.press('v'); await page.keyboard.press('Control+a');
    await page.getByRole('button', { name: '素材编辑', exact: true }).click();
    await paletteDialog.getByRole('button', { name: '读取内部路径', exact: true }).click();
    await paletteDialog.getByRole('combobox', { name: '素材路径', exact: true }).waitFor();
    const geometryHead = await call('GET', panelPath);
    const geometryParts = await call('GET', `${panelPath}/vector-parts?element_id=${vectorImage.id}&expected_revision=${geometryHead.metadata.revision}`);
    let curve, curveGeometry;
    for (const candidate of geometryParts.parts.filter(part => part.geometry_supported && part.point_count > 4)) {
      const value = await call('GET', `${panelPath}/vector-geometry?${new URLSearchParams({ element_id: vectorImage.id, expected_revision: String(geometryHead.metadata.revision), sha256: geometryParts.sha256, part_id: candidate.part_id })}`);
      if (value.points.some(point => point.kind === 'control')) { curve = candidate; curveGeometry = value; break; }
    }
    assert.ok(curve && curveGeometry);
    await paletteDialog.getByRole('combobox', { name: '素材路径', exact: true }).selectOption(String(curve.index - 1));
    await paletteDialog.getByRole('button', { name: '读取端点与控制点', exact: true }).click();
    const control = curveGeometry.points.find(point => point.kind === 'control');
    await paletteDialog.getByRole('combobox', { name: '路径点编号', exact: true }).selectOption(String(control.index));
    const controlHandle = paletteDialog.locator('[aria-label="拖动选中控制点"]'); await controlHandle.scrollIntoViewIfNeeded();
    const controlBox = await controlHandle.boundingBox();
    const controlDelta = await controlHandle.evaluate(element => { const inverse = element.ownerSVGElement.getScreenCTM().inverse(); return [inverse.a * 7 + inverse.c * 4, inverse.b * 7 + inverse.d * 4]; });
    await page.mouse.move(controlBox.x + controlBox.width / 2, controlBox.y + controlBox.height / 2); await page.mouse.down();
    await page.mouse.move(controlBox.x + controlBox.width / 2 + 7, controlBox.y + controlBox.height / 2 + 4, { steps: 6 }); await page.mouse.up();
    assert.ok(Math.abs(Number(await paletteDialog.getByLabel('路径点 X', { exact: true }).inputValue()) - control.position[0] - controlDelta[0]) < .01);
    assert.ok(Math.abs(Number(await paletteDialog.getByLabel('路径点 Y', { exact: true }).inputValue()) - control.position[1] - controlDelta[1]) < .01);
    const controlPosition = [control.position[0] + 3, control.position[1] - 2];
    await paletteDialog.getByLabel('路径点 X', { exact: true }).fill(String(controlPosition[0]));
    await paletteDialog.getByLabel('路径点 Y', { exact: true }).fill(String(controlPosition[1]));
    assert.equal(await paletteDialog.getByRole('checkbox', { name: '平移路径', exact: true }).isEnabled(), false);
    await page.screenshot({ path: '/tmp/autonomics-bioicons-control-point.png' });
    await paletteDialog.getByRole('button', { name: '预览路径修改提案', exact: true }).click();
    await panelPreview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
    const pointCandidates = (await call('GET', `${panelPath}/proposals`)).filter(item => item.status === 'pending'); assert.equal(pointCandidates.length, 1);
    for (const element of geometryHead.scene.document.elements.filter(e => e.id !== vectorImage.id)) assert.deepEqual(pointCandidates[0].document.elements.find(e => e.id === element.id), element);
    assert.equal((await call('GET', panelPath)).metadata.revision, geometryHead.metadata.revision);
    await panelPreview.getByRole('button', { name: '采用提案', exact: true }).click(); await panelPreview.waitFor({ state: 'hidden' });
    await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
    const pointHead = await call('GET', panelPath);
    const newParts = await call('GET', `${panelPath}/vector-parts?element_id=${vectorImage.id}&expected_revision=${pointHead.metadata.revision}`);
    const newCurve = newParts.parts.find(part => part.index === curve.index);
    assert.equal(newParts.parts.length, geometryParts.parts.length);
    const newGeometry = await call('GET', `${panelPath}/vector-geometry?${new URLSearchParams({ element_id: vectorImage.id, expected_revision: String(pointHead.metadata.revision), sha256: newParts.sha256, part_id: newCurve.part_id })}`);
    assert.deepEqual(newGeometry.commands, curveGeometry.commands);
    for (const point of newGeometry.points) { const expected = point.index === control.index ? controlPosition : curveGeometry.points[point.index].position; for (let axis = 0; axis < 2; axis++) assert.ok(Math.abs(point.position[axis] - expected[axis]) < .001); }
    for (const part of newParts.parts.filter(part => part.index !== curve.index)) assert.deepEqual(part.bounds, geometryParts.parts[part.index - 1].bounds);
    await page.getByRole('button', { name: '重新加载服务端版本', exact: true }).click();
    const pointSource = await source(); const pointImage = pointSource.elements.find(e => e.id === vectorImage.id);
    assert.equal(createHash('sha256').update(Buffer.from(pointSource.files[pointImage.fileId].dataURL.split(',')[1], 'base64')).digest('hex'), pointImage.customData.sha256);
    assert.deepEqual(pointImage.customData.modifications.at(-1).edits[0].points, [{ index: control.index, position: controlPosition }]);
    const pointManifestDownload = page.waitForEvent('download'); await page.getByRole('button', { name: '导出素材来源', exact: true }).click();
    const pointManifest = JSON.parse(await readFile(await (await pointManifestDownload).path(), 'utf8'));
    assert.deepEqual(pointManifest.assets[0].modifications.at(-1).edits[0].points, [{ index: control.index, position: controlPosition }]);
    console.log('canvas-native: root-coordinate Bezier control drag, isolated geometry proposal, topology preservation, adoption and reload passed');
    await page.screenshot({ path: '/tmp/autonomics-bioicons-canvas.png' });
    console.log('canvas-native: live BioIcons search, preview, SVG insertion, reviewed recoloring, source manifests in SVG/PNG, agent attribution context, save/reload and vector export passed');
  }
  // More than one page of review history remains accessible in the real UI.
  const historyPng = savedReview.dataURL;
  const currentPanel = await call('GET', panelPath);
  for (let index = 0; index < 55; index++) {
    const item = await call('POST', `${panelPath}/markups`, { expected_revision: currentPanel.metadata.revision, note: `Pagination review ${index}`, strokes: [{ kind: 'cross', points: [[0, 0], [20, 20]] }], dataURL: historyPng });
    if (index >= 52) await call('POST', `${panelPath}/markups/${item.id}/resolve`);
  }
  await page.getByRole('button', { name: '草图标记', exact: true }).click();
  const reviewDialog = page.getByRole('dialog', { name: '草图标记与审阅' });
  await reviewDialog.getByText('Pagination review 54', { exact: true }).waitFor();
  assert.equal(await reviewDialog.getByText('Pagination review 0', { exact: true }).count(), 0);
  await reviewDialog.getByRole('button', { name: '加载更早标记', exact: true }).click();
  await reviewDialog.getByText('Pagination review 0', { exact: true }).waitFor();
  assert.equal(await reviewDialog.getByRole('button', { name: '加载更早标记', exact: true }).count(), 0);
  await reviewDialog.getByRole('combobox', { name: '标记状态', exact: true }).selectOption('resolved');
  await reviewDialog.getByText('Pagination review 0', { exact: true }).waitFor({ state: 'hidden' });
  await reviewDialog.getByText('Pagination review 54', { exact: true }).waitFor();
  assert.equal(await reviewDialog.getByText('Pagination review 0', { exact: true }).count(), 0);
  await reviewDialog.getByRole('combobox', { name: '标记状态', exact: true }).selectOption('open');
  await reviewDialog.getByText('Pagination review 51', { exact: true }).waitFor();
  assert.equal(await reviewDialog.getByText('Pagination review 54', { exact: true }).count(), 0);
  await reviewDialog.getByRole('button', { name: '加载更早标记', exact: true }).click();
  await reviewDialog.getByText('Pagination review 0', { exact: true }).waitFor();
  await reviewDialog.getByRole('button', { name: '关闭标记', exact: true }).click();
  await page.getByRole('button', { name: 'Agent 绘图', exact: true }).click();
  await page.getByRole('textbox', { name: '绘图需求', exact: true }).fill('Review the complete paginated markup history.');
  await page.getByRole('button', { name: '交给 Agent', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('textarea[placeholder="输入消息…"]')?.value.includes('Review the complete paginated markup history.'));
  const historyDraft = await page.getByPlaceholder('输入消息…').inputValue();
  const historyContext = JSON.parse(historyDraft.slice(historyDraft.lastIndexOf('{\n  "project_id"')));
  assert.equal(historyContext.markups.length, 50);
  assert.ok(historyContext.markups.every(item => item.status === 'open'));
  assert.equal(historyContext.markups_pagination.status, 'open');
  assert.ok(historyContext.markups_pagination.next_before);
  const older = await tool({ action: 'list_markups', project_id: project, figure_id: panelFigure.id, status: 'open', before: historyContext.markups_pagination.next_before });
  assert.equal(older.items.length, 2);
  assert.equal(older.next_before, null);
  assert.ok(older.items.some(item => item.note === 'Pagination review 0'));
  console.log('canvas-native: review history pagination, open/resolved filtering and paginated Agent handoff passed');
  assert.ok(fontRequests.some(url => new URL(url).pathname === '/excalidraw/fonts/Liberation/LiberationSans-Regular.woff2'));
  assert.ok(fontRequests.every(url => new URL(url).origin === origin), 'generated diagrams do not fetch remote font assets');
  assert.ok(/data:font\/woff2;base64,/.test(panelSvg), 'standalone SVG embeds its pinned font');
  assert.deepEqual(errors, []);
  console.log('canvas-native: real tools, HTTP, SQLite, review markup, local edits, layouts, scientific planning, clarification, panels and review passed');
} catch (error) {
  await page?.screenshot({ path: "/tmp/autonomics-canvas-native-failure.png" }).catch(() => {});
  throw error;
} finally {
  await browser?.close(); native.kill(); vite.kill();
  await Promise.all([native, vite].map(child => (child.exitCode !== null || child.signalCode !== null) ? Promise.resolve() : new Promise(resolve => child.once('exit', resolve))));
  await rm(directory, { recursive: true, force: true });
}
