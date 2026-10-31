// Run with npm run test:wiki. Drives the real SPA against a stubbed
// daemon and checks the SiYuan-style wiki modes:
//   - 阅读模式 renders [[wikilinks]], resolves existing targets, marks
//     missing ones and routes clicks (open / create-prompt),
//   - 编辑模式 is a block editor: click a block to edit its raw markdown,
//     toolbar wraps the selection, Ctrl+Enter opens a fresh block, and
//     保存 PUTs the re-joined content and returns to the reader,
//   - 底部 dock 的链接/提及按词条 title 显示(REST 批量解析 id→title,
//     不再显示裸 id),行点击仍按 id 路由。
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';

const TOKEN = 'test-token-123';
const SESSION = 'session-abc-123';
const origin = 'http://127.0.0.1:1427';

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
  title: '番茄栽培',
  alias: ['tomato'],
  tags: ['园艺', '蔬菜'],
  ctime: 1760000000,
  mtime: 1760000100,
  content: [
    '# 番茄栽培',
    '',
    '种植笔记,参见 [[邻居词条]] 与 [[不存在词]]。',
    '',
    '```rust',
    'fn main() {}',
    '```',
    '',
    '- 育苗',
    '- 定植',
    '',
  ].join('\n'),
};
const NEIGHBOR = {
  id: 'entry-2',
  title: '邻居词条',
  alias: [],
  tags: [],
  ctime: 1760000000,
  mtime: 1760000000,
  content: '# 邻居词条\n\n正文。\n',
};
const BACKLINK = {
  id: 'entry-3',
  title: '反向引用',
  alias: [],
  tags: [],
  ctime: 1760000000,
  mtime: 1760000000,
  content: '# 反向引用\n\n参见 [[番茄栽培]]。\n',
};
const summary = (entry) => {
  const { content: _content, ...rest } = entry;
  return rest;
};

