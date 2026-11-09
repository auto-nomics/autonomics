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
  const turns = [], vision = [], markupVision = [], errors = [];
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
        for (const review of request.markups || []) {
          const blocks = await tool({ action: 'read_markup', project_id: request.project_id, figure_id: request.figure_id, markup_id: review.id, expected_revision: revision });
          assert.equal(blocks[1].type, 'image'); const caption = JSON.parse(blocks[0].text);
          assert.equal(caption.at_revision, review.at_revision); markupVision.push(caption);
        }
        const input = turns.length === 0 ? {
          action: 'sketch', project_id: request.project_id, figure_id: request.figure_id, expected_revision: revision, title: 'Signal pathway',
          document: { message: 'Conceptual signaling example', audience: request.audience, layout: 'horizontal', nodes: [{ id: 'ligand', label: 'Ligand' }, { id: 'receptor', label: 'Receptor' }, { id: 'response', label: 'Response' }], edges: [{ from: 'ligand', to: 'receptor', label: 'binds' }, { from: 'receptor', to: 'response', label: 'activates' }], assumptions: ['Conceptual test fixture; no biological mechanism is asserted.'] },
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
    assert.equal(await page.locator('.editor-body-occluded [data-canvas-surface]').count(), 1);
    assert.deepEqual(await page.locator('.editor-body-occluded').evaluateAll(bodies => bodies.flatMap(body => [...body.querySelectorAll('*')].filter(element => getComputedStyle(element).visibility === 'visible' && element.getClientRects().length).map(element => element.tagName))), [], 'inactive canvas descendants must remain visually hidden');
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
    { name: 'multilingual', layout: 'vertical', nodes: [{ id: 'a', label: '细胞膜受体复合物与下游转录调控因子之间的相互作用以及细胞内信号传导机制', shape: 'ellipse' }, { id: 'b', label: 'WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW' }], edges: [{ from: 'a', to: 'b', label: '明确提供的调控关系' }] },
  ];
  for (const item of cases) {
    const title = `Layout ${item.name}`;
    const created = await tool({ action: 'sketch', project_id: project, title, document: { message: 'Layout test fixture with explicit conceptual relations', audience: 'Researchers', layout: item.layout, nodes: item.nodes, edges: item.edges, assumptions: ['Layout fixture only; biological meaning requires review.'] } });
    assert.ok(created.id);
    await page.getByRole('button', { name: '刷新画布列表', exact: true }).click();
    await page.getByRole('button', { name: title, exact: true }).click();
    await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
    const scene = await source();
    const boxes = scene.elements.filter(element => ['rectangle', 'ellipse'].includes(element.type));
    const arrows = scene.elements.filter(element => element.type === 'arrow');
    assert.equal(boxes.length, item.nodes.length); assert.equal(arrows.length, item.edges.length);
    assert.equal(boxes[0].customData.scientificPlan.layout, item.layout);
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
    assert.ok((await readFile(await (await exported).path(), 'utf8')).includes('<svg'));
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
  console.log('canvas-native: real tools, HTTP, SQLite, review markup, local edits, layouts, scientific planning, clarification and review passed');
} catch (error) {
  await page?.screenshot({ path: "/tmp/autonomics-canvas-native-failure.png" }).catch(() => {});
  throw error;
} finally {
  await browser?.close(); native.kill(); vite.kill();
  await Promise.all([native, vite].map(child => (child.exitCode !== null || child.signalCode !== null) ? Promise.resolve() : new Promise(resolve => child.once('exit', resolve))));
  await rm(directory, { recursive: true, force: true });
}
