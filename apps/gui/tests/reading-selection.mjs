// A PDF rectangle -> text range conversion is lossy without character maps.
// Check painted geometry against the ORIGINAL DOM selection, not the mapper's
// own inverse, which used to expand a short quote to several whole paragraphs.
import { createServer } from 'vite';
import { chromium } from 'playwright';
import { readFileSync, existsSync } from 'node:fs';
import assert from 'node:assert/strict';

const pdf = readFileSync(new URL('../../../tools/zotero-extractor/test/fixtures/synthetic-list-links.pdf', import.meta.url));
const original = JSON.parse(readFileSync(new URL('../../../tools/zotero-extractor/test/fixtures/synthetic-list-links.json', import.meta.url)));
let structure = structuredClone(original);
// Deliberately ambiguous rectangles shared by multiple paragraphs: this is the
// extractor fallback that made the previous implementation paint extra text.
const coarsen = node => {
  if (typeof node.text === 'string') node.anchor = { pageRects: [[0, 40, 550, 560, 740]] };
  node.content?.forEach(coarsen);
};
structure.content.slice(0, 4).forEach(coarsen);
let annotations = [];
const server = await createServer({ configFile: false, optimizeDeps: { entries: [] }, server: { host: '127.0.0.1', port: 1426, strictPort: true } });
await server.listen();
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1100, height: 1000 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.route('http://127.0.0.1:1426/', route => route.fulfill({ contentType: 'text/html', body: '<html><style>*{box-sizing:border-box}</style><body></body></html>' }));
  await page.route('**/api/v1/**', route => {
    const request = route.request();
    if (request.url().endsWith('/fulltext/raw')) return route.fulfill({ contentType: 'application/pdf', body: pdf });
    if (request.url().endsWith('/fulltext/structured')) return route.fulfill({ json: { structure } });
    if (request.method() === 'POST') {
      const annotation = { ...request.postDataJSON(), id: String(annotations.length) };
      annotations.push(annotation); return route.fulfill({ status: 201, json: { annotation } });
    }
    return route.fulfill({ json: { annotations } });
  });
  await page.goto('http://127.0.0.1:1426/');
  const open = async () => {
    const restoresReadingMode = await page.evaluate(async () => {
      localStorage.setItem('autonomics.pdf.sidebarOpen', '0');
      const restores = JSON.parse(localStorage.getItem('autonomics.pdf.readingLocation.reading-selection') ?? 'null')?.enabled;
      const { openPdfReader } = await import('/src/pdf-reader.ts');
      window.reader = await openPdfReader({ articleId: 'reading-selection', title: 'Precise partial highlights' });
      return restores;
    });
    if (!restoresReadingMode) await page.locator('[data-action=reading-mode]').click();
    await page.waitForSelector('.reader-reflow-article');
    await page.waitForFunction(() => !document.querySelector('[data-action=reading-mode]').disabled);
  };
  const makeRange = async (start, end, select = false) => {
    await page.locator(`[data-ref-path="${start.slice(0, -2).join('.')}"]`).scrollIntoViewIfNeeded();
    await page.waitForTimeout(60);
    return page.evaluate(({ start, end, select }) => {
    const point = value => {
      const block = document.querySelector(`[data-ref-path="${value.slice(0, -2).join('.')}"]`);
      const span = block.querySelector(`[data-text-index="${value.at(-2)}"]`);
      const walker = document.createTreeWalker(span, NodeFilter.SHOW_TEXT);
      let left = value.at(-1);
      while (walker.nextNode()) { if (left <= walker.currentNode.length) return [walker.currentNode, left]; left -= walker.currentNode.length; }
      throw new Error('Invalid test text offset');
    };
    const range = document.createRange(); range.setStart(...point(start)); range.setEnd(...point(end));
    if (select) {
      getSelection().removeAllRanges(); getSelection().addRange(range);
      document.querySelector('.reader-reflow').dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
    }
    return { text: range.toString().replace(/\s+/g, ' ').trim(), rects: [...range.getClientRects()].filter(rect => rect.width > 0 && rect.height > 0).map(rect => ({ left: rect.left, top: rect.top, width: rect.width, height: rect.height })) };
    }, { start, end, select });
  };
  const comparePaint = async (start, end) => {
    await page.waitForTimeout(150);
    const expected = await makeRange(start, end);
    const marks = await page.locator('.reader-reflow-highlight').evaluateAll(elements => elements.map(element => {
      const rect = element.getBoundingClientRect(); return { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
    }));
    assert.equal(marks.length, expected.rects.length, `Extra painted text outside original selection: ${JSON.stringify({ marks, expected })}`);
    for (let i = 0; i < marks.length; i++) for (const key of ['left', 'top', 'width', 'height']) {
      assert.ok(Math.abs(marks[i][key] - expected.rects[i][key]) < 1, `Paint/selection mismatch at ${i}.${key}`);
    }
  };
  const start = [1, 0, 8], end = [1, 2, 17];
  await open();
  const selected = await makeRange(start, end, true);
  await page.waitForFunction(() => !document.querySelector('.reader-selection-popup').hidden);
  await page.locator('[data-color=yellow]').click();
  await page.waitForTimeout(250);
  await page.waitForSelector('.reader-reflow-highlight');
  assert.equal(annotations[0].content, selected.text);
  assert.deepEqual(annotations[0].rects[0].text_range.start, start);
  assert.deepEqual(annotations[0].rects[0].text_range.end, end);
  await comparePaint(start, end);
  await page.locator('[data-action=text-options]').click();
  await page.locator('[data-appearance=fontSize]').fill('26');
  await page.locator('[data-appearance=pageWidth]').selectOption('narrow');
  await page.locator('[data-appearance=close]').click();
  await comparePaint(start, end);
  await page.evaluate(() => window.reader.close()); await open(); await comparePaint(start, end);
  // Existing annotations and older backend versions have no semantic selector.
  // Their saved quote still limits the display to exactly the selected words.
  delete annotations[0].rects[0].text_range;
  await page.evaluate(() => window.reader.close()); await open(); await comparePaint(start, end);
  // References can change after re-extraction: reject an outdated exact range
  // whose text does not match, and use the quote rather than a larger paragraph.
  annotations[0].rects[0].text_range = { source_hash: structure.metadata.source.hash, start: [1, 0, 0], end: [3, 0, 25], quote: selected.text };
  await page.evaluate(() => window.reader.close()); await open(); await comparePaint(start, end);
  // Identical phrases in the same coarse paragraph must preserve WHICH instance
  // the user selected, using saved semantic offsets rather than first-match text.
  await page.evaluate(() => window.reader.close());
  structure = structuredClone(original);
  const text = 'An identical phrase. An identical phrase. A following sentence.';
  structure.content[1].content = [{ text, anchor: { pageRects: [[0, 40, 550, 560, 740]] } }];
  annotations = [];
  await open();
  const second = text.indexOf('An identical phrase.', 1);
  const duplicateStart = [1, 0, second], duplicateEnd = [1, 0, second + 'An identical phrase.'.length];
  await makeRange(duplicateStart, duplicateEnd, true);
  await page.waitForFunction(() => !document.querySelector('.reader-selection-popup').hidden);
  await page.locator('[data-color=yellow]').click(); await page.waitForSelector('.reader-reflow-highlight');
  await comparePaint(duplicateStart, duplicateEnd);
  await page.evaluate(() => window.reader.close()); await open(); await comparePaint(duplicateStart, duplicateEnd);
  // A two-page highlight stores disjoint semantic subranges on its two records.
  await page.evaluate(() => window.reader.close());
  structure = structuredClone(original); annotations = [];
  await open();
  const acrossStart = [3, 0, 10], acrossEnd = [7, 1, 15];
  await makeRange(acrossStart, acrossEnd, true);
  await page.waitForFunction(() => !document.querySelector('.reader-selection-popup').hidden);
  await page.locator('[data-color=yellow]').click();
  await page.waitForFunction(() => document.querySelector('.app-toast-message')?.textContent === 'Highlight saved');
  assert.equal(annotations.length, 2);
  const firstRange = annotations[0].rects[0].text_range, secondRange = annotations[1].rects[0].text_range;
  assert.deepEqual(firstRange.start, acrossStart); assert.deepEqual(secondRange.end, acrossEnd);
  assert.ok(firstRange.end[0] < secondRange.start[0], 'Per-page selectors must not duplicate the entire cross-page selection');
  assert.deepEqual(errors, []);
  console.log('PASS exact partial/inline ranges, coarse PDF maps, typography reflow, reopen, legacy quotes, stale selectors identical repeated phrases and two-page selections');
} finally { await browser?.close(); await server.close(); }
