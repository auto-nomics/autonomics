import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
const origin = 'http://127.0.0.1:1439';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1439', '--strictPort']);
let output = '', browser, feeds = [], failDelete = true, checks = 0, enabledChanges = 0;
server.stdout.on('data', v => output += v); server.stderr.on('data', v => output += v);
const unexpected = [], errors = [];
const items = Array.from({ length: 51 }, (_, i) => ({ id: `item-${i}`, feed_id: 'test', title: `Research ${i}`, authors: [], read: false, first_seen_ms: 1000 - i, published_ms: null, summary: null, link: null, imported_article_id: null }));
try {
  for (let i = 0; !output.includes(origin); i++) { if (i > 100) throw Error(output); await new Promise(r => setTimeout(r, 50)); }
  browser = await chromium.launch({ executablePath: '/usr/bin/chromium', args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
  page.on('pageerror', e => errors.push(e.message));
  await page.route(origin + '/', r => r.fulfill({ contentType: 'text/html', body: '<html><body></body></html>' }));
  await page.route('**/api/v1/**', r => {
    const request = r.request(), url = new URL(request.url()), path = url.pathname.replace('/api/v1', ''), method = request.method();
    const reply = json => r.fulfill({ json });
    if (path === '/bib/collections') return reply({ collections: [] });
    if (path === '/bib/rss/status') return reply({ scheduler: { running: true, active_checks: 0 } });
    if (path === '/bib/rss/feeds' && method === 'GET') return reply({ feeds });
    if (path === '/bib/rss/feeds' && method === 'POST') {
      feeds.push({ id: 'test', version: 1, spec: request.postDataJSON(), unread_count: 51, item_count: 51, last_error: 'feed fetch failed: feed returned 404 Not Found', last_success_ms: null, next_due_ms: Date.now() }); return reply({ feed: feeds[0] });
    }
    if (path === '/bib/rss/items' && method === 'GET') return reply(!feeds.length ? { items: [], next: null } : url.searchParams.has('before') ? { items: items.slice(50), next: null } : { items: items.slice(0, 50), next: 950 });
    if (path === '/bib/rss/feeds/test' && method === 'PUT') {
      assert.equal(request.postDataJSON().version, feeds[0].version); feeds[0].spec = request.postDataJSON().spec; feeds[0].version++; return reply({ feed: feeds[0] });
    }
    if (path === '/bib/rss/feeds/test/enabled' && method === 'POST') {
      assert.equal(request.postDataJSON().version, feeds[0].version); feeds[0].spec.enabled = request.postDataJSON().enabled; feeds[0].version++; enabledChanges++; return reply({ feed: feeds[0] });
    }
    if (path === '/bib/rss/feeds/test' && method === 'POST') { checks++; feeds[0].last_error = null; return reply({ outcome: { status: 'not_modified', new_items: 0 } }); }
    if (path === '/bib/rss/feeds/test' && method === 'DELETE') {
      assert.equal(Number(url.searchParams.get('version')), feeds[0].version);
      if (failDelete) { failDelete = false; return r.fulfill({ status: 500, json: { error: 'Temporary delete failure' } }); }
      feeds = []; return r.fulfill({ status: 204 });
    }
    unexpected.push(`${method} ${path}`); return r.fulfill({ status: 404, json: { error: 'Unknown endpoint' } });
  });
  await page.goto(origin);
  await page.evaluate(async () => {
    await import('/src/app.css'); const { default: R } = await import('/@react-refresh'); R.injectIntoGlobalHook(window);
    window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => t => t; window.__vite_plugin_react_preamble_installed__ = true;
    (await import('/tests/rss-design-mount.tsx')).mount();
  });
  const button = name => page.getByRole('button', { name, exact: true });
  const dialog = page.getByRole('dialog');
  await button('Add your first feed').click();
  await page.getByLabel('Feed name', { exact: true }).fill('Test feed');
  await page.getByLabel('Feed URL', { exact: true }).fill('https://');
  await button('Save feed').click(); await dialog.getByRole('alert').waitFor(); assert.equal(feeds.length, 0);
  await page.getByLabel('Feed URL', { exact: true }).fill('https://example.org/feed');
  await button('Save feed').click(); await dialog.waitFor({ state: 'hidden' });
  await page.getByRole('heading', { name: 'Research 49', exact: true }).waitFor();
  await button('Load more').click(); await page.getByRole('heading', { name: 'Research 50', exact: true }).waitFor();
  await button('Refresh').click(); await page.waitForTimeout(5500);
  assert.equal(await page.locator('.rss-article').count(), 51);
  await page.locator('.rss-feed-error').filter({ hasText: 'Feed URL not found (404)' }).waitFor();
  assert.equal(await page.locator('.rss-sync').textContent(), 'Background sync on');
  await page.locator('.rss-feed-settings > summary').click(); await button('Edit').click();
  await page.getByLabel('Feed name', { exact: true }).fill('Renamed feed'); await button('Save feed').click(); await dialog.waitFor({ state: 'hidden' });
  await button('Pause').click(); await button('Resume').click(); await button('Pause').waitFor();
  await button('Check now').click(); await button('Check all').click(); await page.locator('.rss-feed-error').waitFor({ state: 'hidden' });
  await page.locator('.rss-feed-select').click(); await page.getByRole('heading', { name: 'Renamed feed', exact: true }).waitFor();
  await button('Delete subscription Renamed feed').click(); await button('Keep feed').click(); assert.equal(feeds.length, 1);
  await button('Delete subscription Renamed feed').click(); await button('Confirm deletion').click();
  await dialog.getByRole('alert').filter({ hasText: 'Temporary delete failure' }).waitFor(); assert.equal(feeds.length, 1);
  await button('Confirm deletion').click(); await dialog.waitFor({ state: 'hidden' });
  await page.getByRole('heading', { name: 'No subscriptions yet.', exact: true }).waitFor();
  assert.equal(await page.getByLabel('Inbox feed filter').inputValue(), '');
  assert.equal(checks, 2); assert.equal(enabledChanges, 2); assert.deepEqual(unexpected, []); assert.deepEqual(errors, []);
  console.log('RSS subscriptions: validation, create/edit, pause/resume, check, pagination/polling, cancel/delete/retry and filter reset passed');
} finally { await browser?.close(); server.kill('SIGTERM'); }
