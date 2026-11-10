// Opt-in real Agent + configured provider + tools + SQLite + GUI integration.
// Only the surrounding workbench APIs are fixtures; model responses are never scripted.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, mkdir, writeFile, readFile } from 'node:fs/promises';
import { tmpdir, homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const provider = process.env.VISUALIZATION_LIVE_PROVIDER, model = process.env.VISUALIZATION_LIVE_MODEL;
assert.ok(provider && model, 'set VISUALIZATION_LIVE_PROVIDER and VISUALIZATION_LIVE_MODEL to an existing configured vision model');
const config = process.env.VISUALIZATION_LIVE_CONFIG || join(homedir(), '.autonomics/config.db');
const binary = process.env.VISUALIZATION_LIVE_BIN || fileURLToPath(new URL('../../../target/debug/examples/visualization_live', import.meta.url));
assert.ok(existsSync(binary) && existsSync(config), 'build the visualization_live example and provide a config database');
const directory = process.env.VISUALIZATION_LIVE_OUTPUT || await mkdtemp(join(tmpdir(), 'autonomics-canvas-live-'));
await mkdir(directory, { recursive: true });
const native = spawn(binary, [config, provider, model, join(directory, 'figures.sqlite'), join(directory, 'trace.jsonl')], { stdio: ['ignore', 'pipe', 'pipe'] });
const port = Number(process.env.VISUALIZATION_LIVE_PORT || '1449');
assert.ok(Number.isInteger(port) && port > 1024 && port < 65536, 'choose an unprivileged live test port');
const origin = `http://127.0.0.1:${port}`;
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', viteOutput = '', browser, page; const turns = [], errors = [], checkpoints = [];
native.stdout.on('data', chunk => { output += chunk; process.stdout.write(chunk); }); native.stderr.on('data', chunk => output += chunk);
vite.stdout.on('data', chunk => viteOutput += chunk); vite.stderr.on('data', chunk => viteOutput += chunk);
const until = async (predicate, message, duration = 60000) => { const deadline = Date.now() + duration; while (!await predicate()) { if (Date.now() > deadline) throw new Error(message); await new Promise(resolve => setTimeout(resolve, 100)); } };
const persist = async (name, value) => writeFile(join(directory, name), JSON.stringify(value, null, 2), { mode: 0o600 });
try {
  await until(() => { if (native.exitCode !== null || native.signalCode !== null) throw new Error(output); return output.includes('VISUALIZATION_LIVE_ADDR='); }, 'live host readiness');
  await until(() => { if (vite.exitCode !== null || vite.signalCode !== null) throw new Error(viteOutput); return viteOutput.includes(origin); }, 'Vite readiness');
  const backend = `http://${output.match(/VISUALIZATION_LIVE_ADDR=([^\s]+)/)[1]}`;
  const call = async (method, path, body, timeout = 30000) => {
    const response = await fetch(`${backend}${path}`, { method, headers: body === undefined ? {} : { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(timeout) });
    const value = await response.json(); assert.ok(response.ok, `${response.status} ${JSON.stringify(value)}`); return value;
  };
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  page = await browser.newPage({ viewport: { width: 1600, height: 1000 } }); page.setDefaultTimeout(30000); page.on('pageerror', error => errors.push(error.message));
  await page.route('**/api/session', route => route.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const url = new URL(route.request().url()), path = url.pathname.replace('/api/v1', '');
    if (path.startsWith('/visualization/')) return route.fulfill({ response: await route.fetch({ url: `${backend}${url.pathname}${url.search}` }) });
    if (path === '/events') return new Promise(() => {});
    if (path === '/state') return route.fulfill({ json: { last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: { '/root/alpha': { id: 's1', title: 'Live figure discussion', created_at: 1, last_active: 1 } }, display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false } } });
    if (path.endsWith('/history')) return route.fulfill({ json: { messages: [] } });
    if (path.endsWith('/messages') && route.request().method() === 'POST') {
      try { const result = await call('POST', '/test/turn', { text: route.request().postDataJSON().text }, 250000); turns.push(result); await persist(`turn-${turns.length}.json`, result); return route.fulfill({ json: {} }); }
      catch (error) { errors.push(String(error)); return route.fulfill({ status: 502, json: { error: String(error) } }); }
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
  await page.getByRole('textbox', { name: '新项目名称' }).fill('Live figure evaluation'); await page.getByRole('button', { name: '创建项目', exact: true }).click();
  await page.getByRole('button', { name: '新建画布', exact: true }).click(); await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  await page.getByRole('textbox', { name: '画布名称' }).fill('Live figure');
  const project = await page.getByRole('combobox', { name: '可视化项目' }).inputValue(); const figure = (await call('GET', `/api/v1/visualization/projects/${project}/figures`))[0].id;
  const path = `/api/v1/visualization/projects/${project}/figures/${figure}`;
  const send = async (number, message) => {
    if (message !== undefined) await page.getByPlaceholder('输入消息…').fill(message);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await until(() => turns.length >= number || errors.length, `live turn ${number} completion`, 250000); assert.deepEqual(errors, []);
    console.log(`LIVE_CHECKPOINT turn=${number}`);
  };
  const handoff = async request => {
    await page.getByRole('button', { name: 'Agent 绘图', exact: true }).click();
    await page.getByRole('textbox', { name: '绘图需求' }).fill(request);
    await page.getByRole('button', { name: '交给 Agent', exact: true }).click(); await page.getByPlaceholder('输入消息…').waitFor();
  };
  await handoff('我想画一张受体信号传导示意图，但还没有指定受众、具体受体和需要表达的终点。请先和我澄清图意并保存待讨论的绘图计划。这一轮先提问，不要擅自选择具体受体、分子或通路，也不要先画图。');
  await send(1);
  const first = await call('GET', `${path}/planning`); await persist('plan-stage-1.json', first);
  assert.ok(first && first.status === 'needs_clarification'); assert.ok(first.document.questions.length > 0, 'real model saves focused questions');
  assert.ok(first.document.questions.every(question => !question.answer.trim()), 'model does not answer its own clarification questions');
  assert.equal((await call('GET', `${path}/proposals`)).length, 0); const original = await call('GET', path); assert.equal(original.scene.document.elements.length, 0);
  checkpoints.push('saved unanswered clarification without drawing');
  await send(2, '受众为免疫学研究者。只展示三个抽象组件：“配体 L”“受体 R”“响应 X”，关系为 L→R（结合）和 R→X（激活），水平排列。这是概念示意，不代表已验证的真实通路，不要补入其他分子。请把这些答案记录进已有计划，保留问题和答案历史，并根据这些信息生成待审阅草图，不要直接改正式图稿。');
  const ready = await call('GET', `${path}/planning`); await persist('plan-stage-2.json', ready);
  assert.equal(ready.issues.length, 0); assert.ok(ready.version > first.version);
  for (const question of first.document.questions) assert.ok(ready.document.questions.some(q => q.id === question.id && q.answer.trim()), `preserve and answer ${question.id}`);
  const normalized = text => text.replace(/\s/g, '');
  assert.deepEqual(ready.document.nodes.map(node => normalized(node.label)).sort(), ['受体R', '响应X', '配体L'].sort());
  const labels = Object.fromEntries(ready.document.nodes.map(node => [node.id, normalized(node.label)]));
  assert.deepEqual(ready.document.edges.map(edge => `${labels[edge.from]}→${labels[edge.to]}`).sort(), ['受体R→响应X', '配体L→受体R'].sort());
  assert.ok(ready.document.assumptions.some(item => /概念|conceptual|未验证|not.*validated/i.test(item)), 'record the conceptual status');
  const proposals = (await call('GET', `${path}/proposals`)).filter(p => p.status === 'pending'); assert.equal(proposals.length, 1);
  assert.equal((await call('GET', path)).metadata.revision, original.metadata.revision, 'real model does not directly commit artwork');
  const proposal = proposals[0]; await persist('proposal-stage-2.json', proposal);
  assert.equal(proposal.document.elements.filter(e => e.type === 'arrow').length, 2); assert.equal(proposal.document.elements.filter(e => e.type === 'text').length, 5);
  checkpoints.push('absorbed supplied answers and proposed exactly the supplied nodes and relations');
  await page.getByRole('tab', { name: 'Live figure', exact: true }).click(); await page.getByRole('button', { name: '绘图计划', exact: true }).click();
  const planning = page.getByRole('dialog', { name: '科研绘图计划' }); await planning.getByRole('textbox', { name: '核心图意' }).waitFor();
  await page.screenshot({ path: join(directory, 'planning.png'), animations: 'disabled' }); await planning.getByRole('button', { name: '生成草图提案', exact: true }).click();
  const preview = page.getByRole('dialog', { name: 'Agent 修改预览' }); await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  await page.screenshot({ path: join(directory, 'proposal.png'), animations: 'disabled' }); await preview.getByRole('button', { name: '采用提案', exact: true }).click(); await preview.waitFor({ state: 'hidden' });
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const beforeEdit = await call('GET', path); await persist('adopted-stage-2.json', beforeEdit);
  await handoff('请先查看当前同版本整图预览，把“响应 X”标签改为“细胞响应 X”。只改这一处文字，保留其他所有组件、文字、连线和位置。请提交局部修改提案，不要直接提交正式版本。');
  await send(3);
  assert.ok(turns[2].calls.some(call => call.name === 'visualization' && call.input.action === 'read_preview'), 'real model requests the saved full-canvas vision result');
  const edits = (await call('GET', `${path}/proposals`)).filter(p => p.status === 'pending'); assert.equal(edits.length, 1); const edit = edits[0]; await persist('proposal-stage-3.json', edit);
  const oldElements = beforeEdit.scene.document.elements, newElements = edit.document.elements;
  const changed = newElements.filter(e => JSON.stringify(e) !== JSON.stringify(oldElements.find(old => old.id === e.id)));
  assert.equal(changed.length, 1, 'model preserves every unrelated persisted object'); assert.equal(changed[0].type, 'text'); assert.equal(normalized(changed[0].text), '细胞响应X');
  assert.equal(newElements.length, oldElements.length); assert.equal((await call('GET', path)).metadata.revision, beforeEdit.metadata.revision);
  checkpoints.push('read real canvas preview and proposed only the requested label change');
  await page.getByRole('tab', { name: 'Live figure', exact: true }).click(); await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
  await preview.getByRole('button', { name: new RegExp(edit.summary.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).click();
  await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor(); await page.screenshot({ path: join(directory, 'local-edit.png'), animations: 'disabled' });
  await preview.getByRole('button', { name: '采用提案', exact: true }).click(); await preview.waitFor({ state: 'hidden' }); await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const beforeIllustration = await call('GET', path); await persist('before-bioart.json', beforeIllustration);
  await handoff('请为“受体 R”寻找合适的 NIH BioArt 通用膜受体示意素材。保持概念性，不要把它当作某个特定已验证蛋白或补写机制。先检索并预览图片，核对含义和来源许可；找到合适素材后，用替换提案升级受体占位形状，保留原有标签、连线和其他对象。不要直接插入正式图稿。如果没有合适素材，请解释原因并保留占位形状。');
  await send(4);
  const assetCalls = turns[3].calls.filter(call => call.name === 'bioart');
  assert.ok(assetCalls.some(call => call.input.action === 'search'), 'real agent searches the live NIH catalog');
  const replacement = assetCalls.findLast(call => call.input.action === 'replace'); assert.ok(replacement, 'real agent proposes a visual replacement');
  assert.ok(assetCalls.some(call => call.input.action === 'preview' && call.input.asset_id === replacement.input.asset_id), 'preview the same asset before replacing the placeholder');
  const assetEdits = (await call('GET', `${path}/proposals`)).filter(p => p.status === 'pending'); assert.equal(assetEdits.length, 1);
  const illustrated = assetEdits[0]; await persist('proposal-stage-4.json', illustrated);
  const image = illustrated.document.elements.find(element => element.type === 'image'); assert.ok(image);
  assert.equal(image.customData.asset.id, replacement.input.asset_id); assert.ok(image.customData.asset.licenses.length > 0);
  assert.ok(image.customData.asset.source_url.startsWith('https://bioart.niaid.nih.gov/'));
  assert.ok(illustrated.document.files[image.fileId].dataURL.startsWith('data:image/png;base64,'));
  const receptorLabel = beforeIllustration.scene.document.elements.find(element => element.type === 'text' && normalized(element.text) === '受体R'); assert.ok(receptorLabel);
  const receptor = beforeIllustration.scene.document.elements.find(element => element.id === receptorLabel.containerId); assert.ok(receptor);
  for (const element of beforeIllustration.scene.document.elements.filter(e => ![receptor.id, receptorLabel.id].includes(e.id))) {
    assert.deepEqual(illustrated.document.elements.find(next => next.id === element.id), element, `asset replacement preserves unrelated ${element.id}`);
  }
  assert.equal(normalized(illustrated.document.elements.find(e => e.id === receptorLabel.id).text), '受体R');
  assert.equal((await call('GET', path)).metadata.revision, beforeIllustration.metadata.revision);
  checkpoints.push('searched and visually previewed a live licensed NIH asset, then proposed a localized replacement');
  await page.getByRole('tab', { name: 'Live figure', exact: true }).click(); await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
  await preview.getByRole('button', { name: new RegExp(illustrated.summary.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).click();
  await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor(); await page.screenshot({ path: join(directory, 'bioart-proposal.png'), animations: 'disabled' });
  await preview.getByRole('button', { name: '采用提案', exact: true }).click(); await preview.waitFor({ state: 'hidden' }); await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const beforeMarkup = await call('GET', path); await persist('before-markup.json', beforeMarkup);
  await page.screenshot({ path: join(directory, 'illustrated-figure.png'), animations: 'disabled' });
  const beforeElements = beforeMarkup.scene.document.elements.filter(e => !e.isDeleted);
  await page.getByRole('button', { name: '草图标记', exact: true }).click();
  const markup = page.getByRole('dialog', { name: '草图标记与审阅' });
  await markup.getByRole('button', { name: '新建标记', exact: true }).click();
  const ink = markup.getByRole('application', { name: '草图标记区域' }); await ink.waitFor();
  await markup.getByRole('button', { name: '保存草图标记', exact: true }).waitFor();
  await markup.getByRole('button', { name: '划除', exact: true }).click();
  const bounds = await ink.boundingBox();
  // This compact horizontal scene fits at zoom 1. Validate the saved scene-space
  // stroke below so viewport timing or fitting errors cannot produce a false pass.
  const minX = Math.min(...beforeElements.map(e => e.x)), maxX = Math.max(...beforeElements.map(e => e.x + e.width));
  const minY = Math.min(...beforeElements.map(e => e.y)), maxY = Math.max(...beforeElements.map(e => e.y + e.height));
  const currentImage = beforeElements.find(e => e.id === image.id); assert.ok(currentImage);
  const centerX = currentImage.x + currentImage.width / 2, centerY = currentImage.y + currentImage.height / 2;
  const inkX = bounds.x + bounds.width / 2 + centerX - (minX + maxX) / 2;
  const inkY = bounds.y + bounds.height / 2 + centerY - (minY + maxY) / 2;
  await page.mouse.move(inkX - 45, inkY - 65); await page.mouse.down();
  await page.mouse.move(inkX + 45, inkY + 65, { steps: 10 }); await page.mouse.up();
  await markup.getByRole('textbox', { name: '标记修改说明' }).fill('移除划掉的组件及其直接连接和关系文字；保留其余组件及位置，不补画任何新关系。');
  await markup.getByRole('button', { name: '保存草图标记', exact: true }).click();
  await markup.getByRole('img', { name: '已保存的草图标记' }).waitFor();
  const reviews = await call('GET', `${path}/markups`); assert.equal(reviews.length, 1);
  const review = await call('GET', `${path}/markups/${reviews[0].id}`); await persist('markup-stage-5.json', review);
  assert.equal(review.at_revision, beforeMarkup.metadata.revision); assert.equal(review.strokes.length, 1);
  const [a, b] = review.strokes[0].points;
  assert.ok(Math.min(a[0], b[0]) < centerX && Math.max(a[0], b[0]) > centerX && Math.min(a[1], b[1]) < centerY && Math.max(a[1], b[1]) > centerY, 'GUI cross covers the receptor image in scene coordinates');
  await page.screenshot({ path: join(directory, 'markup.png'), animations: 'disabled' });
  await markup.getByRole('button', { name: '关闭标记', exact: true }).click();
  await handoff('请查看本次保存的草图标记截图，结合笔迹和修改说明识别应移除的组件，生成局部修改提案。不要直接改正式图稿，不要推断新的科学关系。');
  await send(5);
  assert.ok(turns[4].calls.some(c => c.name === 'visualization' && c.input.action === 'read_markup' && c.input.markup_id === review.id), 'real agent reads the exact review as a vision result');
  const markupEdits = (await call('GET', `${path}/proposals`)).filter(p => p.status === 'pending'); assert.equal(markupEdits.length, 1);
  const deletion = markupEdits[0]; await persist('proposal-stage-5.json', deletion);
  const kept = beforeElements.filter(e => ![receptor.id, receptorLabel.id, image.id].includes(e.id) && e.type !== 'arrow' && !(e.type === 'text' && beforeElements.some(arrow => arrow.type === 'arrow' && arrow.id === e.containerId)));
  assert.equal(kept.length, 4); assert.equal(deletion.document.elements.length, kept.length);
  const removedIds = new Set(beforeElements.filter(e => !kept.some(k => k.id === e.id)).map(e => e.id));
  for (const element of kept) {
    const expected = structuredClone(element);
    if (expected.boundElements) expected.boundElements = expected.boundElements.filter(binding => !removedIds.has(binding.id));
    assert.deepEqual(deletion.document.elements.find(e => e.id === element.id), expected, `review edit preserves surviving ${element.id}`);
  }
  assert.equal((await call('GET', path)).metadata.revision, beforeMarkup.metadata.revision);
  checkpoints.push('interpreted a visual cross and proposed removing only its component and explicit relationships');
  await page.getByRole('tab', { name: 'Live figure', exact: true }).click(); await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
  await preview.getByRole('button', { name: deletion.summary, exact: true }).click();
  await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  await page.screenshot({ path: join(directory, 'markup-proposal.png'), animations: 'disabled' });
  await preview.getByRole('button', { name: '采用提案', exact: true }).click(); await preview.waitFor({ state: 'hidden' });
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  assert.equal((await call('GET', path)).scene.document.elements.length, 4);
  await page.getByRole('button', { name: '保存画布', exact: true }).waitFor();
  const download = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 SVG', exact: true }).click(); await writeFile(join(directory, 'figure.svg'), await readFile(await (await download).path()));
  await page.screenshot({ path: join(directory, 'figure.png'), animations: 'disabled' }); await persist('final.json', await call('GET', path));
  assert.deepEqual(errors, []); await persist('result.json', { passed: true, provider, model, project, figure, checkpoints, turns: turns.length });
  console.log(`canvas-live: all checks passed; artifacts=${directory}`);
} catch (error) {
  await page?.screenshot({ path: join(directory, 'failure.png'), animations: 'disabled' }).catch(() => {});
  await persist('result.json', { passed: false, provider, model, checkpoints, error: String(error), errors });
  console.error(`canvas-live: failed; artifacts=${directory}`); throw error;
} finally {
  await writeFile(join(directory, 'host.log'), output, { mode: 0o600 }); await browser?.close(); native.kill(); vite.kill();
  await Promise.all([native, vite].map(child => child.exitCode !== null || child.signalCode !== null ? Promise.resolve() : new Promise(resolve => child.once('exit', resolve))));
}
