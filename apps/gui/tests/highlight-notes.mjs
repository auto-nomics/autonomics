import { createServer } from 'vite';
import { chromium } from 'playwright';
import { readFileSync, existsSync } from 'node:fs';
import assert from 'node:assert/strict';
import { PDFPositionMapper } from '../src/zotero-sdt.generated.js';

const pdf = readFileSync(new URL('../../../tools/zotero-extractor/test/fixtures/synthetic-list-links.pdf', import.meta.url));
const structure = JSON.parse(readFileSync(new URL('../../../tools/zotero-extractor/test/fixtures/synthetic-list-links.json', import.meta.url)));
const position = new PDFPositionMapper(structure).sdtToSourcePosition({ start: [1, 0, 0], end: [1, 2, 20] });
const annotations = [{ id: 'commented', kind: 'highlight', content: 'The invented sources [1-6] illustrate independent', page: 1, color: '#ffd400', comment: 'A note on the highlighted passage', rects: position.rects.map(rect => ({ left: rect[0] / 612, top: rect[1] / 792, width: (rect[2] - rect[0]) / 612, height: (rect[3] - rect[1]) / 792, space: 'pdf' })) }, { id: 'plain', kind: 'highlight', content: 'The invented sources', page: 1, color: '#5fb236', comment: '  ', rects: [{ left: .1, top: .3, width: .1, height: .02, space: 'pdf' }] }];
const origin = 'http://127.0.0.1:1427';
const server = await createServer({ configFile: false, optimizeDeps: { entries: [] }, server: { host: '127.0.0.1', port: 1427, strictPort: true } });
await server.listen();
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html><style>*{box-sizing:border-box}</style><body></body></html>' }));
  await page.route('**/api/v1/**', route => {
    const request = route.request();
    if (request.url().endsWith('/fulltext/raw')) return route.fulfill({ contentType: 'application/pdf', body: pdf });
    if (request.url().endsWith('/fulltext/structured')) return route.fulfill({ json: { structure } });
    if (request.method() === 'PATCH') {
      const annotation = annotations.find(value => value.id === decodeURIComponent(request.url().split('/').at(-1)));
      Object.assign(annotation, request.postDataJSON());
      return route.fulfill({ json: { annotation } });
    }
    return route.fulfill({ json: { annotations } });
  });
  await page.goto(origin);
  await page.evaluate(async () => {
    localStorage.setItem('autonomics.pdf.sidebarOpen', '0');
    const { openPdfReader } = await import('/src/pdf-reader.ts');
    window.reader = await openPdfReader({ articleId: 'note-indicators', title: 'Highlight note indicators' });
  });
  await page.waitForSelector('.reader-page-area .reader-highlight-note-indicator');
  await page.waitForTimeout(250);
  const icon = page.locator('.reader-page-area .reader-highlight-note-indicator');
  assert.equal(await icon.count(), 1, 'One indicator for a commented highlight, none for whitespace-only comments');
  assert.equal(await icon.getAttribute('data-annotation-id'), 'commented');
  assert.equal(await icon.getAttribute('title'), annotations[0].comment);
  assert.equal(await icon.evaluate(element => getComputedStyle(element).opacity), '0.6');
  await icon.click();
  const inspector = page.getByRole('dialog', { name: 'Annotation details' });
  await inspector.waitFor();
  const comment = inspector.getByRole('textbox', { name: 'Annotation comment', exact: true });
  assert.equal(await comment.inputValue(), annotations[0].comment);
  // Clearing the note removes the indicator immediately after the edit saves.
  await comment.fill('');
  await page.waitForFunction(() => !document.querySelector('.reader-highlight-note-indicator'));
  await comment.fill('A restored highlight note');
  await page.waitForSelector('.reader-page-area .reader-highlight-note-indicator');
  await page.keyboard.press('Escape');
  await page.locator('[data-action=reading-mode]').click();
  await page.waitForSelector('.reader-reflow-article');
  await page.locator('.reader-reflow-article [data-ref-path="1"]').scrollIntoViewIfNeeded();
  const reflowIcon = page.locator('.reader-reflow .reader-highlight-note-indicator');
  await reflowIcon.waitFor();
  assert.equal(await reflowIcon.count(), 1);
  await reflowIcon.click(); await inspector.waitFor();
  assert.equal(await comment.inputValue(), 'A restored highlight note');
  await page.keyboard.press('Escape');
  await page.locator('[data-action=text-options]').click();
  await page.locator('[data-appearance=fontSize]').fill('25');
  await page.locator('[data-appearance=pageWidth]').selectOption('narrow');
  await page.locator('[data-appearance=close]').click();
  await page.waitForTimeout(200);
  assert.equal(await reflowIcon.count(), 1, 'Reflow must not duplicate the indicator');
  const placement = await reflowIcon.evaluate(element => {
    const box = element.getBoundingClientRect();
    const area = element.closest('.reader-reflow').getBoundingClientRect();
    return box.left >= area.left && box.right <= area.right && box.top >= area.top && box.bottom <= area.bottom;
  });
  assert.ok(placement, 'Indicator stays inside the reading viewport');
  await reflowIcon.click(); await inspector.waitFor(); await comment.fill('  ');
  await page.waitForFunction(() => !document.querySelector('.reader-highlight-note-indicator'));
  await page.evaluate(() => window.reader.close());
  assert.deepEqual(errors, []);
  console.log('PASS commented highlights: single translucent indicator, click-to-open, clearing/restoring comments, PDF/Reading Mode and typography reflow');
} finally { await browser?.close(); await server.close(); }