const server = spawn(
  process.execPath,
  ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1427', '--strictPort'],
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
  const errors = [];
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
  // registration order, so the wiki handlers win for /api/v1/wiki.
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
      // Keep the SSE stream pending forever: a finite body closes the
      // stream, and the app's reconnect cycle unmounts the workbench
      // (losing wiki state) until it reconnects.
      await new Promise(() => {});
      return;
    }
    await route.fulfill({ json: {} });
  });

  let savedContent = null;
  await page.route('**/api/v1/wiki/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const title = url.searchParams.get('title');
    if (path === '/api/v1/wiki/entries' && request.method() === 'GET') {
      if (title === '邻居词条') {
        await route.fulfill({ json: { entries: [NEIGHBOR] } });
        return;
      }
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
    if (path === '/api/v1/wiki/entries/entry-2' && request.method() === 'GET') {
      await route.fulfill({ json: NEIGHBOR });
      return;
    }
    if (path === '/api/v1/wiki/entries/entry-3' && request.method() === 'GET') {
      await route.fulfill({ json: BACKLINK });
      return;
    }
    if (path === '/api/v1/wiki/entries/entry-1/content' && request.method() === 'PUT') {
      savedContent = request.postDataJSON().content;
      await route.fulfill({ json: {} });
      return;
    }
    // REST 把链接 id 批量解析成 {id, title}(UI 只显示名称)。
    if (path === '/api/v1/wiki/entries/entry-1/links') {
      await route.fulfill({
        json: {
          entry: { id: 'entry-1', title: '番茄栽培' },
          out_links: [{ id: 'entry-2', title: '邻居词条' }],
          in_links: [{ id: 'entry-3', title: '反向引用' }],
        },
      });
      return;
    }
    if (path === '/api/v1/wiki/entries/entry-1/mentions') {
      await route.fulfill({
        json: { count: 1, unlinked_mentions: [{ id: 'entry-4', title: '提及者' }] },
      });
      return;
    }
    if (path.endsWith('/links')) {
      await route.fulfill({ json: { entry: { id: 'x', title: 'x' }, out_links: [], in_links: [] } });
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

  // ── open the wiki workspace (scoped: the agent transcript also renders markdown) ──
  await page.getByRole('button', { name: 'Wiki' }).click();
  const wiki = page.locator('section[aria-label="知识库工作区"]');
  await page.getByRole('button', { name: /番茄栽培/ }).first().click();

  // ── 阅读模式 ──
  await wiki.getByRole('heading', { name: '番茄栽培' }).first().waitFor();
  assert.equal(await wiki.locator('.md-wikilink').count(), 2, 'both wikilinks render as chips');
  await wiki
    .locator('.md-wikilink--missing[data-wikilink="不存在词"]')
    .waitFor({ timeout: 10_000 });
  await wiki.locator('.md-wikilink[data-wikilink="邻居词条"]:not(.md-wikilink--missing)').waitFor({
    timeout: 10_000,
  });
  assert.equal(await wiki.locator('.md-code').count(), 1, 'code block shell renders');

  // Missing link → create prompt.
  await page.locator('[data-wikilink="不存在词"]').click();
  await page.getByText('词条不存在').waitFor();
  await page.keyboard.press('Escape');
  await page.getByText('词条不存在').waitFor({ state: 'hidden' });









  // Resolved link → pushes the neighbor entry onto this tab's stack.
  await page.locator('[data-wikilink="邻居词条"]').click();
  await wiki.getByRole('heading', { name: '邻居词条' }).first().waitFor();
  await wiki.getByRole('button', { name: '后退' }).click();
  await wiki.getByRole('heading', { name: '番茄栽培' }).first().waitFor();

  // ── 编辑模式: block editor ──
  await page.getByTitle('编辑模式').click();
  await wiki.locator('.wiki-block').first().waitFor();
  const blockCount = await wiki.locator('.wiki-block').count();
  assert.ok(blockCount >= 5, `expected several blocks, got ${blockCount}`);
  assert.equal(await page.locator('.wiki-block--editing').count(), 0, 'nothing is editing yet');

  // Click the paragraph block → raw markdown textarea under the cursor.
  await wiki.locator('.wiki-block', { hasText: '种植笔记' }).first().click();
  const textarea = wiki.locator('.wiki-block--editing textarea');
  await textarea.waitFor();
  const raw = await textarea.inputValue();
  assert.ok(raw.includes('[[邻居词条]]'), 'editing shows raw markdown');

  // Select the line and bold it through the toolbar.
  await textarea.click();
  await page.keyboard.press('End');
  await page.keyboard.press('Shift+Home');
  await page.getByRole('button', { name: '粗体 Ctrl+B' }).click();
  assert.ok(
    (await textarea.inputValue()).startsWith('**种植笔记'),
    'toolbar wraps the selection in **',
  );

  // Ctrl+Enter opens a fresh block below; type into it.
  await textarea.click();
  await page.keyboard.press('Control+Enter');
  const fresh = wiki.locator('.wiki-block--editing textarea');
  assert.equal(await fresh.inputValue(), '', 'the fresh block starts empty');
  await page.keyboard.type('新块内容');

  // Save → PUT carries the re-joined content, view returns to the reader.
  await wiki.getByRole('button', { name: '保存' }).click();
  await wiki.getByRole('heading', { name: '番茄栽培' }).first().waitFor();
  assert.ok(savedContent, 'content was PUT');
  assert.ok(savedContent.startsWith('# 番茄栽培'), 'heading block survives the round-trip');
  assert.ok(savedContent.includes('**种植笔记'), 'bold transform is saved');
  assert.ok(savedContent.includes('新块内容'), 'fresh block content is saved');
  assert.ok(
    savedContent.includes('```rust'),
    'code fence survives the block split/re-join',
  );
  assert.equal(await wiki.locator('.md-wikilink').count(), 2, 'reader is back with rendered links');

  // ── 底部 dock:链接按词条名显示,不再显示裸 id ──
  await wiki.getByRole('button', { name: '链接与历史' }).click();
  await wiki.getByText('反向引用').first().waitFor({ timeout: 10_000 });
  assert.ok((await wiki.getByText('提及者').count()) >= 1, 'mention row shows the title');
  assert.equal(await wiki.getByText('entry-3').count(), 0, 'raw entry ids must not be displayed');
  assert.equal(await wiki.getByText('entry-4').count(), 0, 'raw entry ids must not be displayed');
  // Row click still routes by id (same tab, stack push).
  await wiki.getByText('反向引用').first().click();
  await wiki.getByRole('heading', { name: '反向引用' }).first().waitFor();

  assert.deepEqual(errors, []);
  console.log('wiki siyuan modes: ok');
} finally {
  await browser?.close();
  server.kill('SIGTERM');
  await once(server, 'exit').catch(() => {});
}
