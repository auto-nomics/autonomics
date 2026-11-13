import {clickCanvasAction} from './helpers/canvas-action.mjs';
import {checkContextDraftSend} from './helpers/context-draft-send.mjs';
import {readCanvasPreparedPrompt} from './helpers/canvas-prepared-context.mjs';
// Canvas editor integration: drawing, images, history, persistence and export.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
const sketch = process.env.CANVAS_SKETCH_SCENE ? JSON.parse(readFileSync(process.env.CANVAS_SKETCH_SCENE, 'utf8')) : null;

const origin = 'http://127.0.0.1:1447';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1447', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser, page;
server.stdout.on('data', chunk => output += chunk); server.stderr.on('data', chunk => output += chunk);
try {
  for (let n = 0; !output.includes(origin); n++) { if (server.exitCode !== null || n > 1200) throw new Error(output); await new Promise(r => setTimeout(r, 50)); }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 } }); page.setDefaultTimeout(30000);
  const errors = []; let spawnedAgent;
  page.on('pageerror', e => { errors.push(e.message); console.error('PAGE ERROR', e.stack); });

  const figures = new Map(); let conflict = false; let proposal; const annotations = []; const previews = new Map(); const revisions = new Map();
  const rememberRevision = () => { const item = figures.get("figure"); revisions.set(item.metadata.revision, { revision: item.metadata.revision, title: item.metadata.title, created_at: new Date().toISOString(), document: structuredClone(item.scene.document) }); };
  await page.route(`${origin}/api/session`, r => r.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '');
    if (path.endsWith('/proposal-summary')) return route.fulfill({json:{current_revision:figures.get("figure")?.metadata.revision ?? 1,total:0,current:0,stale:0,latest:null}});
    if (path === '/events') return;
    if (path.endsWith('/proposal-preview-requests')) return route.fulfill({ json: [] });
    if (path === '/visualization/bioart/search') return route.fulfill({ json: { assets: [{ id: 1, title: 'Test cell', source_url: 'https://bioart.niaid.nih.gov/asset/1', licenses: ['CC0'], creators: ['NIH'] }] } });
    if (path === '/visualization/bioart/1/image') return route.fulfill({ json: { asset: { id: 1, title: 'Test cell', source_url: 'https://bioart.niaid.nih.gov/asset/1', licenses: ['CC0'] }, sha256: 'test-image', file: {}, dataURL: await page.evaluate(() => { const canvas = document.createElement('canvas'); canvas.width = 20; canvas.height = 20; return canvas.toDataURL(); }) } });
    if (path === '/visualization/projects') return route.fulfill({ json: [{ id: 'study', title: 'Study' }] });
    if (path === '/visualization/projects/study/figures') {
      if (route.request().method() === 'GET') return route.fulfill({ json: [...figures.values()].map(item => item.metadata) });
      const body = route.request().postDataJSON(); const metadata = { id: 'figure', project_id: 'study', title: body.title, revision: 1, updated_at: new Date().toISOString() };
      figures.set('figure', { metadata, scene: { document: sketch || body.document } }); rememberRevision(); return route.fulfill({ json: metadata });
    }
    const previewPath = path.match(/^\/visualization\/projects\/study\/figures\/figure\/revisions\/(\d+)\/preview$/);
    if (previewPath) {
      const revision = Number(previewPath[1]);
      if (route.request().method() === 'GET') return route.fulfill({ json: previews.get(revision) });
      if (revision !== figures.get('figure').metadata.revision) return route.fulfill({ status: 409, json: { error: 'document changed' } });
      const { dataURL } = route.request().postDataJSON(); assert.ok(dataURL.startsWith('data:image/png;base64,'));
      const bytes = Buffer.from(dataURL.split(',')[1], 'base64');
      assert.equal(bytes.subarray(1, 4).toString(), 'PNG');
      assert.ok(bytes.readUInt32BE(16) > 0 && bytes.readUInt32BE(16) <= 1280);
      assert.ok(bytes.readUInt32BE(20) > 0 && bytes.readUInt32BE(20) <= 1280);
      previews.set(revision, { revision, dataURL }); return route.fulfill({ json: { revision } });
    }
    if (path === '/visualization/projects/study/figures/figure/markups') return route.fulfill({ json: new URL(route.request().url()).searchParams.get('page') === 'true' ? { items: [], next_before: null } : [] });
    if (path === '/visualization/projects/study/figures/figure/annotations') {
      if (route.request().method() === 'GET') return route.fulfill({ json: annotations });
      const body = route.request().postDataJSON();
      const annotation = { id: 'annotation', element_ids: body.element_ids, note: body.note, status: 'open', at_revision: body.expected_revision };
      annotations.push(annotation); return route.fulfill({ json: annotation });
    }
    if (path === '/visualization/projects/study/figures/figure/revisions') {
      const query = new URL(route.request().url()).searchParams; const before = Number(query.get('before') || Infinity);
      return route.fulfill({ json: [...revisions.values()].filter(item => item.revision < before).sort((a, b) => b.revision - a.revision).slice(0, Number(query.get('limit') || 20)).map(({ document, ...metadata }) => metadata) });
    }
    if (path.match(/^\/visualization\/projects\/study\/figures\/figure\/revisions\/\d+$/)) return route.fulfill({ json: revisions.get(Number(path.split('/').at(-1))) });
    if (path === '/visualization/projects/study/figures/figure/proposal-index' || path === '/visualization/projects/study/figures/figure/proposals') {
      const item = figures.get('figure'); const document = JSON.parse(JSON.stringify(item.scene.document)); document.elements[0].backgroundColor = '#d3f9d8';
      proposal = { id: 'candidate', summary: '突出细胞', status: 'pending', base_revision: item.metadata.revision, document };
      return route.fulfill({ json: path.endsWith('/proposal-index') ? {items:[{id:proposal.id,summary:proposal.summary,base_revision:proposal.base_revision}],next_before:null} : [proposal] });
    }
    if (path === '/visualization/projects/study/figures/figure/proposals/candidate') return route.fulfill({json:proposal});
    if (path === '/visualization/projects/study/figures/figure/proposals/candidate/accept') {
      const item = figures.get('figure');
      if (item.metadata.revision !== proposal.base_revision) return route.fulfill({ status: 409, json: { error: 'document changed' } });
      item.metadata.revision++; item.scene.document = proposal.document; rememberRevision();
      return route.fulfill({ json: item.metadata });
    }
    if (path === '/visualization/projects/study/figures/figure') {
      const item = figures.get('figure');
      if (route.request().method() === 'GET') return route.fulfill({ json: item });
      const body = route.request().postDataJSON();
      if (conflict || body.expected_revision !== item.metadata.revision) return route.fulfill({ status: 409, json: { error: 'document changed; reload before saving' } });
      item.metadata = { ...item.metadata, title: body.title, revision: item.metadata.revision + 1 }; item.scene.document = body.document; rememberRevision();
      return route.fulfill({ json: item.metadata });
    }

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
  await page.goto(origin, { waitUntil: "domcontentloaded", timeout: 60000 });

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
  const waitCount = async expected => {
    const deadline = Date.now() + 10000;
    while (await count() !== expected) { if (Date.now() > deadline) assert.equal(await count(), expected); await page.waitForTimeout(100); }
  };
  if (sketch) {
    await waitCount(sketch.elements.length);
    const downloadPromise = page.waitForEvent('download');
    await clickCanvasAction(page, '导出 源文件');
    const download = await downloadPromise;
    const exported = JSON.parse(readFileSync(await download.path(), 'utf8'));
    assert.equal(exported.elements.filter(element => element.type === 'text').length, 5);
    assert.equal(exported.elements.filter(element => element.type === 'arrow').length, 2);
    assert.ok(exported.elements.find(element => element.id === 'node-0').customData.scientificPlan.message);
    const arrowBefore = exported.elements.find(element => element.id === 'edge-0');
    assert.equal(arrowBefore.startBinding.elementId, 'node-0');
    assert.ok(exported.elements.find(element => element.id === 'edge-label-0').width < arrowBefore.width, 'connector label must not cover the entire arrow');
    const zoom = Number.parseFloat(await surface.locator('.reset-zoom-button').innerText()) / 100;
    const points = exported.elements.flatMap(element => element.type === 'arrow' ? element.points.map(point => [element.x + point[0], element.y + point[1]]) : [[element.x, element.y], [element.x + element.width, element.y + element.height]]);
    const midX = (Math.min(...points.map(p => p[0])) + Math.max(...points.map(p => p[0]))) / 2;
    const midY = (Math.min(...points.map(p => p[1])) + Math.max(...points.map(p => p[1]))) / 2;
    const node = exported.elements.find(element => element.id === 'node-0');
    const centerX = bounds.x + bounds.width / 2 + (node.x + node.width / 2 - midX) * zoom;
    const centerY = bounds.y + bounds.height / 2 + (node.y + node.height * .15 - midY) * zoom;
    await page.mouse.move(centerX, centerY); await page.mouse.down();
    await page.mouse.move(centerX + 60, centerY + 50, { steps: 8 }); await page.mouse.up();
    const movedPromise = page.waitForEvent('download');
    await clickCanvasAction(page, '导出 源文件');
    const moved = JSON.parse(readFileSync(await (await movedPromise).path(), 'utf8'));
    assert.ok(moved.elements.find(element => element.id === 'node-0').x > 40, 'generated node can be moved');
    const arrowAfter = moved.elements.find(element => element.id === 'edge-0');
    assert.notEqual(arrowAfter.x, arrowBefore.x, 'bound arrow follows the moved node');
    assert.equal(arrowAfter.startBinding.elementId, 'node-0');
    await page.screenshot({ path: '/tmp/autonomics-scientific-sketch.png' });
  } else {
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
  await page.waitForFunction(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    return store.allGroups(store.getWorkbenchState().groups).flatMap(([, group]) => group.editors).find(editor => editor.kind === 'canvas')?.state?.imageFileCount === 1;
  });
  await page.getByRole('textbox', { name: '画布名称' }).fill('基础画布测试');
  await page.getByRole('button', { name: '保存画布', exact: true }).click();
  await page.getByRole('button', { name: '基础画布测试', exact: true }).waitFor();
  await page.waitForFunction(() => !document.querySelector('[role="tab"][data-dirty="true"]'));
  const sourcePromise = page.waitForEvent('download');
  await clickCanvasAction(page, '导出 源文件');
  const sourceDownload = await sourcePromise;
  const { readFile } = await import('node:fs/promises');
  const source = JSON.parse(await readFile(await sourceDownload.path(), 'utf8'));
  assert.equal(source.elements.filter(element => !element.isDeleted).length, 2);
  assert.equal(source.elements.find(element => element.type === 'rectangle').roughness, 0, 'default shapes have straight strokes');
  assert.equal(Object.keys(source.files).length, 1, 'source export includes image bytes');
  const svgPromise = page.waitForEvent('download');
  await clickCanvasAction(page, '导出 SVG');
  const svg = await svgPromise;
  assert.ok((await readFile(await svg.path(), 'utf8')).includes('<svg'));
  const pngPromise = page.waitForEvent('download');
  await clickCanvasAction(page, '导出 PNG');
  const png = await pngPromise;
  assert.equal((await readFile(await png.path())).subarray(1, 4).toString(), 'PNG');
  await page.reload();
  await page.getByRole('textbox', { name: '画布名称' }).waitFor();
  await page.waitForFunction(() => document.querySelector('input[aria-label="画布名称"]')?.value === '基础画布测试');
  await waitCount(2); assert.equal(await count(), 2, 'saved objects and images restore after reload');
  await page.getByRole('tab', { name: '基础画布测试', exact: true }).getByRole('button', { name: '关闭 基础画布测试' }).click();
  await page.getByRole('button', { name: '基础画布测试', exact: true }).click();
  await page.getByRole('textbox', { name: '画布名称' }).waitFor();
  await waitCount(2); assert.equal(await count(), 2, 'sidebar reopens a saved canvas');
  await clickCanvasAction(page, 'BioArt 素材');
  await page.getByRole('textbox', { name: 'BioArt 搜索词' }).fill('cell');
  await page.getByRole('button', { name: '搜索', exact: true }).click();
  await page.getByRole('button', { name: '预览图片', exact: true }).click();
  await page.getByRole('img', { name: 'Test cell', exact: true }).waitFor();
  assert.equal(await count(), 2, 'catalogue preview does not modify artwork');
  await page.getByRole('button', { name: '插入图片', exact: true }).click();
  await waitCount(3);
  conflict = true;
  await page.getByRole('textbox', { name: '画布名称' }).fill('未保存修改');
  await page.getByRole('button', { name: '保存画布', exact: true }).click();
  await page.getByRole('alert').filter({ hasText: '保存失败' }).waitFor();
  assert.equal(await page.getByRole('textbox', { name: '画布名称' }).inputValue(), '未保存修改');
  assert.equal(figures.get('figure').metadata.title, '基础画布测试');
  page.once('dialog', dialog => dialog.dismiss());
  await clickCanvasAction(page, '重新加载服务端版本');
  assert.equal(await page.getByRole('textbox', { name: '画布名称' }).inputValue(), '未保存修改');
  page.once('dialog', dialog => dialog.accept());
  await clickCanvasAction(page, '重新加载服务端版本');
  await page.waitForFunction(() => document.querySelector('input[aria-label="画布名称"]')?.value === '基础画布测试');

  conflict = false;
  await clickCanvasAction(page, '查看 Agent 提案');
  const preview = page.getByRole('dialog', { name: 'Agent 修改预览' });
  await preview.getByRole('button', { name: /突出细胞/ }).click();
  await preview.getByRole('region', { name: '修改前画布' }).locator('.excalidraw').waitFor();
  await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  await preview.getByRole('status').filter({ hasText: '修改 1' }).waitFor();
  await page.screenshot({ path: '/tmp/autonomics-proposal-preview.png' });
  await preview.getByRole('button', { name: '采用提案', exact: true }).click();
  await preview.waitFor({ state: 'hidden' });
  await waitCount(2);
  assert.equal(figures.get('figure').scene.document.elements[0].backgroundColor, '#d3f9d8');
  const headBeforeRestore = figures.get('figure').metadata.revision;
  const historical = structuredClone(revisions.get(2).document);
  await clickCanvasAction(page, '历史版本');
  const history = page.getByRole('dialog', { name: '画布历史' });
  await history.getByRole('button', { name: /^v2 ·/ }).click();
  await history.getByRole('region', { name: '历史画布' }).locator('.excalidraw').waitFor();
  await history.getByRole('button', { name: '恢复为新版本', exact: true }).click();
  await history.waitFor({ state: 'hidden' });
  assert.equal(figures.get('figure').metadata.revision, headBeforeRestore + 1);
  assert.deepEqual(figures.get('figure').scene.document, historical);
  assert.deepEqual(revisions.get(2).document, historical, 'restore preserves old revisions');
  await page.locator('[data-canvas-surface] .excalidraw').waitFor();
  await page.getByRole('textbox', { name: '画布名称' }).fill('本地未保存');
  await clickCanvasAction(page, '历史版本');
  await history.getByRole('button', { name: /^v2 ·/ }).click();
  await history.getByRole('button', { name: '恢复为新版本', exact: true }).click();
  await history.getByRole('alert').filter({ hasText: '未保存修改' }).waitFor();
  assert.equal(figures.get('figure').metadata.revision, headBeforeRestore + 1);
  await history.getByRole('button', { name: '关闭历史', exact: true }).click();
  await page.getByRole('textbox', { name: '画布名称' }).fill('基础画布测试');
  await page.locator('[data-canvas-surface] .zoom-in-button').click();
  await clickCanvasAction(page, '框选批注');
  const region = await page.getByRole('application', { name: '框选批注区域' }).boundingBox();
  await page.mouse.move(region.x + 230, region.y + 160); await page.mouse.down();
  await page.mouse.move(region.x + 1000, region.y + 650, { steps: 8 }); await page.mouse.up();
  await page.getByRole('complementary', { name: '画布 Agent' }).waitFor();
  assert.equal(await count(), 2, 'review selection does not add artwork objects');
  await page.getByRole('textbox', { name: '批注意见' }).fill('请放大这个细胞');
  await page.getByRole('button', { name: '批注选中对象', exact: true }).click();
  await page.getByRole('button', { name: '定位对象', exact: true }).waitFor();
  assert.equal(annotations.length, 1);
  assert.equal(annotations[0].element_ids.length, 2);

  await page.getByRole('textbox', { name: '绘图受众' }).fill('免疫学研究者');
  await page.getByRole('textbox', { name: '绘图需求' }).fill('请强调细胞的相互作用');
  await page.getByRole('button', { name: '交给 Agent', exact: true }).click();
  const draft = page.getByPlaceholder('输入消息…');
  await draft.waitFor();
  assert.ok((await draft.inputValue()).includes('请强调细胞的相互作用'));
  assert.equal(await draft.inputValue(),'请强调细胞的相互作用');const preparedPrompt=await readCanvasPreparedPrompt(page);assert.ok(preparedPrompt.includes('请放大这个细胞'));
  assert.ok(preparedPrompt.includes('selected_objects'));
  assert.ok(preparedPrompt.includes('"preview_available": true'));
  assert.ok(previews.has(figures.get('figure').metadata.revision));
  assert.ok(preparedPrompt.includes('"project_id": "study"'));
  assert.ok(preparedPrompt.includes('"observed_revision":'));
  assert.deepEqual(await page.locator('.editor-body-occluded').evaluateAll(bodies => bodies.flatMap(body => [...body.querySelectorAll('*')].filter(element => getComputedStyle(element).visibility === 'visible' && element.getClientRects().length).map(element => element.tagName))), [], 'inactive canvas descendants must remain visually hidden');
  await page.screenshot({ path: '/tmp/autonomics-canvas-editor.png' });
  if(process.env.VISUALIZATION_DRAFT_CONTEXT_SEND==='1')await checkContextDraftSend({page,canvasTitle:'基础画布测试'});
  }
  assert.deepEqual(errors, []);
  console.log('canvas-editor: all assertions passed');
} catch (error) { await page?.screenshot({ path: "/tmp/canvas-failure.png" }).catch(() => {}); throw error; } finally { await browser?.close(); server.kill(); }
