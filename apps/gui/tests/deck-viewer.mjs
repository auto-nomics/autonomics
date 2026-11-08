// Run with npm run test:deck. Install a browser with npx playwright install
// chromium, or set PDF_TEST_CHROMIUM to an existing Chromium executable.
// Covers the deck viewer tab end-to-end against a mock gateway:
//   1. deck payload renders slides on the fixed canvas (cover layout).
//   2. click directives step through ClickPlan states (v-click reveal).
//   3. speaker notes pane appears for the noted slide.
//   4. figure-left layout places the figure slot.
//   5. TOC navigation jumps slides.
//   6. mermaid fences render SVG (lazy module).
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import http from 'node:http';

const MOCK_PORT = 14401;
const VITE_PORT = 1429;
const origin = `http://127.0.0.1:${VITE_PORT}`;

// ── Mock gateway ──

const slides = [
  {
    index: 0,
    raw: '---\ntitle: Mock Deck\n---\n\n# Mock Deck\n\n封面副标题',
    frontmatter: { layout: 'cover', title: 'Mock Deck' },
    content: '# Mock Deck\n\n封面副标题',
    note: '这是封面备注',
    title: 'Mock Deck',
    level: 1,
    images: [],
    clickPlan: null,
    revision: 'rev0',
  },
  {
    index: 1,
    raw: '---\n---\n\n## 方法\n\n<v-click>第一点</v-click>\n\n<v-click>第二点</v-click>\n\n<v-click at="3">第三点</v-click>',
    frontmatter: {},
    content: '## 方法\n\n<span data-deck-click="0">第一点</span>\n\n<span data-deck-click="1">第二点</span>\n\n<span data-deck-click="2" data-deck-at="3">第三点</span>',
    note: null,
    title: '方法',
    level: 2,
    images: [],
    clickPlan: {
      total: 3,
      clicksStart: 0,
      elements: [
        { order: 0, start: 1, end: null, max: 1, delta: 1 },
        { order: 1, start: 2, end: null, max: 2, delta: 1 },
        { order: 2, start: 3, end: null, max: 3, delta: 0 },
      ],
    },
    revision: 'rev1',
  },
  {
    index: 2,
    raw: '---\nlayout: figure-left\nimage: assets://asset-1\ntableCaption: Figure 1. 曲线\n---\n\n## 结果\n\n正文说明',
    frontmatter: { layout: 'figure-left', image: 'assets://asset-1', tableCaption: 'Figure 1. 曲线' },
    content: '## 结果\n\n正文说明',
    note: null,
    title: '结果',
    level: 2,
    images: [],
    clickPlan: null,
    revision: 'rev2',
  },
  {
    index: 3,
    raw: '---\n---\n\n## 流程\n\n```mermaid\ngraph TD; A-->B;\n```\n',
    frontmatter: {},
    content: '## 流程\n\n<div class="deck-mermaid-holder"><pre><code class="language-mermaid">graph TD; A--&gt;B;</code></pre></div>',
    note: null,
    title: '流程',
    level: 2,
    images: [],
    clickPlan: null,
    revision: 'rev3',
  },
];

const deckPayload = {
  deck: {
    id: 'deck-mock',
    title: 'Mock Deck',
    aspectRatio: { width: 16, height: 9 },
    canvasWidth: 980,
    colorSchema: null,
    transition: null,
    exportFilename: null,
    defaults: null,
    features: { katex: false, mermaid: true },
    revision: 'deckrev',
    slideCount: 4,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
  },
  slides,
  assets: [
    {
      assetId: 'asset-1',
      filename: 'fig.png',
      mime: 'image/png',
      size: 100,
      caption: null,
      provenance: null,
      ref: 'assets://asset-1',
    },
  ],
};

let deckList = [
  { id: 'deck-mock', title: 'Mock Deck', slideCount: 4, version: 1, createdAt: '2026-01-01T00:00:00Z', updatedAt: '2026-01-01T00:00:00Z' },
];
let livePayload = null;
function buildLivePayload(title) {
  return {
    deck: { ...deckPayload.deck, id: 'deck-live', title, revision: 'liverev1', slideCount: 6 },
    slides: [{ ...deckPayload.slides[0], index: 0, title, content: `# ${title}\n\n从侧栏新建` }],
    assets: [],
  };
}

