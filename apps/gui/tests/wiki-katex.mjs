// Run with npm run test:katex. Drives the real SPA against a stubbed
// daemon and checks KaTeX math rendering in the wiki markdown pipeline
// (the gateway serves the SPA as text assets, so KaTeX runs in MathML
// output mode and the browser renders <math> natively):
//   - $…$ inline and $$…$$ display math render as .md-math with a
//     KaTeX <math> element carrying the original TeX in <annotation>,
//   - currency text ("$5 and $10") stays plain prose,
//   - the block editor round-trip preserves the raw $$…$$ source.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';

const TOKEN = 'test-token-123';
const SESSION = 'session-abc-123';
const origin = 'http://127.0.0.1:1428';

const state = {
  last_seq: 0,
  active_model_spec: null,
  profiles: [],
  agents: [],
  sessions: {},
  display_settings: {
    collapse_thinking: false,
    collapse_tool_calls: false,
    collapse_tool_results: false,
  },
  model_catalog: { providers: [], models: [], active_model: null },
};

const ENTRY = {
  id: 'entry-1',
  title: '相对论笔记',
  alias: [],
  tags: ['物理'],
  ctime: 1760000000,
  mtime: 1760000100,
  content: [
    '# 相对论笔记',
    '',
    '质能方程的完整形式是 $E = mc^2$,其中 $c$ 为光速。',
    '',
    '## 光速不变原理',
    '',
    '$$',
    'E = mc^2',
    '$$',
    '',
    '这门课学费 $5 and $10 不等,不是公式。',
    '',
    '欧拉公式 \\(e^{i\\pi} + 1 = 0\\) 很美。',
    '',
    '\\[',
    '\\int_0^1 x^2 \\, dx = \\frac{1}{3}',
    '\\]',
    '',
  ].join('\n'),
};
const summary = ({ content: _content, ...rest }) => rest;

const server = spawn(
  process.execPath,
  ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1428', '--strictPort'],
  { stdio: ['ignore', 'pipe', 'pipe'] },
);
let output = '';
server.stdout.on('data', (chunk) => {
  output += chunk;
});
server.stderr.on('data', (chunk) => {
  output += chunk;
});

