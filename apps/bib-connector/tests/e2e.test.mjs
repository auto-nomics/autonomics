/**
 * End-to-end smoke against a real unpacked extension loaded in headless Chromium.
 * Headless Chromium suppresses optional-permission prompts, so this test builds a
 * variant with permanent host permissions (which need no prompt) and seeds the
 * connection settings directly, the same way Zotero's tests drive its connector.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { cpSync, mkdirSync, readFileSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const require = createRequire(new URL('../../gui/package.json', import.meta.url));
const executablePath = process.env.PDF_TEST_CHROMIUM || ['/usr/bin/chromium', '/opt/google/chrome/chrome'].find(existsSync);
const { chromium } = require('playwright');

test('real extension: detect, badge, save with PDF attachment', { skip: !executablePath || !existsSync(new URL('../dist/unpacked/manifest.json', import.meta.url)) }, async () => {
  // Test build: permanent host permissions instead of the optional ones the popup requests.
  const extDir = join(tmpdir(), `bib-connector-e2e-${process.pid}`);
  rmSync(extDir, { recursive: true, force: true });
  mkdirSync(extDir, { recursive: true });
  cpSync(new URL('../dist/unpacked', import.meta.url), extDir, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(extDir, 'manifest.json')));
  manifest.host_permissions = ['http://127.0.0.1/*', 'http://localhost/*', 'https://*/*', 'http://*/*'];
  writeFileSync(join(extDir, 'manifest.json'), JSON.stringify(manifest, null, 2));

  const profile = join(tmpdir(), `bib-connector-profile-${process.pid}`);
  rmSync(profile, { recursive: true, force: true });
  const ctx = await chromium.launchPersistentContext(profile, {
    executablePath, headless: true,
    args: ['--no-sandbox', `--disable-extensions-except=${extDir}`, `--load-extension=${extDir}`],
  });
  try {
    const worker = ctx.serviceWorkers()[0] ?? await ctx.waitForEvent('serviceworker', { timeout: 15000 });
    const extId = new URL(worker.url()).host;
    const seen = [];
    await ctx.route('http://127.0.0.1:8765/api/v1/bib/**', route => {
      const url = route.request().url();
      seen.push(route.request().method() + ' ' + url.replace('http://127.0.0.1:8765/api/v1/bib', ''));
      const headers = { 'Access-Control-Allow-Origin': '*' };
      if (url.endsWith('/collections')) return route.fulfill({ headers, json: { collections: [] } });
      if (url.endsWith('/articles/manual')) return route.fulfill({ headers, status: 201, json: { article: { id: 'a1' } } });
      if (url.includes('/articles/a1/attachments')) return route.fulfill({ headers, json: { ok: true } });
      if (url.endsWith('/health')) return route.fulfill({ headers, json: { ok: true } });
      return route.fulfill({ headers, json: {} });
    });
    await ctx.route('https://publisher.example/p.pdf', route => route.fulfill({ contentType: 'application/pdf', body: '%PDF-1.4 real' }));

    const page = await ctx.newPage();
    await page.route('https://publisher.example/**', route => route.fulfill({ contentType: 'text/html', body: `
      <html><head><title>P</title><meta name="citation_title" content="Real sw paper">
      <meta name="citation_author" content="Ada"><meta name="citation_doi" content="10.1000/real">
      <meta name="citation_pdf_url" content="https://publisher.example/p.pdf"></head><body>x</body></html>` }));
    await page.goto('https://publisher.example/paper');

    // Seed the connection like the popup's Connect button would have stored it.
    const seed = await ctx.newPage();
    await seed.goto(`chrome-extension://${extId}/popup.html?view=options`);
    await seed.evaluate(async () => {
      await chrome.storage.local.set({ origin: 'http://127.0.0.1:8765', detect: true, preferPdf: true });
      await chrome.storage.session.set({ token: 'tok', origin: 'http://127.0.0.1:8765' });
    });
    await seed.close();

    await page.bringToFront();
    await page.goto('https://publisher.example/paper2');
    await page.waitForTimeout(2500); // content script detects and the worker sets the icon
    const info = await worker.evaluate(async () => {
      const tabs = await chrome.tabs.query({});
      const tab = tabs.find(candidate => candidate.url?.startsWith('https://publisher.example'));
      return { id: tab.id, title: await chrome.action.getTitle({ tabId: tab.id }), badge: await chrome.action.getBadgeText({ tabId: tab.id }) };
    });
    assert.match(info.title, /Save to Autonomics \(Embedded Metadata\)/);
    assert.equal(info.badge, '●');

    await worker.evaluate(async tabId => { await chrome.tabs.sendMessage(tabId, { type: 'save' }); }, info.id);
    await page.waitForTimeout(5000);

    assert.ok(seen.some(entry => entry === 'POST /articles/manual'), 'article created');
    assert.ok(seen.some(entry => entry.includes('POST /articles/a1/attachments')), 'PDF attachment uploaded');
    const outbox = await worker.evaluate(async () => (await chrome.storage.local.get(['outbox'])).outbox || []);
    assert.deepEqual(outbox, []);
  } finally {
    await ctx.close().catch(() => {});
    rmSync(extDir, { recursive: true, force: true });
    rmSync(profile, { recursive: true, force: true });
  }
});
