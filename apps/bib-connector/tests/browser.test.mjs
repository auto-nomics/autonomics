import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
const require = createRequire(new URL('../../gui/package.json', import.meta.url));
const { chromium } = require('playwright');
const executablePath = process.env.PDF_TEST_CHROMIUM || ['/usr/bin/chromium', '/opt/google/chrome/chrome'].find(existsSync);
const FILES = ['popup.html', 'popup.js', 'style.css', 'client.js', 'translators.js', 'content.js', 'progress.html', 'progress.js'];

async function serve() {
  const server = createServer(async (req, res) => {
    const name = new URL(req.url, 'http://localhost').pathname.slice(1).split('?')[0] || 'popup.html';
    if (!FILES.includes(name)) { res.writeHead(404).end(); return; }
    res.setHeader('Content-Type', name.endsWith('.js') ? 'text/javascript' : name.endsWith('.css') ? 'text/css' : 'text/html');
    res.setHeader('Access-Control-Allow-Origin', '*');
    res.end(await readFile(new URL(`../${name}`, import.meta.url)));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return { server, origin: `http://127.0.0.1:${server.address().port}` };
}

// Zotero-style detection runs against real DOMs with the bundled translator registry.
test('translators detect and extract publisher, PubMed, arXiv, COinS and generic pages', async () => {
  const { server, origin } = await serve();
  const browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  try {
    const page = await browser.newPage();
    // Serve the module same-origin via route interception; a cross-origin http import would be blocked as mixed content.
    const moduleBody = await readFile(new URL('../translators.js', import.meta.url), 'utf8');
    await page.route('**/__translators.js', route => route.fulfill({ contentType: 'text/javascript', body: moduleBody }));
    const detect = url => page.evaluate(async url => {
      const { detectWeb } = await import(new URL('__translators.js', location.href).href);
      const match = detectWeb(document, url);
      return match && { translator: match.translator.id, type: match.type,
        item: match.translator.doWeb(document, url) };
    }, url);

    await page.route('https://publisher.example/paper', route => route.fulfill({ contentType: 'text/html', body: `
      <head><title>Publisher branding</title><meta name="citation_title" content="A scientific paper">
      <meta name="citation_author" content="Wang, Ming"><meta name="citation_author" content="Ada Lovelace">
      <meta name="citation_doi" content="https://doi.org/10.1234/example"><meta name="citation_date" content="2026/01/02">
      <meta name="citation_journal_title" content="Journal"><meta name="citation_pdf_url" content="https://publisher.example/paper.pdf">
      <meta name="citation_keywords" content="genetics; science"></head><body>full text` }));
    await page.goto('https://publisher.example/paper');
    const publisher = await detect('https://publisher.example/paper');
    assert.equal(publisher.translator, 'embedded-metadata');
    const capture = publisher.item.items[0];
    assert.equal(capture.title, 'A scientific paper'); assert.equal(capture.doi, '10.1234/example');
    assert.equal(capture.year, 2026); assert.deepEqual(capture.authors, ['Wang, Ming', 'Ada Lovelace']);
    assert.equal(capture.pdf_url, 'https://publisher.example/paper.pdf');
    assert.equal(publisher.item.pdfUrl, 'https://publisher.example/paper.pdf');

    await page.setContent(`<title>Listing</title><body>
      <a class="docsum-title" href="/11111111/">First result</a>
      <a class="docsum-title" href="/22222222/">Second result</a>`);
    const listing = await detect('https://pubmed.ncbi.nlm.nih.gov/?term=genetics');
    assert.equal(listing.type, 'multiple');
    assert.deepEqual(listing.item.multiple.map(entry => entry.title), ['First result', 'Second result']);

    await page.setContent(`<title>arXiv</title><body><blockquote class="abstract">We study things.</blockquote>`);
    const preprint = await detect('https://arxiv.org/abs/2601.01234v2');
    assert.equal(preprint.translator, 'arxiv'); assert.equal(preprint.type, 'preprint');
    assert.equal(preprint.item.items[0].arxiv, '2601.01234v2');
    assert.equal(preprint.item.pdfUrl, 'https://arxiv.org/pdf/2601.01234v2');

    await page.setContent(`<title>Library search</title><body>
      <span class="Z3988" title="ctx_ver=Z3988-2004&amp;rft.atitle=Paper+One&amp;rft.au=Ada&amp;rft.date=2024&amp;rft_id=info:doi/10.1000/one">x</span>
      <span class="Z3988" title="ctx_ver=Z3988-2004&amp;rft.atitle=Paper+Two">x</span>`);
    const coins = await detect('https://library.example/search');
    assert.equal(coins.type, 'multiple');
    assert.deepEqual(coins.item.multiple.map(entry => entry.title), ['Paper One', 'Paper Two']);
    const single = await page.evaluate(async () => {
      const { TRANSLATORS } = await import(new URL('__translators.js', location.href).href);
      const coins = TRANSLATORS.find(t => t.id === 'coins');
      document.querySelectorAll('span.Z3988')[1].remove();
      return coins.doWeb(document, 'https://library.example/search').items[0];
    }, origin);
    assert.equal(single.title, 'Paper One'); assert.equal(single.authors[0], 'Ada');
    assert.equal(single.year, 2024); assert.equal(single.doi, '10.1000/one');

    await page.setContent('<title>A plain webpage</title><meta name="description" content="A summary">');
    const generic = await detect('https://random.example/post');
    assert.equal(generic.translator, 'webpage');
    assert.equal(generic.item.items[0].abstract_text, 'A summary');
  } finally { await browser.close(); server.close(); }
});

test('popup preview, credentials, save with PDF attachment and offline retry', async () => {
  const { server, origin } = await serve();
  const browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  try {
    const page = await browser.newPage();
    await page.addInitScript(() => {
      window.localData = {}; window.sessionData = {}; window.permissions = [];
      const store = data => ({ get: async keys => Object.fromEntries((Array.isArray(keys) ? keys : [keys]).map(key => [key, data[key]])), set: async value => Object.assign(data, value), remove: async key => delete data[key], setAccessLevel: async () => {} });
      window.chrome = {
        storage: { local: store(window.localData), session: store(window.sessionData) },
        permissions: { request: async request => { window.permissions.push(request); return true; } },
        runtime: { sendMessage: async () => ({ translator: 'PubMed', capture: { title: 'Captured paper', authors: ['Ada'], doi: '10.1000/example', keywords: [], source_url: 'https://pubmed.ncbi.nlm.nih.gov/1/', pdf_url: 'https://pubmed.ncbi.nlm.nih.gov/1.pdf' } }) },
      };
    });
    const requests = []; let fail = false;
    await page.route('http://127.0.0.1:8765/api/v1/bib/**', async route => {
      const req = route.request();
      let body; try { body = req.postDataJSON(); } catch { body = req.postData(); }
      requests.push({ url: req.url(), method: req.method(), body, auth: req.headers().authorization, type: req.headers()['content-type'] });
      const headers = { 'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Headers': 'Authorization,Content-Type', 'Access-Control-Allow-Methods': 'GET,POST,OPTIONS' };
      if (req.method() === 'OPTIONS') return route.fulfill({ status: 204, headers });
      if (req.url().endsWith('/collections')) return route.fulfill({ headers, json: { collections: [{ id: 'c1', name: 'Research' }] } });
      if (req.url().endsWith('/articles/manual')) return route.fulfill({ headers, status: fail ? 503 : 201, json: fail ? { error: 'Try later' } : { article: { id: 'saved' } } });
      return route.fulfill({ headers, json: { ok: true } });
    });
    await page.route('https://pubmed.ncbi.nlm.nih.gov/1.pdf', route => route.fulfill({ contentType: 'application/pdf', body: '%PDF-1.4 fake' }));
    await page.goto(`${origin}/popup.html`);
    await page.waitForFunction(() => document.getElementById('title').value === 'Captured paper');
    await page.locator('#token').fill('private-token');
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Connected' }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.permissions[0].origins.slice(0, 1)), ['http://127.0.0.1:8765/*']);
    assert.equal((await page.evaluate(() => window.sessionData)).token, 'private-token');
    await page.locator('#collection').selectOption('c1');
    await page.getByRole('button', { name: 'Save metadata', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Saved to your library' }).waitFor();
    const saved = requests.find(req => req.url.endsWith('/articles/manual') && req.method === 'POST');
    assert.equal(saved.body.title, 'Captured paper'); assert.equal(saved.auth, 'Bearer private-token');
    assert.ok(requests.some(req => req.url.endsWith('/collections/c1/articles')));
    const upload = requests.find(req => req.url.includes('/articles/saved/attachments'));
    assert.ok(upload, 'attachment upload happened'); assert.match(upload.type, /multipart\/form-data/);
    assert.deepEqual((await page.evaluate(() => window.localData)).outbox, []);
    fail = true; await page.getByRole('button', { name: 'Save metadata', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Try later' }).waitFor();
    assert.equal((await page.evaluate(() => window.localData)).outbox.length, 1);
  } finally { await browser.close(); server.close(); }
});

test('progress window renders saving, selection and errors', async () => {
  const { server, origin } = await serve();
  const browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  try {
    const page = await browser.newPage();
    const posted = [];
    await page.exposeFunction('recordPost', payload => posted.push(payload));
    await page.goto(`${origin}/progress.html#nonce-1`);
    await page.evaluate(() => { window.addEventListener('message', event => window.recordPost(event.data)); });
    const send = data => page.evaluate(async data => window.dispatchEvent(new MessageEvent('message', { data })), data);
    await send({ nonce: 'nonce-1', event: 'show', headline: 'Saving via PubMed' });
    await send({ nonce: 'nonce-1', event: 'itemProgress', index: 0, title: 'Paper A', progress: 'Attaching PDF…' });
    await send({ nonce: 'nonce-1', event: 'itemProgress', index: 0, title: 'Paper A', progress: 'Saved with PDF', done: true });
    await page.waitForFunction(() => document.querySelector('#items li .state').textContent === 'Saved with PDF');
    await send({ nonce: 'nonce-1', event: 'select', entries: [{ index: 0, title: 'One' }, { index: 1, title: 'Two' }] });
    await page.waitForFunction(() => !document.getElementById('selector').classList.contains('hidden'));
    await page.locator('#entries label:has-text("Two") input').uncheck();
    await page.locator('#ok').click();
    await page.waitForTimeout(300);
    assert.deepEqual(posted.filter(p => p.kind === 'selection').map(p => p.indices), [[0]]);
    await send({ nonce: 'wrong', event: 'error', message: 'must be ignored' });
    await send({ nonce: 'nonce-1', event: 'error', message: 'boom' });
    await page.waitForFunction(() => document.getElementById('error').textContent === 'boom');
  } finally { await browser.close(); server.close(); }
});