const snapshot = {
  last_seq: 0,
  agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }],
  contexts: { '/root/alpha': { id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 } },
  display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
};

const mock = http.createServer((req, res) => {
  const path = new URL(req.url, 'http://mock').pathname;
  const json = (body) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
  if (path === '/api/session') return json({});
  if (path === '/api/v1/state') return json(snapshot);
  if (path === '/api/v1/agents/a1/sessions/s1/history') return json({ messages: [] });
  if (path === '/api/v1/bib/collections/changes') return json({ revision: 0 });
  if (path === '/api/v1/events') {
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
    res.write(': ping\n\n');
    return;
  }
  if (path === '/api/v1/deck/decks/deck-mock') return json(deckPayload);
  if (path === '/api/v1/deck/decks/deck-live') return json(livePayload);
  if (path === '/api/v1/deck/decks' && req.method === 'GET') {
    return json({ decks: deckList });
  }
  if (path === '/api/v1/deck/decks' && req.method === 'POST') {
    let body = '';
    req.on('data', c => { body += c; });
    req.on('end', () => {
      const parsed = JSON.parse(body || '{}');
      const title = parsed.title || '新 deck';
      const now = new Date().toISOString();
      deckList.unshift({ id: 'deck-live', title, slideCount: 6, version: 1, createdAt: now, updatedAt: now });
      livePayload = buildLivePayload(title);
      res.writeHead(201, { 'content-type': 'application/json' });
      res.end(JSON.stringify(livePayload));
    });
    return;
  }
  if (path === '/api/v1/deck/decks/deck-mock/assets/asset-1/raw') {
    // 1x1 PNG
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64');
    res.writeHead(200, { 'content-type': 'image/png' });
    return res.end(png);
  }
  res.writeHead(404, { 'content-type': 'application/json' });
  res.end('{"error":"not mocked"}');
});

