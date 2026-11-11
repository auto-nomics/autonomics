import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import { existsSync, readFileSync } from 'node:fs';
import { extractPDF } from '../../../tools/zotero-extractor/extract.mjs';
import { PDFPositionMapper } from '../src/vendor/zotero-sdt.generated.js';

const pdf = readFileSync(new URL('../../../tools/zotero-extractor/test/fixtures/synthetic-list-links.pdf', import.meta.url));
const { structure } = await extractPDF(pdf);
let activePDF = pdf, activeStructure = structure;
let origin = '';
const reservation = createServer();
await new Promise(resolve => reservation.listen(0, '127.0.0.1', resolve));
const port = reservation.address().port;
await new Promise(resolve => reservation.close(resolve));
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 600) throw new Error(`Vite failed: ${output}`);
    const address = output.match(/http:\/\/127\.0\.0\.1:\d+/)?.[0];
    if (address) { origin = address; break; }
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, deviceScaleFactor: 1.5 });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  let annotations = [], structureRequests = 0, fail = false, failPatch = false;
  const patches = [];
  await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html lang="zh-CN"><body></body></html>' }));
  await page.route('**/api/v1/**', async route => {
    const request = route.request();
    if (request.url().endsWith('/fulltext/raw')) return route.fulfill({ contentType: 'application/pdf', body: activePDF });
    if (request.url().endsWith('/fulltext/structured')) {
      structureRequests++;
      return route.fulfill(fail ? { status: 503, json: { error: 'Extractor unavailable' } } : { json: { structure: activeStructure } });
    }
    if (request.method() === 'PATCH' && request.url().includes('/annotations/')) {
      const patch = request.postDataJSON();
      patches.push(patch);
      if (failPatch) return route.fulfill({ status: 503, json: { error: 'Temporary save failure' } });
      const id = decodeURIComponent(request.url().split('/').at(-1));
      const annotation = annotations.find(value => value.id === id);
      Object.assign(annotation, patch);
      return route.fulfill({ json: { annotation } });
    }
    if (request.method() === 'DELETE' && request.url().includes('/annotations/')) {
      annotations = annotations.filter(value => value.id !== decodeURIComponent(request.url().split('/').at(-1)));
      return route.fulfill({ json: { deleted: true } });
    }
    if (request.method() === 'POST') {
          const annotation = { ...request.postDataJSON(), id: String(annotations.length) };
          annotations.push(annotation);
          return route.fulfill({ status: 201, json: { annotation } });
        }
    return route.fulfill({ json: { annotations } });
  });
  await page.goto(origin);
  const open = (articleId = 'reading-test') => page.evaluate(async (articleId) => {
    await import('/src/app.css');
    // These isolated fixtures bypass Vite's HTML transform.
      const { default: RefreshRuntime } = await import('/@react-refresh');
      RefreshRuntime.injectIntoGlobalHook(window);
      window.$RefreshReg$ = () => {};
      window.$RefreshSig$ = () => type => type;
      window.__vite_plugin_react_preamble_installed__ = true;
      const { openPdfReader } = await import('/src/features/reader/pdf-reader.ts');
    window.references = [];
    window.reader = await openPdfReader({ articleId, title: 'Synthetic reference links', onReference: reference => window.references.push(reference) });
  }, articleId);
  annotations = [{ id: 'agent-legacy', kind: 'highlight', content: 'The invented sources', comment: 'Agent note', page: 1,
    rects: [{ left: 25, top: 0, width: 20, height: 0, space: 'char' }] }];
  await open('legacy-agent-note');
  const mark = page.locator('.reader-annotation[data-annotation-id="agent-legacy"]').first();
  await mark.waitFor();
  await page.waitForTimeout(300);
  await mark.evaluate(node => node.scrollIntoView({ block: 'center' }));
  await page.waitForTimeout(100);
  const box = await mark.boundingBox();
  assert.ok(box.width > 0 && box.height > 0);
  await page.mouse.click(box.x + 5, box.y + box.height / 2);
  const inspector = page.getByRole('dialog', { name: 'Annotation details' });
  await inspector.waitFor();
  assert.equal(await inspector.getByRole('textbox', { name: 'Annotation comment' }).inputValue(), 'Agent note');
  await inspector.getByRole('textbox', { name: 'Annotation comment' }).fill('Updated agent note');
  await page.waitForFunction(() => document.querySelector('.reader-annotation-save-status').textContent === 'Saved');
  assert.equal(await page.locator('.reader-annotation[data-annotation-id="agent-legacy"]').count(), 1);
  assert.equal(annotations[0].comment, 'Updated agent note');
  await page.keyboard.press('Escape');
  await page.locator('[data-action=reading-mode]').click();
  await page.waitForSelector('.reader-reflow-highlight[data-annotation-id="agent-legacy"]');
  const geometry = await page.evaluate(() => {
    const span = document.querySelector('.reader-reflow-article [data-ref-path="1"] [data-text-index="0"]');
    const range = document.createRange(); range.setStart(span.firstChild, 0); range.setEnd(span.firstChild, 20);
    const expected = range.getBoundingClientRect();
    const mark = document.querySelector('.reader-reflow-highlight[data-annotation-id="agent-legacy"]').getBoundingClientRect();
    return { quote: range.toString(), delta: ['left', 'top', 'width', 'height'].map(key => Math.abs(mark[key] - expected[key])) };
  });
  assert.equal(geometry.quote, 'The invented sources');
  assert.ok(geometry.delta.every(delta => delta < 1), JSON.stringify(geometry));
  await page.evaluate(() => window.reader.close());
  assert.deepEqual(errors, []);
  console.log('PASS agent PDF annotations: legacy coordinates, source quote, comment editing, native and Reading Mode');
} catch (error) {
  console.error(error);
  throw error;
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, 'exit');
}