let browser;
const errors = [];
try {
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    if (output.includes(origin)) break;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  const executablePath =
    process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  page.on('pageerror', (error) => errors.push(error.message));

  const sessions = new Set();
  const hasSession = (request) =>
    (request.headers().cookie || '').includes(`autonomics_session=${SESSION}`) && sessions.has(SESSION);

  await page.route('**/api/session', async (route) => {
    const request = route.request();
    if (request.method() === 'POST') {
      const body = request.postDataJSON();
      if (body?.token !== TOKEN) {
        await route.fulfill({ status: 401, json: { error: 'invalid token' } });
        return;
      }
      sessions.add(SESSION);
      await route.fulfill({
        status: 200,
        json: { authenticated: true, expires_in_secs: 43200 },
        headers: {
          'set-cookie': `autonomics_session=${SESSION}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200`,
        },
      });
      return;
    }
    await route.fulfill(
      hasSession(request)
        ? { status: 200, json: { authenticated: true, expires_in_secs: 43200 } }
        : { status: 401, json: { authenticated: false } },
    );
  });

  // Catch-all first, wiki specifics after — routes match in reverse
  // registration order.
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (!hasSession(request)) {
      await route.fulfill({ status: 401, json: { error: 'missing or invalid credentials' } });
      return;
    }
    if (path.endsWith('/state')) {
      await route.fulfill({ json: state });
      return;
    }
    if (path.endsWith('/profiles')) {
      await route.fulfill({ json: [] });
      return;
    }
    if (path.endsWith('/events')) {
      await new Promise(() => {});
      return;
    }
    await route.fulfill({ json: {} });
  });

  await page.route('**/api/v1/wiki/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const title = url.searchParams.get('title');
    if (path === '/api/v1/wiki/entries' && request.method() === 'GET') {
      if (title !== null) {
        await route.fulfill({ json: { entries: [] } });
        return;
      }
      await route.fulfill({ json: { entries: [summary(ENTRY)] } });
      return;
    }
    if (path === '/api/v1/wiki/entries/entry-1' && request.method() === 'GET') {
      await route.fulfill({ json: ENTRY });
      return;
    }
    if (path === '/api/v1/wiki/entries/entry-1/content' && request.method() === 'PUT') {
      await route.fulfill({ json: {} });
      return;
    }
    if (path === '/api/v1/wiki/entries/entry-1/links') {
      await route.fulfill({
        json: { entry: { id: 'entry-1', title: ENTRY.title }, out_links: [], in_links: [] },
      });
      return;
    }
    if (path === '/api/v1/wiki/entries/entry-1/mentions') {
      await route.fulfill({ json: { count: 0, unlinked_mentions: [] } });
      return;
    }
    if (path.endsWith('/links')) {
      await route.fulfill({
        json: { entry: { id: 'x', title: 'x' }, out_links: [], in_links: [] },
      });
      return;
    }
    if (path.endsWith('/mentions')) {
      await route.fulfill({ json: { count: 0, unlinked_mentions: [] } });
      return;
    }
    if (path.endsWith('/attachments')) {
      await route.fulfill({ json: { attachments: [] } });
      return;
    }
    if (path.endsWith('/versions')) {
      await route.fulfill({ json: { versions: [] } });
      return;
    }
    if (path === '/api/v1/wiki/recycle') {
      await route.fulfill({ json: { items: [] } });
      return;
    }
    await route.fulfill({ json: {} });
  });

  await page.goto(origin);
  await page.locator('input[type="password"]').fill(TOKEN);
  await page.locator('button[type="submit"]').click();
  await page.getByText('Gateway 已连接').waitFor();

  // ── 主题切换: 默认深色,循环 浅色 → 深色 → 跟随系统 ──
  const html = page.locator('html');
  assert.ok((await html.getAttribute('class'))?.includes('dark'), 'app boots dark by default');
  const themeButton = page.getByRole('button', { name: /色彩主题/ });
  await page.emulateMedia({ colorScheme: 'light' });
  await themeButton.click(); // 深色 → 跟随系统,解析为浅色
  await page.waitForFunction(() => !document.documentElement.classList.contains('dark'));
  await themeButton.click(); // → 浅色
  await page.waitForFunction(() => !document.documentElement.classList.contains('dark'));
  await themeButton.click(); // → 深色
  await page.waitForFunction(() => document.documentElement.classList.contains('dark'));
  await themeButton.click(); // → 跟随系统,跟随模拟的系统深色
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.waitForFunction(() => document.documentElement.classList.contains('dark'));

  await page.getByRole('button', { name: 'Wiki' }).click();
  const wiki = page.locator('section[aria-label="知识库工作区"]');
  await page.getByRole('button', { name: /相对论笔记/ }).first().click();
  await wiki.getByRole('heading', { name: '相对论笔记' }).first().waitFor();

  // ── 阅读模式: inline + display math ──
  const inline = wiki.locator('.md-math:not(.md-math--display)').first();
  await inline.waitFor({ timeout: 10_000 });
  assert.ok(inline.locator('math').count(), 'inline math renders a MathML element');
  assert.equal(
    await inline.locator('annotation[encoding="application/x-tex"]').textContent(),
    'E = mc^2',
    'inline math keeps the original TeX in <annotation>',
  );
  const display = wiki.locator('.md-math--display').first();
  await display.waitFor();
  assert.equal(
    await display.locator('math').getAttribute('display'),
    'block',
    'display math is block-level MathML',
  );
  // Two inline ($…$) + one $$ + one \(…\) + one \[…\] → 5 math spans.
  assert.equal(await wiki.locator('.md-math').count(), 5, 'all five formulas render');
  const bracket = wiki.locator('.md-math--display').nth(1);
  assert.equal(
    await bracket.locator('annotation[encoding="application/x-tex"]').textContent(),
    '\\int_0^1 x^2 \\, dx = \\frac{1}{3}',
    'LaTeX bracket block keeps its TeX source',
  );

  // ── 标题左侧大纲级别标记(纯 CSS ::before)+ GitHub 深色标题绿 ──
  const gutterLevel = (locator) => locator.evaluate((el) => getComputedStyle(el, '::before').content);
  const mdH1 = wiki.locator('.wiki-md h1').first();
  assert.equal(await gutterLevel(mdH1), '"H1"', 'H1 shows its outline level in the gutter');
  assert.equal(
    await gutterLevel(wiki.locator('.wiki-md h2').first()),
    '"H2"',
    'H2 shows its outline level in the gutter',
  );
  assert.equal(await mdH1.evaluate((el) => getComputedStyle(el).color), 'rgb(126, 231, 135)', 'dark headings use the GitHub green');

  // ── currency prose stays plain ──
  const prose = wiki.locator('p', { hasText: '学费' });
  assert.equal(await prose.locator('.md-math').count(), 0, '"$5 and $10" is not math');
  assert.ok((await prose.textContent()).includes('$5 and $10'), 'dollar text survives verbatim');

  // ── 编辑模式: block editor keeps raw $$…$$ source ──
  await page.getByTitle('编辑模式').click();
  await wiki.locator('.wiki-block').first().waitFor();
  const mathBlock = wiki.locator('.wiki-block', { hasText: 'E = mc^2' }).last();
  await mathBlock.click();
  const textarea = wiki.locator('.wiki-block--editing textarea');
  await textarea.waitFor();
  const raw = await textarea.inputValue();
  assert.ok(raw.startsWith('$$'), 'math block edits as raw $$ source');
  await page.keyboard.press('Escape');
  await wiki.getByRole('button', { name: '保存' }).click();
  await wiki.getByRole('heading', { name: '相对论笔记' }).first().waitFor();

  assert.deepEqual(errors, []);
  console.log('wiki katex: ok');
} catch (err) {
  console.error('page errors:', errors);
  throw err;
} finally {
  await browser?.close();
  server.kill('SIGTERM');
  await once(server, 'exit').catch(() => {});
}