const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', String(VITE_PORT), '--strictPort'], {
  stdio: ['ignore', 'pipe', 'pipe'],
  env: { ...process.env, AUTONOMICS_HTTP_API_ADDR: `127.0.0.1:${MOCK_PORT}` },
});
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  mock.listen(MOCK_PORT, '127.0.0.1');
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    if (output.includes(origin)) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));

  // Open the deck tab directly through the app: login → palette command.
  await page.goto(origin);
  await page.getByPlaceholder('输入消息…').waitFor({ state: 'visible', timeout: 15000 });

  // Drive the opener bridge directly (the workbench registered it).
  await page.evaluate(async () => {
    const mod = await import('/src/features/deck/deck-state.ts');
    mod.openDeck('deck-mock', { title: 'Mock Deck' });
  });

  // 1. Cover slide renders on the canvas.
  await page.locator('.deck-canvas .deck-slide[data-slide-no="1"]').waitFor({ state: 'visible', timeout: 15000 });
  await page.waitForTimeout(300);
  const coverText = await page.locator('.deck-canvas .deck-slide[data-slide-no="1"]').innerText();
  assert.ok(coverText.includes('Mock Deck'), `cover text: ${coverText}`);
  const canvasBox = await page.locator('.deck-canvas').boundingBox();
  assert.ok(canvasBox && canvasBox.height > 100, 'canvas sized');

  // 2. Notes pane shows the cover note.
  await page.waitForTimeout(200);
  const notes = await page.locator('.deck-notes-body').innerText().catch(() => '');
  assert.ok(notes.includes('封面备注'), `notes: ${notes}`);

  // 3. Navigate to slide 2 (click demo) via TOC.
  await page.locator('.deck-toc-item').nth(1).click();
  await page.locator('.deck-canvas .deck-slide[data-slide-no="2"]').waitFor({ state: 'visible' });
  await page.waitForTimeout(200);

  const clickBody = page.locator('.deck-canvas .deck-slide[data-slide-no="2"] .deck-slide-body');
  // click 0: everything hidden
  let hiddenCount = await clickBody.locator('.deck-click-hidden').count();
  assert.equal(hiddenCount, 3, `all hidden at click 0, got ${hiddenCount}`);

  // advance: right arrow reveals in order
  await page.keyboard.press('ArrowRight');
  await page.waitForTimeout(120);
  const visible1 = await clickBody.locator('[data-deck-click]:not(.deck-click-hidden)').innerText();
  assert.ok(visible1.includes('第一点'), `first reveal: ${visible1}`);

  await page.keyboard.press('ArrowRight');
  await page.waitForTimeout(120);
  const visibleCount2 = await clickBody.locator('[data-deck-click]:not(.deck-click-hidden)').count();
  assert.equal(visibleCount2, 2, 'two revealed after second click');

  // counter shows click position
  const counter = await page.locator('.deck-counter').innerText();
  assert.ok(counter.includes('2 / 4'), `counter: ${counter}`);
  assert.ok(counter.includes('click'), `counter click: ${counter}`);

  // 4. figure-left layout: figure slot + raw asset URL
  await page.locator('.deck-toc-item').nth(2).click();
  await page.locator('.deck-canvas .deck-slide[data-slide-no="3"]').waitFor({ state: 'visible' });
  await page.waitForTimeout(200);
  const figureSrc = await page.locator('.deck-slide[data-slide-no="3"] .deck-figure-img').getAttribute('src');
  assert.ok(figureSrc && figureSrc.includes('/assets/asset-1/raw'), `figure src: ${figureSrc}`);
  const caption = await page.locator('.deck-slide[data-slide-no="3"] .deck-caption').innerText();
  assert.ok(caption.includes('Figure 1'), `caption: ${caption}`);

  // 5. mermaid slide renders SVG
  await page.locator('.deck-toc-item').nth(3).click();
  await page.locator('.deck-canvas .deck-slide[data-slide-no="4"] .deck-mermaid svg').waitFor({ state: 'visible', timeout: 10000 });

  // 6. live update: agent edits land in the open tab within a poll
  //    interval (4s poll — no reload, navigation preserved)
  await page.locator('.deck-toc-item').nth(0).click();
  await page.locator('.deck-canvas .deck-slide[data-slide-no="1"]').waitFor({ state: 'visible' });
  await page.waitForTimeout(200);
  assert.ok((await page.locator('.deck-canvas .deck-slide[data-slide-no="1"]').innerText()).includes('封面副标题'));

  deckPayload.deck.revision = 'deckrev-live';
  deckPayload.slides[0].content = '# Mock Deck\n\n已实时更新';
  deckPayload.slides[0].raw = '---\ntitle: Mock Deck\n---\n\n# Mock Deck\n\n已实时更新';

  await page.locator('.deck-canvas .deck-slide[data-slide-no="1"]')
    .locator('text=已实时更新')
    .waitFor({ state: 'visible', timeout: 9000 });

  // 7. sidebar: decks container lists decks; 新建 dialog creates and
  //    opens the new deck tab
  await page.locator('button[title="幻灯片(新建/浏览 deck,点开即编辑器标签)"]').click();
  await page.locator('[data-deck-sidebar]').waitFor({ state: 'visible' });
  await page.waitForTimeout(300);
  const railText = await page.locator('[data-deck-sidebar]').innerText();
  assert.ok(railText.includes('Mock Deck'), `sidebar lists deck: ${railText}`);

  await page.locator('[data-deck-sidebar]').getByText('新建', { exact: true }).click();
  await page.getByPlaceholder('deck 标题(如:组会 · 单细胞图谱)').fill('侧栏新 deck');
  await page.getByRole('radio', { name: /空白/ }).click();
  await page.getByRole('button', { name: '创建' }).click();

  // new tab opens with the created deck rendered
  await page.locator('.deck-canvas .deck-slide[data-slide-no="1"]').locator('text=侧栏新 deck').waitFor({ state: 'visible', timeout: 9000 });
  // sidebar refreshed with the new entry
  await page.waitForTimeout(300);
  const railAfter = await page.locator('[data-deck-sidebar]').innerText();
  assert.ok(railAfter.includes('侧栏新 deck'), `sidebar refreshed: ${railAfter}`);

  // no page errors
  assert.deepEqual(errors.filter(e => !e.includes('ResizeObserver')), [], 'no page errors');
  console.log('deck-viewer: all assertions passed');
} finally {
  browser?.close();
  server.kill();
  mock.close();
}
