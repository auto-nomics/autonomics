import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1426';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1426', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
const articles = [{ id: 'a', title: 'Alpha Study', authors: [], identifiers: [] }, { id: 'b', title: 'Beta Study', authors: [], identifiers: [] }];
const calls = [];
let browser;
try {
  for (let attempt = 0; !output.includes(origin); attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || ['/usr/bin/chromium', '/opt/google/chrome/chrome'].find(existsSync);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, acceptDownloads: true });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html lang="en"><body></body></html>' }));
  await page.route('**/api/v1/**', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace(/^\/api\/v1/, '');
    const body = request.postDataJSON();
    calls.push({ path, method: request.method(), body });
    if (path === '/bib/collections') return route.fulfill({ json: { collections: [] } });
    if (path === '/bib/health') return route.fulfill({ json: { article_count: 2 } });
    if (path === '/bib/collections/changes') return route.fulfill({ json: {} });
    if (path === '/bib/articles') return route.fulfill({ json: { articles, total: 2 } });
    if (path === '/bib/search/export') return route.fulfill({ json: { export: '@article{b, title={Beta Study}}', count: 1 } });
    if (path === '/bib/search/v2') {
      if (body.query === 'slow') await new Promise(resolve => setTimeout(resolve, 700));
      if (body.query === 'bad:field') return route.fulfill({ status: 400, json: { error: 'Unknown search field bad at position 0' } });
      const article = body.query === 'beta' ? articles[1] : articles[0];
      const word = article.id === 'b' ? 'beta' : 'alpha';
      return route.fulfill({ json: {
        articles: [article], total: 1, index_generation: 'test:1', next_cursor: null,
        hits: [{ article_id: article.id, title: article.title, score: 1, snippet: `😀 ${word} evidence`, highlight_ranges: [{ start: 2, end: 2 + word.length }], matched_field: 'abstract' }],
        facets: { tags: [{ value: '22', label: 'gene', count: 1 }], creators: [], publication_types: [], years: [], fulltext_status: [] },
      } }).catch(() => {});
    }
    if (path === '/bib/articles/a') {
      await new Promise(resolve => setTimeout(resolve, 500));
      return route.fulfill({ json: { article: articles[0], annotations: [], fulltext: null } }).catch(() => {});
    }
    if (path === '/bib/articles/b') return route.fulfill({ json: { article: articles[1], annotations: [], fulltext: null } });
    return route.fulfill({ json: {} });
  });
  await page.goto(origin);
  await page.evaluate(async () => {
    await import('/src/app.css');
    const { default: RefreshRuntime } = await import('/@react-refresh');
    RefreshRuntime.injectIntoGlobalHook(window);
    window.$RefreshReg$ = () => {};
    window.$RefreshSig$ = () => type => type;
    window.__vite_plugin_react_preamble_installed__ = true;
    const { mountBibliography } = await import('/tests/bibliography-menu-mount.tsx');
    mountBibliography(document.body);
  });
  const alpha = () => page.getByRole('button', { name: /Alpha Study/ });
  await alpha().waitFor();
  const input = page.getByRole('textbox', { name: 'Search bibliography' });
  await input.fill('alpha');
  await page.locator('mark', { hasText: 'alpha' }).waitFor();
  assert.equal(calls.filter(call => call.path === '/bib/health').length, 1);
  const latestSearch = () => calls.filter(call => call.path === '/bib/search/v2').at(-1).body;
  assert.equal(latestSearch().include_articles, true);
  assert.equal(latestSearch().include_facets, true);
  await alpha().click();
  await input.fill('slow');
  await Promise.all([page.waitForRequest(request => request.url().endsWith('/bib/search/v2') && request.postDataJSON()?.query === 'slow'), input.press('Enter')]);
  await input.fill('beta'); await input.press('Enter');
  await page.locator('mark', { hasText: 'beta' }).waitFor();
  await page.getByRole('button', { name: /Beta Study/ }).click();
  await Promise.all([page.waitForResponse(response => response.url().endsWith('/bib/search/v2') && response.request().postDataJSON()?.filters?.tag_ids?.[0] === 22), page.getByLabel('Exact tag filter').selectOption('22')]);
  assert.deepEqual(latestSearch().filters.tag_ids, [22]);
  await page.waitForTimeout(800);
  assert.equal(await page.locator('mark').first().innerText(), 'beta');
  assert.equal(await alpha().count(), 0, 'late query response must not replace current results');
  assert.equal(await page.locator('aside').last().getByText('Alpha Study', { exact: true }).count(), 0, 'late detail response must not replace selected article');
  assert.equal(calls.filter(call => call.path === '/bib/health').length, 1, 'queries reuse the independent metadata cache');
  await page.screenshot({ path: '/tmp/bibliography-search-p2.png' });
  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Export BibTeX', exact: true }).click();
  assert.equal((await downloadPromise).suggestedFilename(), 'autonomics.bib');
  const exportBody = calls.findLast(call => call.path === '/bib/search/export').body;
  assert.equal(exportBody.search.query, 'beta'); assert.deepEqual(exportBody.search.filters.tag_ids, [22]);
  await input.fill('bad:field'); await input.press('Enter');
  await page.getByRole('alert').filter({ hasText: 'Unknown search field' }).waitFor();
  assert.equal(await page.locator('mark').first().innerText(), 'beta', 'failed query keeps prior results');
  await page.getByRole('button', { name: 'Clear search', exact: true }).click();
  await alpha().waitFor();
  await input.focus(); await input.press('Escape');
  assert.equal(await input.inputValue(), '');
  assert.deepEqual(errors, []);
  console.log('V2 search, Unicode highlights, exact filters, stale responses, metadata cache and export passed');
} finally {
  await browser?.close();
  server.kill('SIGTERM');
  await once(server, 'exit');
}
