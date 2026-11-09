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
import { existsSync, readFileSync } from 'node:fs';
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
  outline: [
    { slide: 1, title: 'Mock Deck', level: 1, depth: 0, parent: null, isSection: false, sectionTitle: 'Mock Deck', sectionPath: [], hidden: false },
    { slide: 2, title: '方法', level: 1, depth: 0, parent: null, isSection: true, sectionTitle: '方法', sectionPath: [2], hidden: false },
    { slide: 3, title: '结果', level: 2, depth: 1, parent: 2, isSection: true, sectionTitle: '核心结果', sectionPath: [2, 3], hidden: false },
    { slide: 4, title: '流程', level: 1, depth: 0, parent: null, isSection: true, sectionTitle: '讨论', sectionPath: [4], hidden: false },
    { slide: 5, title: '不显示', level: 2, depth: 1, parent: 4, isSection: false, sectionTitle: '不显示', sectionPath: [4], hidden: true },
  ],
  themeCss: readFileSync('../../crates/ppt-base/src/deck/default-theme.css', 'utf8'),
  themeRevision: 'theme0',
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
let markdownSource = '---\ntitle: Mock Deck\n---\n\n# Mock Deck\n';
let markdownRevision = 'deckrev';
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
  if (path === '/api/v1/deck/decks/deck-mock/theme.css' && req.method === 'PUT') {
    let body = ''; req.on('data', c => { body += c; }); req.on('end', () => {
      const data = JSON.parse(body); deckPayload.themeCss = data.css; deckPayload.themeRevision = 'theme1';
      if (data.css.includes('--deck-width: 1280px')) deckPayload.canvasSize = { width: 1280, height: 720 };
      json({ revision: 'theme1' });
    }); return;
  }
  if (path === '/api/v1/deck/decks/deck-mock/default-theme.css') return json({ css: ':root { --deck-width: 980px; --deck-height: 551px; }\n' + readFileSync('../../crates/ppt-base/src/deck/default-theme.css', 'utf8') });
  if (path === '/api/v1/deck/decks/deck-mock/slides/2/css' && req.method === 'PUT') {
    let body = ''; req.on('data', c => { body += c; }); req.on('end', () => {
      const data = JSON.parse(body); deckPayload.slides[1].frontmatter.slideCss = data.css; deckPayload.deck.revision = 'deckrev-css'; json({ revision: 'deckrev-css' });
    }); return;
  }
  if (path === '/api/v1/deck/decks/deck-mock/print-data') return json({ ...deckPayload, token: 'test' });
  if (path === '/api/v1/deck/decks/deck-mock/source' && req.method === 'GET') return json({ markdown: markdownSource, revision: markdownRevision });
  if (path === '/api/v1/deck/decks/deck-mock/source' && req.method === 'PUT') {
    let body = ''; req.on('data', c => { body += c; }); req.on('end', () => {
      const data = JSON.parse(body); markdownSource = data.markdown; markdownRevision = 'deckrev-source'; deckPayload.deck.revision = markdownRevision; json({ deck: deckPayload.deck });
    }); return;
  }
  if (path === '/api/v1/deck/decks/deck-mock/print-assets/asset-1') {
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64');
    res.writeHead(200, { 'content-type': 'image/png' }); return res.end(png);
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
  async function setCode(language, value) {
    const editor = page.locator(`[data-code-editor="${language}"]:visible`);
    await editor.locator('.monaco-editor textarea.inputarea').focus();
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.insertText(value);
  }
  const errors = [];
  page.on('pageerror', error => { errors.push(error.message); console.error(error.stack); });

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
  assert.equal(await page.locator('.deck-slide').evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(23, 50, 70)', 'default cover uses the biomedical theme');
  assert.equal(await page.locator('.deck-section-nav').isVisible(), false, 'section navigation is opt-in');
  const listStyles = await page.locator('.deck-slide .deck-prose').evaluate(el => {
    const ul = document.createElement('ul'); const ol = document.createElement('ol');
    el.append(ul, ol);
    const result = [getComputedStyle(ul).listStyleType, getComputedStyle(ol).listStyleType];
    ul.remove(); ol.remove(); return result;
  });
  assert.deepEqual(listStyles, ['disc', 'decimal'], 'scientific lists and references retain visible markers');
  const canvasBox = await page.locator('.deck-canvas').boundingBox();
  assert.ok(canvasBox && canvasBox.height > 100, 'canvas sized');
  if (process.env.DECK_UI_SCREENSHOT) await page.screenshot({ path: process.env.DECK_UI_SCREENSHOT });

  // The view switcher exposes a keyboard-navigable tab list.
  await page.getByRole('tab', { name: '预览', exact: true }).focus();
  await page.keyboard.press('ArrowRight');
  assert.equal(await page.getByRole('tab', { name: 'Markdown 源码' }).getAttribute('aria-selected'), 'true');
  await page.keyboard.press('End');
  assert.equal(await page.getByRole('tab', { name: 'CSS 样式' }).getAttribute('aria-selected'), 'true');
  await page.keyboard.press('Home');
  assert.equal(await page.getByRole('tab', { name: '预览', exact: true }).getAttribute('aria-selected'), 'true');

  // 2. Notes pane shows the cover note.
  await page.waitForTimeout(200);
  const notes = await page.locator('.deck-notes-body').innerText().catch(() => '');
  assert.ok(notes.includes('封面备注'), `notes: ${notes}`);

  // Audience fullscreen and a separately synchronized presenter window.
  await page.getByRole('button', { name: '全屏展示' }).click();
  await page.waitForFunction(() => !!document.fullscreenElement);
  assert.equal(await page.locator('.deck-notes').isVisible(), false, 'notes stay private in fullscreen');
  await page.getByRole('button', { name: '退出全屏' }).click();
  await page.waitForFunction(() => !document.fullscreenElement);
  const popupPromise = page.waitForEvent('popup');
  await page.getByRole('button', { name: '打开讲演者模式' }).click();
  const presenter = await popupPromise;
  await presenter.getByText('已连接观众画面').waitFor();
  assert.match(await presenter.locator('.presenter-notes-body').innerText(), /封面备注/);
  await presenter.getByRole('button', { name: '下一动画 / 页' }).click();
  await page.locator('.deck-slide[data-slide-no="2"]').waitFor();
  await presenter.getByText('下一动画', { exact: true }).waitFor();
  await presenter.getByRole('button', { name: '下一动画 / 页' }).click();
  await page.waitForFunction(() => document.querySelector('.deck-counter')?.textContent?.includes('click 1/3'));
  await page.getByRole('button', { name: '上一页' }).click();
  await presenter.waitForFunction(() => document.querySelector('.presenter-current [data-slide-no="2"]'));
  await presenter.getByRole('button', { name: '上一动画 / 页' }).click();
  await page.locator('.deck-slide[data-slide-no="1"]').waitFor();
  await presenter.getByRole('button', { name: '开始', exact: true }).click();
  await presenter.getByRole('button', { name: '暂停', exact: true }).waitFor();
  await presenter.getByRole('button', { name: '重置', exact: true }).click();
  assert.equal(await presenter.getByLabel('计时').innerText(), '00:00:00');
  await presenter.close();

  // CSS can be edited independently and remains scoped on the print surface.
  await page.getByRole('tab', { name: 'CSS 样式' }).click();
  await page.getByRole('button', { name: '载入默认主题' }).click();
  await page.getByText('未保存', { exact: true }).waitFor();
  await page.getByRole('button', { name: '放弃更改并加载最新' }).click();
  await page.getByText('已保存', { exact: true }).waitFor();
  await setCode('css', ':root { --deck-width: 1280px; --deck-height: 720px; } @keyframes fade { from { opacity: .3; } to { opacity: .3; } } @font-face { font-family: Shared; src: local("Arial"); } :scope { background-color: rgb(1, 2, 3); background-image: url("assets://asset-1"); --deck-section-nav-display: flex; --anim: fade; --family: Shared; animation: var(--anim) 1s infinite; font-family: var(--family); } h1 { color: rgb(17, 34, 51); } .deck-prose h2 { color: red; } .deck-editor-toolbar { color: rgb(255, 0, 0); }');
  await page.locator('[data-code-editor="css"] .view-line [class*="mtk"]').first().waitFor();
  await page.waitForFunction(() => new Set(Array.from(document.querySelectorAll('[data-code-editor="css"] .view-line span[class*="mtk"]'), el => el.className)).size > 1);
  await page.getByRole('tab', { name: '预览', exact: true }).click();
  await page.getByRole('tab', { name: 'CSS 样式' }).click();
  await page.waitForFunction(() => document.querySelector('[data-code-editor="css"] .view-lines')?.textContent?.includes('deck-width'));
  await page.getByRole('button', { name: '保存 CSS' }).click();
  await page.getByText('已保存', { exact: true }).waitFor();
  await page.getByLabel('编辑样式').selectOption('2');
  await setCode('css', '@keyframes fade { from { opacity: .8; } to { opacity: .8; } } @font-face { font-family: Shared; src: local("Times New Roman"); } :scope { --deck-section-nav-display: none; background-color: rgb(90, 80, 70); width: 20px !important; } h2 { color: rgb(10, 30, 50); }');
  await page.keyboard.press('ControlOrMeta+S');
  await page.getByText('已保存', { exact: true }).waitFor();
  await page.getByRole('tab', { name: '预览', exact: true }).click();
  assert.equal(await page.locator('.deck-canvas').evaluate(el => el.style.width), '1280px');
  assert.equal(await page.locator('.deck-canvas').evaluate(el => el.style.height), '720px');
  assert.equal((await page.locator('.deck-slide').evaluate(el => getComputedStyle(el).getPropertyValue('--deck-width'))).trim(), '1280px');
  assert.equal(await page.locator('.deck-slide').evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(1, 2, 3)');
  assert.equal(await page.locator('.deck-slide h1').evaluate(el => getComputedStyle(el).color), 'rgb(17, 34, 51)');
  assert.match(await page.locator('.deck-slide').evaluate(el => getComputedStyle(el).backgroundImage), /\/assets\/asset-1\/raw/);
  assert.notEqual(await page.locator('.deck-editor-toolbar').evaluate(el => getComputedStyle(el).color), 'rgb(255, 0, 0)');

  // Markdown uses the same core, with syntax highlighting and keyboard save.
  await page.getByRole('tab', { name: 'Markdown 源码' }).click();
  await setCode('markdown', '# Updated heading\n\n**bold** and `code`\n');
  await page.locator('[data-code-editor="markdown"] .view-line [class*="mtk"]').first().waitFor();
  await page.waitForFunction(() => new Set(Array.from(document.querySelectorAll('[data-code-editor="markdown"] .view-line span[class*="mtk"]'), el => el.className)).size > 1);
  await page.keyboard.press('ControlOrMeta+S');
  await page.waitForFunction(() => !document.querySelector('.deck-editor-toolbar')?.textContent?.includes('未保存'));
  assert.match(markdownSource, /Updated heading/);
  await page.getByRole('tab', { name: '预览', exact: true }).click();

  const printPage = await browser.newPage();
  await printPage.goto(`${origin}/deck/print?deck=deck-mock&token=test&mode=pdf`);
  await printPage.locator('html[data-deck-ready="true"]').waitFor({ timeout: 15000 });
  assert.equal(await printPage.locator('[data-deck-canvas-width]').first().getAttribute('data-deck-canvas-width'), '1280');
  assert.equal(await printPage.locator('[data-deck-canvas-height]').first().getAttribute('data-deck-canvas-height'), '720');
  assert.equal(await printPage.locator('.deck-slide').nth(0).evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(1, 2, 3)');
  assert.equal(await printPage.locator('.deck-slide').nth(1).evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(90, 80, 70)');
  assert.equal(await printPage.locator('.deck-slide').nth(1).locator('.deck-section-nav').isVisible(), false, 'page CSS can hide section navigation');
  assert.equal(await printPage.locator('.deck-slide').nth(1).evaluate(el => el.getBoundingClientRect().width), 1280, 'per-slide CSS cannot change physical page size');
  assert.equal(await printPage.locator('.deck-slide').nth(1).locator('h2').evaluate(el => getComputedStyle(el).color), 'rgb(10, 30, 50)');
  assert.equal(await printPage.locator('.deck-slide').nth(2).evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(1, 2, 3)');
  assert.equal(await printPage.locator('.deck-slide').nth(2).locator('.deck-section-nav-link[data-active="true"]').textContent(), '01方法');
  assert.match(await printPage.locator('.deck-slide').nth(2).locator('.deck-section-breadcrumb').innerText(), /核心结果/);
  await printPage.waitForTimeout(100);
  const opacity = await printPage.locator('.deck-slide').evaluateAll(els => els.slice(0, 3).map(el => getComputedStyle(el).opacity));
  assert.deepEqual(opacity, ['0.3', '0.8', '0.3'], 'per-page keyframes do not collide');
  const fonts = await printPage.locator('.deck-slide').evaluateAll(els => els.slice(0, 2).map(el => getComputedStyle(el).fontFamily));
  assert.notEqual(fonts[0], fonts[1], 'per-page font families do not collide');
  assert.match(await printPage.locator('.deck-slide').nth(0).evaluate(el => getComputedStyle(el).backgroundImage), /\/print-assets\/asset-1\?token=test/);
  await printPage.close();

  const capturePage = await browser.newPage();
  await capturePage.goto(`${origin}/deck/print?deck=deck-mock&token=test&mode=single`);
  await capturePage.locator('html[data-deck-ready="true"]').waitFor({ timeout: 15000 });
  const invalidated = await capturePage.evaluate(() => { window.__deckGoto(2, 0); return document.documentElement.dataset.deckReady; });
  assert.equal(invalidated, 'false', 'navigation invalidates the previous ready state synchronously');
  await capturePage.locator('.deck-slide[data-slide-no="2"]').waitFor();
  await capturePage.waitForFunction(() => document.documentElement.dataset.deckReady === 'true' && window.__deckState?.slide === 2);
  assert.equal(await capturePage.locator('.deck-slide').evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(90, 80, 70)');
  await capturePage.evaluate(() => window.__deckGoto(2, 1));
  await capturePage.waitForFunction(() => document.documentElement.dataset.deckReady === 'true' && window.__deckState?.click === 1);
  await capturePage.evaluate(() => window.__deckGoto(2, 1));
  assert.equal(await capturePage.locator('html').getAttribute('data-deck-ready'), 'true', 'a no-op navigation stays ready');
  await capturePage.evaluate(() => window.__deckGoto(3, 0));
  await capturePage.waitForFunction(() => document.documentElement.dataset.deckReady === 'true' && window.__deckState?.slide === 3);
  assert.equal(await capturePage.locator('.deck-section-nav-link[data-active="true"]').textContent(), '01方法');
  assert.match(await capturePage.locator('.deck-section-breadcrumb').innerText(), /核心结果/);
  await capturePage.close();

  await page.locator('button[title="幻灯片(新建/浏览 deck,点开即编辑器标签)"]').click();
  await page.getByRole('tab', { name: '页面目录' }).click();
  await page.getByRole('button', { name: '折叠 方法' }).focus();
  await page.keyboard.press('Space');
  assert.equal(await page.getByRole('button', { name: '结果', exact: true }).count(), 0, 'collapse hides descendants');
  assert.equal(await page.locator('.deck-slide').getAttribute('data-slide-no'), '1', 'space on an expander does not advance the slide');
  await page.getByRole('button', { name: '展开 方法' }).click();
  assert.equal(await page.getByRole('button', { name: '不显示', exact: true }).count(), 0, 'hideInToc excludes an entry');
  await page.waitForTimeout(200);
  const coverBox = await page.locator('.deck-canvas').boundingBox();

  // 3. Navigate to slide 2 (click demo) via TOC.
  await page.locator('.deck-outline-link').nth(1).click();
  await page.locator('.deck-canvas .deck-slide[data-slide-no="2"]').waitFor({ state: 'visible' });
  await page.waitForTimeout(200);
  const secondBox = await page.locator('.deck-canvas').boundingBox();
  assert.ok(Math.abs(coverBox.width - secondBox.width) < 0.1 && Math.abs(coverBox.height - secondBox.height) < 0.1, 'notes do not change display size');

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
  await page.locator('.deck-outline-link').nth(2).click();
  await page.locator('.deck-canvas .deck-slide[data-slide-no="3"]').waitFor({ state: 'visible' });
  await page.waitForTimeout(200);
  const figureSrc = await page.locator('.deck-slide[data-slide-no="3"] .deck-figure-img').getAttribute('src');
  assert.ok(figureSrc && figureSrc.includes('/assets/asset-1/raw'), `figure src: ${figureSrc}`);
  assert.equal(await page.locator('.deck-section-nav').isVisible(), true, 'theme enables section navigation');
  assert.equal(await page.locator('.deck-section-nav').getAttribute('data-current-section'), '2');
  assert.equal(await page.locator('.deck-section-nav').getAttribute('data-current-subsection'), '3');
  assert.equal(await page.locator('.deck-section-nav-link[data-active="true"]').textContent(), '01方法');
  assert.match(await page.locator('.deck-section-breadcrumb').innerText(), /核心结果/);
  if (process.env.DECK_SECTION_SCREENSHOT) await page.screenshot({ path: process.env.DECK_SECTION_SCREENSHOT });
  const caption = await page.locator('.deck-slide[data-slide-no="3"] .deck-caption').innerText();
  assert.ok(caption.includes('Figure 1'), `caption: ${caption}`);

  // 5. the section bar also navigates the presentation.
  await page.locator('.deck-section-nav-link').filter({ hasText: '讨论' }).click();
  await page.locator('.deck-canvas .deck-slide[data-slide-no="4"] .deck-mermaid svg').waitFor({ state: 'visible', timeout: 10000 });

  // 6. live update: agent edits land in the open tab within a poll
  //    interval (4s poll — no reload, navigation preserved)
  await page.locator('.deck-outline-link').nth(0).click();
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
  await page.getByRole('tab', { name: '演示文稿' }).click();
  await page.locator('[data-deck-sidebar]').waitFor({ state: 'visible' });
  await page.waitForTimeout(300);
  const railText = await page.locator('[data-deck-sidebar]').innerText();
  assert.ok(railText.includes('Mock Deck'), `sidebar lists deck: ${railText}`);

  await page.getByRole('button', { name: '新建幻灯片' }).click();
  await page.getByPlaceholder('deck 标题(如:组会 · 单细胞图谱)').fill('侧栏新 deck');
  await page.getByRole('radio', { name: /空白/ }).click();
  await page.getByRole('button', { name: '创建' }).click();

  // new tab opens with the created deck rendered
  await page.locator('.deck-canvas .deck-slide[data-slide-no="1"]').locator('text=侧栏新 deck').waitFor({ state: 'visible', timeout: 9000 });
  // sidebar refreshed with the new entry
  await page.waitForTimeout(300);
  const railAfter = await page.locator('[data-deck-sidebar]').innerText();
  assert.ok(railAfter.includes('侧栏新 deck'), `sidebar refreshed: ${railAfter}`);

  // The default figure-full layout displays the original asset and caption.
  deckPayload.themeCss = readFileSync('../../crates/ppt-base/src/deck/default-theme.css', 'utf8');
  deckPayload.slides[2].frontmatter.layout = 'figure-full';
  const figurePage = await browser.newPage();
  await figurePage.goto(`${origin}/deck/print?deck=deck-mock&token=test&mode=pdf`);
  await figurePage.locator('html[data-deck-ready="true"]').waitFor({ timeout: 15000 });
  const fullFigure = figurePage.locator('.deck-slide[data-layout="figure-full"] .deck-figure-img');
  await fullFigure.waitFor({ state: 'visible' });
  assert.match(await fullFigure.getAttribute('src'), /print-assets\/asset-1/);
  const figureBox = await fullFigure.boundingBox();
  const slideBox = await figurePage.locator('.deck-slide[data-layout="figure-full"]').boundingBox();
  assert.ok(figureBox.width <= slideBox.width && figureBox.height <= slideBox.height, 'figure stays within its page');
  await figurePage.close();

  // 5. Diagnostics rule engine: synthetic slide with known violations.
  const findings = await page.evaluate(async () => {
    const mod = await import('/src/features/deck/render/diagnostics.ts');
    const stage = document.createElement('div');
    stage.className = 'deck-slide';
    stage.style.cssText = 'position:fixed;left:0;top:0;width:1280px;height:720px;background:#ffffff;color:#17324d;';
    const overflowing = document.createElement('div');
    overflowing.textContent = `wide text ${'overflow '.repeat(400)}`;
    overflowing.style.cssText = 'font-size:9px;white-space:nowrap;';
    stage.appendChild(overflowing);
    const faint = document.createElement('div');
    faint.textContent = 'low contrast text';
    faint.style.cssText = 'color:#bbbbbb;';
    stage.appendChild(faint);
    document.body.appendChild(stage);
    try {
      return mod.diagnoseSlide(stage, { slideNo: 3 });
    } finally {
      stage.remove();
    }
  });
  const rules = findings.map(f => f.rule);
  assert.ok(rules.includes('text-overflow-canvas'), `overflow rule fires: ${JSON.stringify(rules)}`);
  assert.ok(rules.includes('font-too-small'), `font rule fires: ${JSON.stringify(rules)}`);
  assert.ok(rules.includes('low-contrast'), `contrast rule fires: ${JSON.stringify(rules)}`);
  assert.ok(rules.includes('text-density'), `density rule fires: ${JSON.stringify(rules)}`);
  const overflowFinding = findings.find(f => f.rule === 'text-overflow-canvas');
  assert.equal(overflowFinding.severity, 'error');
  assert.ok(overflowFinding.message.includes('Slide 3'), `message names the page: ${overflowFinding.message}`);

  // Independent resource consumers share the same draft and save state.
  const sharedDocument = await page.evaluate(async () => {
    const { getDeckDocument } = await import('/src/features/deck/deck-document.ts');
    const first = getDeckDocument('deck-mock');
    const second = getDeckDocument('deck-mock');
    let firstChanges = 0, secondChanges = 0;
    const releaseFirst = first.subscribe(() => firstChanges++);
    const releaseSecond = second.subscribe(() => secondChanges++);
    try {
      await first.loadSource();
      const original = first.getSnapshot().source;
      first.setSource(`${original}\n\nShared document draft`);
      const dirtyUnload = new Event('beforeunload', { cancelable: true });
      window.dispatchEvent(dirtyUnload);
      const shared = second.getSnapshot().source === first.getSnapshot().source;
      releaseFirst();
      second.setSource(`${original}\n\nEdited after first view closed`);
      await second.saveSource();
      const saved = second.getSnapshot();
      const cleanUnload = new Event('beforeunload', { cancelable: true });
      window.dispatchEvent(cleanUnload);
      return { unloadGuard: dirtyUnload.defaultPrevented && !cleanUnload.defaultPrevented, sameModel: first === second, shared, notifications: firstChanges > 0 && secondChanges > 0, saved: saved.source === saved.savedSource && !saved.saving && !saved.sourceError, remainingView: saved.source.includes('Edited after first view closed') };
    } finally { releaseFirst(); releaseSecond(); }
  });
  assert.deepEqual(sharedDocument, { unloadGuard: true, sameModel: true, shared: true, notifications: true, saved: true, remainingView: true });
  const splitDocument = await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    const id = store.openDeckEditor('deck-mock');
    store.splitEditor();
    const editors = store.allGroups(store.getWorkbenchState().groups).flatMap(([, group]) => group.editors).filter(editor => editor.kind === 'deck' && editor.params.deckId === 'deck-mock');
    const duplicate = editors.find(editor => editor.id !== id);
    const distinct = Boolean(duplicate) && new Set(editors.map(editor => editor.id)).size === editors.length;
    if (duplicate) store.patchEditorState(duplicate.id, { title: 'Second view', slideNo: 2 });
    const independent = store.allGroups(store.getWorkbenchState().groups).flatMap(([, group]) => group.editors).find(editor => editor.id === id)?.state?.title !== 'Second view';
    if (duplicate) store.closeEditor(duplicate.id);
    return { distinct, independent };
  });
  assert.deepEqual(splitDocument, { distinct: true, independent: true }, 'split creates another view with independent navigation state');
  const forcedView = await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    const first = store.openDeckEditor('deck-mock');
    const second = store.openDeckEditor('deck-mock', { forceNew: true, slideNo: 2 });
    const distinct = first !== second;
    store.closeEditor(second);
    return distinct;
  });
  assert.equal(forcedView, true, 'forceNew creates a distinct editor for the same document');
  const sharedViews = await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    store.openDeckEditor('deck-mock');
    store.splitEditor();
    return store.allGroups(store.getWorkbenchState().groups).flatMap(([groupId, group]) => group.editors.filter(editor => editor.kind === 'deck' && editor.params.deckId === 'deck-mock').map(editor => ({ groupId, id: editor.id })));
  });
  const firstView = page.locator(`[data-editor-group="${sharedViews[0].groupId}"]`);
  const secondView = page.locator(`[data-editor-group="${sharedViews[1].groupId}"]`);
  await firstView.getByRole('tab', { name: 'Markdown 源码', exact: true }).click();
  await secondView.getByRole('tab', { name: 'Markdown 源码', exact: true }).click();
  const firstInput = firstView.locator('[data-code-editor="markdown"] .monaco-editor textarea.inputarea');
  const secondInput = secondView.locator('[data-code-editor="markdown"] .monaco-editor textarea.inputarea');
  await firstInput.focus();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.insertText('# Shared split editor fixture');
  await secondView.getByText('Shared split editor fixture', { exact: false }).first().waitFor();
  await secondInput.focus();
  await page.keyboard.press('ControlOrMeta+Z');
  await page.waitForFunction(() => [...document.querySelectorAll('[data-code-editor="markdown"]')].every(editor => !editor.textContent.includes('Shared split editor fixture')));
  await firstInput.focus();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.insertText('# Saved from first split view');
  await firstView.getByRole('button', { name: '保存', exact: true }).click();
  await secondView.getByRole('button', { name: '保存', exact: true }).waitFor();
  await page.waitForFunction(() => [...document.querySelectorAll('.deck-save-btn')].every(button => button.disabled));
  await page.evaluate(async id => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    store.closeEditor(id);
  }, sharedViews[1].id);
  await firstInput.waitFor({ state: 'visible' });
  const restoreFixture = await page.evaluate(async () => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    const first = store.openDeckEditor('deck-mock');
    const second = store.openDeckEditor('deck-mock', { forceNew: true, slideNo: 2 });
    store.patchEditorState(first, { title: 'Restored first view', slideNo: 1 });
    return { first, second };
  });
  await page.reload();
  await page.getByRole('tab', { name: 'Restored first view', exact: true }).waitFor();
  const restoredViews = await page.evaluate(async fixture => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    const editors = store.allGroups(store.getWorkbenchState().groups).flatMap(([, group]) => group.editors);
    const first = editors.find(editor => editor.id === fixture.first);
    const second = editors.find(editor => editor.id === fixture.second);
    store.closeEditor(fixture.first);
    const reopened = store.openDeckEditor('deck-mock');
    return { restored: Boolean(first && second), independentSlides: first?.state?.slideNo === 1 && second?.state?.slideNo === 2, reusedRemaining: reopened === fixture.second };
  }, restoreFixture);
  assert.deepEqual(restoredViews, { restored: true, independentSlides: true, reusedRemaining: true }, 'reload restores distinct views and opening reuses the remaining instance');
  await page.evaluate(async () => {
    const { getDeckDocument } = await import('/src/features/deck/deck-document.ts');
    const model = getDeckDocument('deck-mock');
    await model.loadSource();
    model.setSource('# Unsaved final view');
  });
  page.once('dialog', dialog => dialog.dismiss());
  const cancelledClose = await page.evaluate(async id => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    return store.closeEditor(id);
  }, restoreFixture.second);
  assert.equal(cancelledClose, false, 'cancelling discard retains the final editor');
  const retainedDraft = await page.evaluate(async () => {
    const { getDeckDocument } = await import('/src/features/deck/deck-document.ts');
    return getDeckDocument('deck-mock').getSnapshot().source;
  });
  assert.equal(retainedDraft, '# Unsaved final view');
  page.once('dialog', dialog => dialog.accept());
  const discardedClose = await page.evaluate(async id => {
    const store = await import('/src/app/workbench/workbench-store.ts');
    return store.closeEditor(id);
  }, restoreFixture.second);
  assert.equal(discardedClose, true, 'confirming discard closes the final editor');
  const remainingDirty = await page.evaluate(async () => {
    const { dirtyDocumentResources } = await import('/src/app/workbench/dirty-documents.ts');
    return dirtyDocumentResources();
  });
  assert.ok(!remainingDirty.includes('deck:deck-mock'), 'discard removes the resource unload guard');
  // no page errors
  assert.deepEqual(errors.filter(e => !e.includes('ResizeObserver')), [], 'no page errors');
  console.log('deck-viewer: all assertions passed');
} finally {
  browser?.close();
  server.kill();
  mock.close();
}
