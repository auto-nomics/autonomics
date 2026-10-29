import { createServer } from 'vite';
import react from '@vitejs/plugin-react';
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

function fixture(rotation) {
  const stream = ['BT /F1 20 Tf', ...Array.from({ length: 8 }, (_, i) =>
    `1 0 0 1 82 ${700 - i * 32} Tm (Line ${i + 1} alpha beta gamma delta epsilon) Tj`), 'ET'].join('\n');
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Rotate ${rotation} /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>`,
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
    `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`,
  ];
  let pdf = '%PDF-1.4\n';
  const offsets = objects.map((object, index) => {
    const offset = pdf.length;
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
    return offset;
  });
  const xref = pdf.length;
  pdf += 'xref\n0 6\n0000000000 65535 f \n';
  pdf += offsets.map(offset => `${String(offset).padStart(10, '0')} 00000 n \n`).join('');
  return Buffer.from(pdf + `trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`);
}

const origin = 'http://127.0.0.1:1425';
const server = await createServer({ configFile: false, plugins: [react()], resolve: { alias: { '@': fileURLToPath(new URL('../src', import.meta.url)) } }, optimizeDeps: { entries: [] }, server: { host: '127.0.0.1', port: 1425, strictPort: true } });
await server.listen();
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1100, height: 1000 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  const annotations = []; let fail = false;
  await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html><style>*{box-sizing:border-box}</style><body></body></html>' }));
  await page.route('**/api/v1/**', route => {
    const request = route.request();
    if (request.url().endsWith('/fulltext/raw')) return route.fulfill({ contentType: 'application/pdf', body: fixture(0) });
    if (request.method() === 'POST') {
      if (fail) return route.fulfill({ status: 503, json: { error: 'Temporary failure' } });
      const annotation = { ...request.postDataJSON(), id: String(annotations.length) };
      annotations.push(annotation); return route.fulfill({ status: 201, json: { annotation } });
    }
    return route.fulfill({ json: { annotations } });
  });
  await page.goto(origin);
  const open = async () => {
    await page.evaluate(async () => {
      localStorage.setItem('autonomics.pdf.sidebarOpen', '0');
      // These isolated fixtures bypass Vite's HTML transform.
      const { default: RefreshRuntime } = await import('/@react-refresh');
      RefreshRuntime.injectIntoGlobalHook(window);
      window.$RefreshReg$ = () => {};
      window.$RefreshSig$ = () => type => type;
      window.__vite_plugin_react_preamble_installed__ = true;
      const { openPdfReader } = await import('/src/features/reader/pdf-reader.ts');
      window.reader = await openPdfReader({ articleId: 'tools', title: 'Annotation tools' });
    });
    await page.waitForFunction(() => document.querySelectorAll('.reader-text-layer > span').length === 8 && !document.querySelector('.reader-page-wrapper.rendering'));
    await page.waitForTimeout(250);
  };
  await open();
  const selectLine = async () => {
    await page.evaluate(() => {
      const span = document.querySelector('.reader-text-layer span');
      const range = document.createRange(); range.selectNodeContents(span);
      const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
      span.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
    });
    await page.waitForTimeout(100);
  };
  const clear = () => page.locator('[data-action=tool-clear]').click();
  const highlight = page.locator('[data-action=tool-highlight]');
  const note = page.locator('[data-action=tool-note]');
  await highlight.click();
  assert.equal(await highlight.getAttribute('aria-pressed'), 'true');
  assert.equal(annotations.length, 0, 'Activating highlighter must wait for a drag');
  const span = await page.locator('.reader-text-layer > span').first().boundingBox();
  await page.mouse.move(span.x + 3, span.y + span.height / 2); await page.mouse.down();
  await page.mouse.move(span.x + span.width - 3, span.y + span.height / 2, { steps: 12 }); await page.mouse.up();
  await page.waitForSelector('.reader-annotation');
  assert.equal(annotations.length, 1); assert.equal(annotations[0].kind, 'highlight');
  assert.equal(annotations[0].color, '#ffd400'); assert.match(annotations[0].content, /Line 1/);
  assert.equal(annotations[0].rects[0].space, 'pdf');
  assert.equal(await highlight.getAttribute('aria-pressed'), 'true', 'Highlighter stays active for repeated use');
  await highlight.click(); await selectLine(); assert.equal(annotations.length, 1, 'Pointer mode only selects');
  await highlight.click(); await page.waitForFunction(() => document.querySelectorAll('.reader-annotation').length === 2);
  assert.equal(annotations.length, 2, 'Toolbar applies to an existing selection');
  await clear(); await note.click();
  assert.equal(await note.getAttribute('aria-pressed'), 'true');
  assert.equal(await highlight.getAttribute('aria-pressed'), 'false');
  const box = await page.locator('.reader-page-wrapper').boundingBox();
  const point = { x: box.x + 40, y: box.y + 80 };
  await page.mouse.click(point.x, point.y);
  await page.waitForSelector('.reader-note-composer:not([hidden])');
  assert.equal(await note.getAttribute('aria-pressed'), 'false');
  assert.equal(await page.locator('.reader-note-composer [type=submit]').isEnabled(), false, 'Empty notes cannot be saved');
  await page.locator('[data-action=cancel-note]').click(); assert.equal(annotations.length, 2);
  await note.click(); await page.mouse.click(point.x, point.y);
  await page.getByRole('textbox', { name: 'Note text', exact: true }).fill('A note placed on the page <safe>');
  fail = true; await page.getByRole('button', { name: 'Save note', exact: true }).click();
  await page.waitForSelector('.app-toast[data-kind=error]');
  assert.equal(await page.getByRole('textbox', { name: 'Note text', exact: true }).inputValue(), 'A note placed on the page <safe>');
  assert.equal(annotations.length, 2, 'Failed note retains draft without creating a mark');
  fail = false; await page.getByRole('button', { name: 'Save note', exact: true }).click();
  await page.waitForSelector('.reader-note-marker');
  assert.equal(annotations.length, 3); assert.equal(annotations[2].kind, 'comment'); assert.equal(annotations[2].page, 1);
  const pin = await page.locator('.reader-note-marker').boundingBox();
  assert.ok(Math.abs(pin.x + pin.width / 2 - point.x) < 1 && Math.abs(pin.y + pin.height / 2 - point.y) < 1, 'Pin must be placed at the clicked PDF coordinate');
  await page.waitForSelector('.reader-note-content');
  assert.equal(await page.locator('.reader-note-content').textContent(), annotations[2].content);
  // Failure feedback also preserves text for a successful highlighter retry.
  await clear(); fail = true; await highlight.click(); await selectLine();
  await page.waitForSelector('.app-toast[data-kind=error]');
  assert.equal(annotations.length, 3); assert.equal(await page.locator('.reader-selection-popup').isVisible(), true);
  fail = false; await page.locator('[data-color=yellow]').click();
  await page.waitForFunction(() => document.querySelectorAll('.reader-annotation').length === 4);
  assert.equal(annotations.length, 4);
  await page.keyboard.press('Escape'); assert.equal(await highlight.getAttribute('aria-pressed'), 'false');
  await page.evaluate(() => window.reader.close()); await open();
  assert.equal(await page.locator('.reader-annotation').count(), 4, 'Annotations survive reopening');
  assert.equal(await page.locator('.reader-note-marker').count(), 1);
  await page.locator('[data-action=tool-note]').click(); await page.keyboard.press('Escape');
  assert.equal(await note.getAttribute('aria-pressed'), 'false');
  assert.deepEqual(errors, []);
  console.log('PASS toolbar modes, real drag highlight, selected-text highlight, point note, cancellation, save retry, pin coordinates, persistence and Escape');
} finally { await browser?.close(); await server.close(); }
