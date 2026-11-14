import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
const origin = 'http://127.0.0.1:1438';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1438', '--strictPort']);
let output = '', browser;
server.stdout.on('data', v => output += v); server.stderr.on('data', v => output += v);
const feed = { id: 'nature', version: 1, spec: { name: 'Nature Research', url: 'https://example.org/feed', enabled: true, interval_seconds: 3600, max_items_per_feed: 500 }, unread_count: 1, item_count: 1, last_success_ms: Date.now(), next_due_ms: Date.now() + 3600000 };
const item = { id: 'one', feed_id: 'nature', title: 'A new atlas of cellular discovery', summary: '<p>Evidence for human biology.</p>', authors: ['Ada Researcher'], published_ms: Date.now(), first_seen_ms: Date.now(), read: false, link: 'https://example.org/article', imported_article_id: null };
try {
  for (let i = 0; !output.includes(origin); i++) { if (i > 100) throw Error(output); await new Promise(r => setTimeout(r, 50)); }
  browser = await chromium.launch({ executablePath: '/usr/bin/chromium', args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 950 } }); page.setDefaultTimeout(30000);
  const errors = []; page.on('pageerror', e => errors.push(e.message));
  await page.route(`${origin}/api/session`, r => r.fulfill({ json: {} }));
  await page.route('**/api/v1/**', r => {
    const url = new URL(r.request().url()), path = url.pathname.replace('/api/v1', '');
    if (path === '/events') return;
    if (path === '/state') return r.fulfill({ json: { last_seq: 0, agents: [], contexts: {}, display_settings: {} } });
    if (path === '/bib/rss/items/one' && r.request().method() === 'POST') item.imported_article_id = 'paper';
    const data = {
      '/bib/rss/feeds': { feeds: [feed] }, '/bib/rss/items': { items: [item], next: null },
      '/bib/rss/status': { scheduler: { running: true, active_checks: 0, last_error: null } },
      '/bib/rss/feeds/nature': { outcome: { status: 'not_modified', new_items: 0 } },
      '/bib/collections': { collections: [{ id: 'research', name: 'Research collection', article_ids: [] }] },
      '/bib/articles': { articles: [], total: 0 }, '/bib/health': { article_count: 0 },
      '/bib/search/status': { engine: 'legacy' },
    };
    return r.fulfill({ json: data[path] ?? {} });
  });
  await page.goto(origin, { waitUntil: "domcontentloaded", timeout: 60000 });
  await page.locator('main[data-workbench]').waitFor();
  const open = async workspace => page.evaluate(async workspace => { const s = await import('/src/app/workbench/workbench-store.ts'); const id = s.openEditor({ kind: 'bibliography', params: { workspace } }); s.setSidebarCollapsed(false); return id; }, workspace);
  const rssId = await open('subscriptions');
  const sidebar = page.locator('[data-slot="workbench-sidebar"]');
  await sidebar.getByLabel('Feed list', { exact: true }).waitFor();
  const editor = page.getByRole('region', { name: 'RSS subscriptions', exact: true });
  assert.equal(await editor.getByLabel('Feed list', { exact: true }).count(), 0);
  for (const name of ['Refresh', 'Check all', 'Add feed']) { assert.equal(await sidebar.getByRole('button', { name, exact: true }).count(), 1); assert.equal(await editor.getByRole('button', { name, exact: true }).count(), 0); }
  await sidebar.locator('.rss-feed-select').filter({ hasText: 'Nature Research' }).click();
  await editor.getByRole('heading', { name: 'Nature Research', exact: true }).waitFor();
  await editor.getByLabel('Search loaded articles').fill('atlas');
  await editor.locator('.rss-article-select').click();
  await editor.getByRole('button', { name: 'Import to Library', exact: true }).click();
  await page.locator('[data-sonner-toast]').filter({ hasText: 'Imported “A new atlas' }).waitFor();
  assert.equal(await editor.locator('.rss-message').count(), 0);
  await sidebar.getByRole('button', { name: 'Check all', exact: true }).click();
  const libraryId = await open('library');
  await sidebar.getByText('Research collection', { exact: true }).waitFor();
  assert.equal(await sidebar.getByLabel('Feed list', { exact: true }).count(), 0);
  await page.evaluate(async id => (await import('/src/app/workbench/workbench-store.ts')).activateEditorById(id), rssId);
  await sidebar.getByLabel('Feed list', { exact: true }).waitFor();
  assert.equal(await editor.getByLabel('Search loaded articles').inputValue(), 'atlas');
  await page.evaluate(async () => (await import("/src/lib/theme.ts")).setThemeMode("light"));
  await page.screenshot({ path: '/tmp/rss-workbench-light.png' });
  await page.evaluate(async () => (await import('/src/lib/theme.ts')).setThemeMode('dark'));
  await page.screenshot({ path: '/tmp/rss-workbench-dark.png' });
  // Separate visible editor groups must not contribute duplicate sidebar trees.
  await page.evaluate(async id => { const s = await import('/src/app/workbench/workbench-store.ts'); s.activateEditorById(id); s.splitEditor(); }, libraryId);
  await sidebar.getByText('Research collection', { exact: true }).waitFor();
  assert.equal(await sidebar.getByLabel('Feed list', { exact: true }).count(), 0);
  await page.evaluate(async id => (await import('/src/app/workbench/workbench-store.ts')).activateEditorById(id), rssId);
  await sidebar.getByLabel('Feed list', { exact: true }).waitFor();
  assert.equal(await sidebar.getByText('Research collection', { exact: true }).count(), 0);
  for (const width of [800, 390]) {
    await page.setViewportSize({ width, height: 850 });
    await editor.getByRole('button', { name: 'Show subscriptions sidebar' }).click();
    await sidebar.getByRole('button', { name: 'Add feed', exact: true }).click();
    await page.getByRole('dialog', { name: 'Add feed', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await page.screenshot({ path: `/tmp/rss-workbench-${width}.png` });
    assert.ok(await page.evaluate(() => document.body.scrollWidth <= innerWidth));
  }
  assert.deepEqual(errors, []);
  console.log('RSS workbench: sidebar ownership, actions, filter binding, toast, tab state, split focus, themes and mobile drawer passed');
} finally { await browser?.close(); server.kill('SIGTERM'); }
