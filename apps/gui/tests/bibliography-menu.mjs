// Run with npm run test:bib-menu. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, readFileSync } from 'node:fs';

const articles = [
  { id: 'a1', title: 'Alpha paper', authors: [{ last_name: 'Wang', fore_name: 'Li', orcid: '0000-0002-1825-0097', affiliation: 'Research lab', corresponding: true }], identifiers: [{ kind: 'other', value: 'archive-123' }], month: 6, pub_types: ['Review'], source: 'manual' },
  { id: 'a2', title: 'Beta preprint', authors: [], identifiers: [] },
];
const requests = [];
let deleted = false;
let rejectMetadataSave = false;
const metadataUpdates = [];

const origin = 'http://127.0.0.1:1423';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1423', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    if (output.includes(origin)) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());
  await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html lang="zh-CN"><body></body></html>' }));
  await page.route('**/api/v1/**', async route => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname.replace(/^\/api\/v1/, '');
    requests.push(`${request.method()} ${path}${url.search}`);
    if (request.method() === 'GET' && path === '/bib/collections') return route.fulfill({ json: { collections: [] } });
    if (request.method() === 'GET' && path === '/bib/articles') {
      return route.fulfill({
        json: {
          articles: articles.filter(article => !deleted || article.id !== 'a2'),
          total: articles.length - (deleted ? 1 : 0),
          attachment_summaries: {
            a1: { pdf: true, snapshot: false, link: false, file: true, note: false, count: 1 },
          },
        },
      });
    }
    if (request.method() === 'PUT' && path === '/bib/articles/a1') {
      metadataUpdates.push(request.postDataJSON());
      if (rejectMetadataSave) return route.fulfill({ status: 500, json: { error: 'Storage unavailable' } });
      articles[0] = request.postDataJSON();
      return route.fulfill({ json: { article: articles[0] } });
    }
    if (request.method() === 'GET' && path === '/bib/articles/a1') {
      return route.fulfill({ json: { article: articles[0], fulltext: { file_format: 'pdf', file_path: 'vfs://bib/研究资料.pdf' }, attachments: [{ key: 'supplement', link_mode: 0, title: 'Supplementary data', original_filename: '补充资料.txt', content_type: 'text/plain', storage_hash: 'fixture-hash' }], annotations: [] } });
    }
    if (request.method() === 'POST' && path.endsWith('/fulltext')) {
      return route.fulfill({ json: { fulltext: { file_format: 'pdf' } } });
    }
    if (request.method() === 'DELETE' && path.endsWith('/fulltext')) {
      return route.fulfill({ json: { deleted: true } });
    }
    if (request.method() === 'DELETE' && /^\/bib\/articles\/[^/]+$/.test(path)) {
      if (path.endsWith('/a2')) deleted = true;
      return route.fulfill({ json: { deleted: true } });
    }
    if (request.method() === 'GET' && path.endsWith('/attachments/supplement/raw')) {
      assert.equal(url.searchParams.get('expected_hash'), 'fixture-hash');
      return route.fulfill({ contentType: 'text/plain; charset=utf-8', body: '附件内容\n' });
    }
    if (request.method() === 'GET' && path.endsWith('/fulltext/raw')) {
      return route.fulfill({ contentType: 'text/plain; charset=utf-8', body: 'plain text fixture\n' });
    }
    return route.fulfill({ json: { annotations: [] } });
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
  await page.waitForFunction(() => document.querySelectorAll('button[data-bib-row-primary]').length === 2);

  // Edit the full record, validate locally, preserve hidden fields, and retry failed saves.
  await page.locator('button[data-bib-row-primary]').first().click();
  await page.getByRole('button', { name: 'Edit metadata', exact: true }).click();
  let editor = page.getByRole('dialog', { name: 'Edit metadata', exact: true });
  await editor.getByRole('textbox', { name: 'Title', exact: true }).fill('');
  await editor.getByRole('button', { name: 'Save changes', exact: true }).click();
  await editor.getByRole('alert').filter({ hasText: 'Title is required' }).waitFor();
  assert.equal(metadataUpdates.length, 0);
  await editor.getByRole('textbox', { name: 'Title', exact: true }).fill('Revised Alpha paper');
  await editor.getByRole('textbox', { name: 'Year', exact: true }).fill('2026.5');
  await editor.getByRole('button', { name: 'Save changes', exact: true }).click();
  await editor.getByRole('alert').filter({ hasText: 'whole number' }).waitFor();
  assert.equal(metadataUpdates.length, 0);
  await editor.getByRole('textbox', { name: 'Year', exact: true }).fill('2026');
  await editor.getByRole('textbox', { name: 'Publication', exact: true }).fill('Edited Journal');
  await editor.getByRole('textbox', { name: 'Volume', exact: true }).fill('12');
  await editor.getByRole('textbox', { name: 'Author 1 given names', exact: true }).fill('Ming');
  await editor.getByRole('button', { name: 'Add identifier', exact: true }).click();
  await editor.getByRole('textbox', { name: 'Identifier 2 value', exact: true }).fill('10.1234/example');
  await editor.getByRole('textbox', { name: 'Abstract', exact: true }).fill('Updated abstract');
  await editor.getByRole('textbox', { name: 'Tags (one per line)', exact: true }).fill('genetics\nreview\ngenetics');
  rejectMetadataSave = true;
  await editor.getByRole('button', { name: 'Save changes', exact: true }).click();
  await editor.getByRole('alert').filter({ hasText: 'Storage unavailable' }).waitFor();
  assert.equal(await editor.getByRole('textbox', { name: 'Title', exact: true }).inputValue(), 'Revised Alpha paper');
  rejectMetadataSave = false;
  await editor.getByRole('button', { name: 'Save changes', exact: true }).click();
  await editor.waitFor({ state: 'detached' });
  await page.locator('button[data-bib-row-primary]').filter({ hasText: 'Revised Alpha paper' }).waitFor();
  assert.equal(articles[0].month, 6);
  assert.deepEqual(articles[0].pub_types, ['Review']);
  assert.equal(articles[0].authors[0].orcid, '0000-0002-1825-0097');
  assert.equal(articles[0].authors[0].affiliation, 'Research lab');
  assert.equal(articles[0].authors[0].fore_name, 'Ming');
  assert.equal(articles[0].identifiers[0].value, 'archive-123');
  assert.deepEqual(articles[0].keywords, ['genetics', 'review']);
  assert.equal(articles[0].year, 2026);
  await page.getByRole('button', { name: 'Edit metadata', exact: true }).click();
  editor = page.getByRole('dialog', { name: 'Edit metadata', exact: true });
  assert.equal(await editor.getByRole('textbox', { name: 'Volume', exact: true }).inputValue(), '12');
  await editor.getByRole('textbox', { name: 'Title', exact: true }).fill('Discard this title');
  await editor.getByRole('button', { name: 'Cancel', exact: true }).click();
  await editor.waitFor({ state: 'detached' });
  assert.equal(metadataUpdates.length, 2);
  assert.equal(articles[0].title, 'Revised Alpha paper');

  // Both attachment surfaces offer a real original-file download, independent of the reader.
  const downloadButtons = page.getByRole('button', { name: 'Download Supplementary data', exact: true });
  await downloadButtons.first().waitFor();
  assert.equal(await downloadButtons.count(), 2);
  const downloaded = page.waitForEvent('download');
  await downloadButtons.last().click();
  const file = await downloaded;
  assert.equal(file.suggestedFilename(), '补充资料.txt');
  assert.equal(readFileSync(await file.path(), 'utf8'), '附件内容\n');
  await page.route('**/attachments/supplement/raw?*', route => route.fulfill({ status: 404, json: { error: 'Original file missing' } }));
  await downloadButtons.first().click();
  await page.getByText('Unable to download attachment: Error: Original file missing', { exact: true }).first().waitFor();
  assert.ok(await downloadButtons.first().isEnabled());

  const menuItem = (name) =>
    page.locator('[data-slot=context-menu-content]').last().locator(`[data-slot=context-menu-item]:has-text("${name}")`);
  const dismissMenu = async () => {
    await page.mouse.click(5, 500);
    await page.waitForFunction(() => !document.querySelector('[data-slot=context-menu-content]'));
  };

  // Right-click row with an attachment: read + delete enabled.
  await page.locator('button:has-text("Alpha paper")').click({ button: 'right' });
  await page.waitForSelector('[data-slot=context-menu-content]');
  assert.ok(await menuItem('Read').isEnabled());
  assert.ok(await menuItem('Delete attachment').isEnabled());
  assert.ok(await menuItem('Delete article').isEnabled());
  await dismissMenu();

  // Right-click row without an attachment: read + delete-attachment disabled.
  await page.locator('button:has-text("Beta preprint")').click({ button: 'right' });
  await page.waitForSelector('[data-slot=context-menu-content]');
  assert.ok(await menuItem('Read').isDisabled());
  assert.ok(await menuItem('Delete attachment').isDisabled());

  // Upload attachment through the menu.
  const chooserPromise = page.waitForEvent('filechooser');
  await menuItem('Upload attachment').click();
  const chooser = await chooserPromise;
  await chooser.setFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('fixture') });
  await page.waitForFunction(() => {
    const row = [...document.querySelectorAll('button')].find(node => node.textContent?.includes('Beta preprint'));
    return Boolean(row?.querySelector('[aria-label="Stored file"]'));
  });
  assert.ok(requests.includes('POST /bib/articles/a2/fulltext'), requests.join(', '));

  // Checkboxes drive selective export.
  const waitForRequest = async (match) => {
    for (let attempt = 0; attempt < 200; attempt++) {
      if (requests.some(value => value.includes(match))) return;
      await new Promise(resolve => setTimeout(resolve, 25));
    }
    assert.fail(`missing request: ${match}\n${requests.join(', ')}`);
  };
  const selectAll = page.locator('[aria-label="Select all visible articles"]');
  await page.locator('[aria-label="Select Beta preprint"]').click();
  assert.ok(await selectAll.getAttribute('data-indeterminate') !== null, 'header checkbox should be indeterminate');
  await page.locator('[aria-label="Export BibTeX"]').click();
  await waitForRequest('/bib/export?format=bibtex&query=&ids=a2');
  await selectAll.click();
  assert.ok(await selectAll.getAttribute('data-checked') !== null, 'header checkbox should be fully checked');
  assert.ok(await page.locator('[aria-label="Select Revised Alpha paper"]').getAttribute('data-checked') !== null);
  await page.locator('[aria-label="Export BibTeX"]').click();
  await waitForRequest('ids=a2%2Ca1');
  await selectAll.click();
  assert.ok(await selectAll.getAttribute('data-indeterminate') === null
    && await selectAll.getAttribute('data-checked') === null, 'header checkbox should be unchecked');

  // Read sniffs the served content type and opens the TXT reader.
  await page.locator('button:has-text("Beta preprint")').click({ button: 'right' });
  await page.waitForSelector('[data-slot=context-menu-content]');
  await menuItem('Read').click();
  await page.waitForSelector('.zotero-pdf-reader[data-reader-kind="txt"] .reader-txt-document');
  assert.equal(await page.locator('[data-output=font-size]').textContent(), '18 px');
  await page.locator('[data-action=close]').click();
  await page.waitForFunction(() => !document.querySelector('.zotero-pdf-reader'));

  // Delete attachment calls the fulltext endpoint and drops the file icon.
  await page.locator('button:has-text("Alpha paper")').click({ button: 'right' });
  await page.waitForSelector('[data-slot=context-menu-content]');
  await menuItem('Delete attachment').click();
  await page.waitForFunction(() => {
    const row = [...document.querySelectorAll('button')].find(node => node.textContent?.includes('Alpha paper'));
    return row && !row.querySelector('[aria-label="Stored file"]');
  });
  assert.ok(requests.includes('DELETE /bib/articles/a1/fulltext'), requests.join(', '));

  // Delete article confirms, calls the endpoint and refreshes the list.
  await page.locator('button:has-text("Beta preprint")').click({ button: 'right' });
  await page.waitForSelector('[data-slot=context-menu-content]');
  await menuItem('Delete article').click();
  await page.waitForFunction(() => ![...document.querySelectorAll('button')].some(node => node.textContent?.includes('Beta preprint')));
  assert.ok(requests.includes('DELETE /bib/articles/a2'), requests.join(', '));

  // The desktop path uses the native save dialog and preserves attachment identity.
  const desktop = await browser.newPage();
  await desktop.addInitScript(() => {
    window.nativeCalls = [];
    window.savePath = '/tmp/补充资料.txt';
    window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
      window.nativeCalls.push({ command, args });
      if (command === 'plugin:dialog|save') return window.savePath;
      if (command === 'save_bib_attachment') return;
      throw new Error(`Unexpected command: ${command}`);
    } };
  });
  await desktop.goto(origin);
  const nativeResult = await desktop.evaluate(async () => {
    const { downloadBibAttachment } = await import('/src/gateway.ts');
    const saved = await downloadBibAttachment('a1', '补充资料.txt', 'supplement', 'fixture-hash');
    window.savePath = null;
    const cancelled = await downloadBibAttachment('a1', '补充资料.txt', 'supplement');
    return { saved, cancelled, calls: window.nativeCalls };
  });
  assert.equal(nativeResult.saved, '/tmp/补充资料.txt');
  assert.equal(nativeResult.cancelled, null);
  assert.deepEqual(nativeResult.calls.filter(call => call.command === 'save_bib_attachment'), [{
    command: 'save_bib_attachment', args: { articleId: 'a1', attachmentKey: 'supplement', expectedHash: 'fixture-hash', path: '/tmp/补充资料.txt' },
  }]);
  await desktop.close();

  assert.deepEqual(errors, []);
  await page.close();
  console.log('PASS bibliography table: metadata edit/validation/retry/preservation/cancel, attachment download bytes/name/errors, native save/cancel, right-click menu, per-row states, upload, sniffed read, delete attachment, delete article, checkboxes, selective export');
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, 'exit');
}
