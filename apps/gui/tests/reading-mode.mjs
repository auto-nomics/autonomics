import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, readFileSync } from 'node:fs';
import { extractPDF } from '../../../tools/zotero-extractor/extract.mjs';
import { PDFPositionMapper } from '../src/vendor/zotero-sdt.generated.js';

const pdf = readFileSync(new URL('../../../tools/zotero-extractor/test/fixtures/synthetic-list-links.pdf', import.meta.url));
const { structure } = await extractPDF(pdf);
let activePDF = pdf, activeStructure = structure;
let origin = '';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '0', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
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
    window.reader = await openPdfReader({ articleId, title: 'Synthetic reference links' });
  }, articleId);
  await open();
  const reading = () => page.waitForSelector('.zotero-pdf-reader[data-reading-mode=true] .reader-reflow-article');
  await page.locator('[data-action=reading-mode]').click();
  await reading();
  assert.ok(await page.locator('.reader-reflow-article p').count() >= 5);
  assert.ok(await page.locator('.reader-reflow-article li').count() >= 6);
  assert.equal(await page.locator('.reader-reflow-article [data-ref-path="1"]').textContent(), 'The invented sources [1-6] illustrate independent reference items. Compare (2).');
  assert.equal(await page.locator('.reader-page-area').isVisible(), false);
  const originalText = await page.locator('.reader-reflow-article').textContent();
  await page.locator('[data-action=text-options]').click();
  await page.locator('[data-appearance=fontSize]').fill('26');
  await page.locator('[data-appearance=pageWidth]').selectOption('narrow');
  await page.locator('[data-appearance-theme=sepia]').click();
  assert.equal(await page.locator('.reader-reflow-article').evaluate(element => getComputedStyle(element).fontSize), '26px');
  assert.equal(await page.locator('.reader-reflow-article').evaluate(element => getComputedStyle(element).maxWidth), '650px');
  assert.equal(await page.locator('.reader-reflow').getAttribute('data-reading-theme'), 'sepia');
  assert.equal(await page.locator('.reader-reflow-article').textContent(), originalText);
  await page.locator('[data-appearance=close]').click();
  // Select only the first 20 characters of a semantic text node. Its PDF
  // coordinates must match Zotero's character map, independently of layout.
  await page.locator('.reader-reflow-article [data-ref-path="1"] [data-text-index="0"]').scrollIntoViewIfNeeded();
  await page.waitForTimeout(50);
  await page.evaluate(() => {
    const span = document.querySelector('.reader-reflow-article [data-ref-path="1"] [data-text-index="0"]');
    const range = document.createRange(); range.setStart(span.firstChild, 0); range.setEnd(span.firstChild, 20);
    getSelection().removeAllRanges(); getSelection().addRange(range);
    document.querySelector('.reader-reflow').dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
  });
  await page.waitForFunction(() => !document.querySelector('.reader-selection-popup').hidden);
  await page.locator('[data-color]').first().click();
  await page.waitForSelector('.reader-reflow-highlight');
  assert.equal(annotations[0].content, 'The invented sources');

  assert.equal(annotations[0].page, 1);
  const expected = new PDFPositionMapper(structure).sdtToSourcePosition({ start: [1, 0, 0], end: [1, 0, 20] });
  const rect = annotations[0].rects[0];
  assert.equal(rect.space, 'pdf');
  assert.ok(Math.abs(rect.left * 612 - expected.rects[0][0]) < .01);
  assert.ok(Math.abs(rect.top * 792 - expected.rects[0][1]) < .01);
  // Click painted text itself. Highlight overlays must remain transparent to
  // native text selection and must open details rather than delete anything.
  const clickMark = async selector => {
    const mark = page.locator(selector).first();
    await mark.scrollIntoViewIfNeeded();
    const box = await mark.boundingBox();
    await page.mouse.click(box.x + Math.min(8, box.width / 2), box.y + box.height / 2);
  };
  await clickMark('.reader-reflow-highlight');
  const inspector = page.getByRole('dialog', { name: 'Annotation details' });
  await inspector.waitFor();
  await page.waitForSelector('.reader-annotation-outline');
  assert.equal(annotations.length, 1, 'Clicking a highlight must not delete it');
  assert.equal(await inspector.getAttribute('data-slot'), 'hover-card-content');
  await inspector.hover();
  await page.mouse.move(10, 10);
  await page.waitForTimeout(300);
  assert.ok(await inspector.isVisible(), 'Clicked Hover Card stays pinned after leaving it');
  await page.screenshot({ path: '/tmp/autonomics-annotation-popup.png' });
  await inspector.getByRole('textbox', { name: 'Annotation comment' }).fill('A comment separate from the source quote');
  await page.waitForFunction(() => document.querySelector('.reader-annotation-save-status').textContent === 'Saved');
  assert.equal(annotations[0].comment, 'A comment separate from the source quote');
  await page.evaluate(() => window.dispatchEvent(new Event('resize')));
  await page.waitForTimeout(100);
  assert.ok(await inspector.getByRole('textbox', { name: 'Annotation comment' }).evaluate(node => node === document.activeElement), 'Repositioning keeps the comment focused');
  const cardBounds = await inspector.boundingBox();
  assert.ok(cardBounds.x >= 8 && cardBounds.y >= 8 && cardBounds.x + cardBounds.width <= 1392 && cardBounds.y + cardBounds.height <= 992, 'Hover Card fits within the viewport');
  assert.equal(annotations[0].content, 'The invented sources');
  await inspector.getByRole('textbox', { name: 'Add annotation tags' }).fill('brain, methods');
  await inspector.getByRole('textbox', { name: 'Add annotation tags' }).press('Enter');
  await page.waitForFunction(() => document.querySelector('.reader-annotation-save-status').textContent === 'Saved');
  assert.deepEqual(annotations[0].tags, ['brain', 'methods']);
  await inspector.getByRole('button', { name: 'Annotation options' }).click();
  await inspector.getByRole('button', { name: 'Color #ff6666' }).click();
  await page.waitForFunction(() => document.querySelector('.reader-annotation-save-status').textContent === 'Saved');
  assert.equal(annotations[0].color, '#ff6666');
  // A failed edit stays recoverable and can be retried without changing quote.
  failPatch = true;
  await inspector.getByRole('textbox', { name: 'Annotation comment' }).fill('Retried annotation comment');
  await inspector.getByRole('button', { name: 'Retry' }).waitFor();
  failPatch = false;
  await inspector.getByRole('button', { name: 'Retry' }).click();
  await page.waitForFunction(() => document.querySelector('.reader-annotation-save-status').textContent === 'Saved');
  assert.equal(annotations[0].comment, 'Retried annotation comment');
  await inspector.getByRole('textbox', { name: 'Add annotation tags' }).fill('draft-tag');
  await page.keyboard.press('Escape');
  await inspector.waitFor({ state: 'hidden' });
  await clickMark('.reader-reflow-highlight');
  await inspector.waitFor();
  assert.equal(await inspector.getByRole('textbox', { name: 'Annotation comment' }).inputValue(), 'Retried annotation comment');
  assert.ok(await inspector.getByRole('button', { name: 'Remove tag brain' }).isVisible());
  assert.ok(await inspector.getByRole('button', { name: 'Remove tag draft-tag' }).isVisible(), 'Closing commits unfinished tag input');
  // A real drag over highlighted text must still produce a native selection.
  const spanBox = await page.locator('.reader-reflow-article [data-ref-path="1"] [data-text-index="0"]').boundingBox();
  await page.mouse.move(spanBox.x + 2, spanBox.y + spanBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(spanBox.x + spanBox.width - 2, spanBox.y + spanBox.height / 2, { steps: 8 });
  await page.mouse.up();
  assert.ok(await page.evaluate(() => !getSelection().isCollapsed));
  await inspector.waitFor({ state: 'hidden' });
  await page.evaluate(() => getSelection().removeAllRanges());
  // A multiline reflowed highlight gets one enclosing dashed selection box.
  await page.evaluate(() => {
    const paragraph = document.querySelector('.reader-reflow-article [data-ref-path="1"]');
    const range = document.createRange(); range.selectNodeContents(paragraph);
    getSelection().removeAllRanges(); getSelection().addRange(range);
    document.querySelector('.reader-reflow').dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
  });
  await page.waitForFunction(() => !document.querySelector('.reader-selection-popup').hidden);
  await page.locator('[data-color]').first().click();
  await page.waitForSelector('.reader-reflow-highlight[data-annotation-id="1"]');
  await clickMark('.reader-reflow-highlight[data-annotation-id="1"]:last-of-type');
  await inspector.waitFor();
  const multiline = await page.evaluate(() => {
    const rects = [...document.querySelectorAll('.reader-reflow-highlight[data-annotation-id="1"]')].map(mark => mark.getBoundingClientRect());
    const box = document.querySelector('.reader-annotation-outline').getBoundingClientRect();
    return { lines: new Set(rects.map(rect => Math.round(rect.top))).size,
      covers: rects.every(rect => box.left <= rect.left && box.right >= rect.right && box.top <= rect.top && box.bottom >= rect.bottom) };
  });
  assert.ok(multiline.lines > 1 && multiline.covers, 'Selected border must enclose all highlighted lines');
  await page.screenshot({ path: '/tmp/autonomics-multiline-annotation.png' });
  // Destructive actions are explicit options, never a plain highlight click.
  await inspector.getByRole('button', { name: 'Annotation options' }).click();
  await inspector.getByRole('button', { name: 'Delete annotation', exact: true }).click();
  const confirmDialog = page.getByRole('alertdialog', { name: 'Delete this annotation?' });
  await confirmDialog.waitFor();
  await confirmDialog.getByRole('button', { name: 'Delete', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('.reader-reflow-highlight[data-annotation-id="1"]'));
  assert.equal(annotations.length, 1);
  // Typography changes reproject the same annotation onto the new text layout.
  await page.locator('[data-action=text-options]').click();
  await page.locator('[data-appearance=fontSize]').fill('18');
  await page.locator('[data-appearance=pageWidth]').selectOption('normal');
  await page.locator('[data-appearance-theme=light]').click();
  await page.locator('[data-appearance=close]').click();
  await page.waitForSelector('.reader-reflow-highlight');
  await page.screenshot({ path: '/tmp/autonomics-reading-mode.png' });
  await page.locator('[data-action=reading-mode]').click();
  await page.waitForFunction(() => !document.querySelector('.reader-page-area').hidden && !document.querySelector('[data-action=reading-mode]').disabled);
  // Returning to PDF mode changes the viewport and schedules a debounced refit.
  // Wait for that render rather than clicking a transient annotation node.
  await page.waitForTimeout(250);
  await page.waitForFunction(() => !document.querySelector('.reader-page-wrapper.rendering'));
  await page.waitForSelector('.reader-annotation');
  assert.equal(await page.locator('.reader-reflow-article').count(), 0);
  await clickMark('.reader-annotation');
  await inspector.waitFor();
  assert.equal(await inspector.getByRole('textbox', { name: 'Annotation comment' }).inputValue(), 'Retried annotation comment');
  await page.keyboard.press('Escape');

  // The materialized SDT is reused for subsequent toggles.
  await page.locator('[data-action=reading-mode]').click();
  await reading();
  assert.equal(structureRequests, 1);
  await page.locator('[data-input=page]').fill('2');
  await page.locator('[data-input=page]').dispatchEvent('change');
  await page.waitForTimeout(350);
  assert.equal(await page.locator('[data-input=page]').inputValue(), '2');
  await page.evaluate(() => window.reader.close());
  await open();
  await reading();
  assert.equal(await page.locator('[data-input=page]').inputValue(), '2');
  assert.equal(await page.locator('.reader-reflow-article').evaluate(element => getComputedStyle(element).fontSize), '18px');
  // A failed conversion leaves the original PDF usable, and a later retry works.
  await page.evaluate(() => { window.reader.close(); localStorage.removeItem('autonomics.pdf.readingLocation.reading-test'); });
  fail = true;
  await open();
  const failedConversion = page.waitForResponse(response => response.url().endsWith('/fulltext/structured') && response.status() === 503);
  await page.locator('[data-action=reading-mode]').click();
  await failedConversion;
  await page.waitForFunction(() => !document.querySelector('[data-action=reading-mode]').disabled);
  assert.equal(await page.locator('.reader-page-area').isVisible(), true);
  fail = false;
  await page.locator('[data-action=reading-mode]').click();
  await reading();
  // Toolbar tools work in semantic Reading Mode as well as original PDF mode.
  await page.locator('[data-action=tool-clear]').click();
  const beforeTools = annotations.length;
  const beforeToolMarks = await page.locator('.reader-reflow-highlight').count();
  await page.locator('[data-action=tool-highlight]').click();
  await page.locator('.reader-reflow-article [data-ref-path="1"]').scrollIntoViewIfNeeded();
  await page.evaluate(() => {
    const span = document.querySelector('.reader-reflow-article [data-ref-path="1"] [data-text-index="0"]');
    const range = document.createRange(); range.setStart(span.firstChild, 0); range.setEnd(span.firstChild, 20);
    getSelection().removeAllRanges(); getSelection().addRange(range);
    document.querySelector('.reader-reflow').dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
  });
  await page.waitForFunction(count => document.querySelectorAll('.reader-reflow-highlight').length > count, beforeToolMarks);
  assert.equal(annotations.length, beforeTools + 1);
  assert.equal(annotations.at(-1).kind, 'highlight');
  await page.locator('[data-action=tool-clear]').click();
  await page.locator('[data-action=tool-note]').click();
  const notePoint = await page.evaluate(() => {
    const span = document.querySelector('.reader-reflow-article [data-ref-path="1"] [data-text-index="0"]');
    const range = document.createRange(); range.setStart(span.firstChild, 8); range.setEnd(span.firstChild, 9);
    const rect = range.getBoundingClientRect(); return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  });
  await page.mouse.click(notePoint.x, notePoint.y);
  await page.getByRole('textbox', { name: 'Note text', exact: true }).fill('A semantic Reading Mode note');
  await page.getByRole('button', { name: 'Save note', exact: true }).click();
  await page.waitForSelector('.reader-reflow-highlight.reader-note-marker');
  assert.equal(annotations.length, beforeTools + 2);
  assert.equal(annotations.at(-1).kind, 'comment');
  assert.equal(annotations.at(-1).rects[0].space, 'pdf');
  // Real four-page figure/table fixture: crops use original PDF rendering,
  // not an empty image placeholder or flattened mathematical text.
  await page.evaluate(() => window.reader.close());
  activePDF = readFileSync(new URL('../../../tools/zotero-extractor/test/fixtures/paint-scene-pathologies.pdf', import.meta.url));
  activeStructure = (await extractPDF(activePDF)).structure;
  annotations = [];
  await open('reading-crops');
  await page.locator('[data-action=reading-mode]').click();
  await reading();
  await page.locator('.sdt-source-crop').first().scrollIntoViewIfNeeded();
  await page.waitForSelector('.sdt-source-crop canvas');
  assert.ok(await page.locator('.sdt-source-crop canvas').first().evaluate(canvas => {
    const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
    for (let index = 0; index < pixels.length; index += 4) if (pixels[index + 3] > 200 && pixels[index] < 200) return true;
    return false;
  }), 'Source crop must contain the actual painted PDF region');
  const sourceImage = page.locator('.sdt-source-crop canvas').first();
  await sourceImage.scrollIntoViewIfNeeded();
  const inlinePixels = await sourceImage.evaluate(canvas => canvas.width);
  const readingScroll = await page.locator('.reader-reflow').evaluate(element => element.scrollTop);
  await sourceImage.click();
  const preview = page.getByRole('dialog', { name: 'Image preview' });
  await preview.waitFor();
  await page.waitForFunction(() => !document.querySelector('.reader-image-preview').hasAttribute('aria-busy'));
  const enlarged = preview.locator('canvas');
  assert.ok(await enlarged.evaluate(canvas => canvas.width) > inlinePixels, 'Preview rerenders the PDF region at higher resolution');
  const initialZoom = parseInt(await preview.locator('output').textContent());
  await preview.getByRole('button', { name: 'Zoom in', exact: true }).click();
  assert.ok(parseInt(await preview.locator('output').textContent()) > initialZoom);
  await preview.getByRole('button', { name: '100%', exact: true }).click();
  assert.equal(await preview.locator('output').textContent(), '100%');
  const imageViewport = preview.locator('.reader-image-preview-viewport');
  await imageViewport.hover();
  await page.mouse.wheel(0, -200);
  await page.waitForFunction(() => parseInt(document.querySelector('.reader-image-preview output').value) > 100);
  // Zoom until the image extends past the viewport, then pan with a real drag.
  for (let index = 0; index < 10; index++) await preview.getByRole('button', { name: 'Zoom in', exact: true }).evaluate(button => button.click());
  const area = await imageViewport.boundingBox();
  const beforePan = await imageViewport.evaluate(element => ({ left: element.scrollLeft, top: element.scrollTop }));
  await page.mouse.move(area.x + area.width / 2, area.y + area.height / 2);
  await page.mouse.down();
  await page.mouse.move(area.x + area.width / 2 - 100, area.y + area.height / 2 - 70, { steps: 5 });
  await page.mouse.up();
  const afterPan = await imageViewport.evaluate(element => ({ left: element.scrollLeft, top: element.scrollTop }));
  assert.ok(afterPan.left > beforePan.left || afterPan.top > beforePan.top, 'Dragging pans the enlarged image');
  await preview.getByRole('button', { name: 'Fit to window' }).click();
  assert.ok(await imageViewport.evaluate(element => element.scrollWidth <= element.clientWidth + 1 && element.scrollHeight <= element.clientHeight + 1));
  await page.screenshot({ path: '/tmp/autonomics-reading-image-preview.png' });
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('.reader-image-preview').count(), 0);
  assert.equal(await page.locator('.reader-reflow').evaluate(element => element.scrollTop), readingScroll);
  assert.equal(await sourceImage.evaluate(element => element === document.activeElement), true);
  // Images are keyboard accessible; the backdrop and explicit close both work.
  await page.keyboard.press('Enter');
  await preview.waitFor();
  await preview.getByRole('button', { name: 'Close image preview' }).click();
  await sourceImage.click();
  await preview.waitFor();
  await page.mouse.click(2, 2);
  assert.equal(await page.locator('.reader-image-preview').count(), 0);
  // Closing the reader during the higher-resolution render cancels the preview.
  await sourceImage.click();
  await preview.waitFor();
  await page.evaluate(() => window.reader.close());
  assert.equal(await page.locator('.reader-image-preview').count(), 0);
  assert.deepEqual(errors, []);
  console.error('PASS Reading Mode: real Zotero SDT, semantic blocks, appearance, exact annotation mapping, toggles, location restore, failure/retry, image preview/zoom/pan/cleanup');
} catch (error) {
  console.error(error);
  throw error;
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, 'exit');
}
