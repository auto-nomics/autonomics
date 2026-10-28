// Regression for native PDF selections jumping to the page end in line gaps.
// Run with npm run test:pdf-selection; PDF_TEST_CHROMIUM overrides Chromium.
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

const origin = 'http://127.0.0.1:1423';
const server = await createServer({
  configFile: false, plugins: [react()], optimizeDeps: { entries: [] },
  resolve: { alias: { '@': fileURLToPath(new URL('../src', import.meta.url)) } },
  server: { host: '127.0.0.1', port: 1423, strictPort: true },
});
await server.listen();
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined),
    args: ['--no-sandbox'],
  });
  for (const rotation of [0, 90, 180, 270]) {
    const page = await browser.newPage({ viewport: { width: 1000, height: 1200 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body:
      '<html><style>*{box-sizing:border-box}</style><body></body></html>' }));
    await page.route('**/api/v1/**', route => route.request().url().endsWith('/fulltext/raw')
      ? route.fulfill({ contentType: 'application/pdf', body: fixture(rotation) })
      : route.fulfill({ json: { annotations: [] } }));
    await page.goto(origin);
    await page.evaluate(async () => {
      localStorage.setItem('autonomics.pdf.sidebarOpen', '0');
      // These isolated fixtures bypass Vite's HTML transform.
      const { default: RefreshRuntime } = await import('/@react-refresh');
      RefreshRuntime.injectIntoGlobalHook(window);
      window.$RefreshReg$ = () => {};
      window.$RefreshSig$ = () => type => type;
      window.__vite_plugin_react_preamble_installed__ = true;
      const { openPdfReader } = await import('/src/features/reader/pdf-reader.ts');
      window.reader = await openPdfReader({ articleId: 'selection', title: 'Selection regression' });
    });
    await page.waitForFunction(() => document.querySelectorAll('.reader-text-layer > span').length === 8
      && !document.querySelector('.reader-page-wrapper.rendering'));
    await page.waitForTimeout(250); // Settle initial ResizeObserver notification.

    const points = await page.locator('.reader-text-layer > span').evaluateAll(spans => spans.map(span => {
      const caret = offset => {
        const range = document.createRange();
        range.setStart(span.firstChild, offset); range.setEnd(span.firstChild, offset + 1);
        const box = range.getBoundingClientRect();
        return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
      };
      const box = span.getBoundingClientRect();
      return { start: caret(1), end: caret(8), left: box.left, top: box.top, right: box.right, bottom: box.bottom };
    }));
    const selectionText = () => page.evaluate(() => getSelection().toString());
    // Both forward and backward selection must stay on the same text while
    // repeatedly crossing into the whitespace immediately beside its line.
    for (const backwards of [false, true]) {
      await page.locator('[data-action="tool-clear"]').click();
      const start = points[backwards ? 2 : 0].start;
      const end = points[1].end;
      await page.mouse.move(start.x, start.y);
      await page.mouse.down();
      await page.mouse.move(end.x, end.y, { steps: 12 });
      await page.waitForTimeout(30);
      const expected = await selectionText();
      assert.ok(expected.includes('beta gamma delta epsilon'), `Drag must span text lines: rotation=${rotation}, backwards=${backwards}, text=${JSON.stringify(expected)}`);
      assert.ok(!expected.includes('Line 4'), 'Drag must not select distant lines');
      const gap = { ...end };
      const side = backwards ? 1 : -1;
      if (rotation === 0) gap.y = side < 0 ? points[1].top - 3 : points[1].bottom + 3;
      if (rotation === 90) gap.x = side < 0 ? points[1].right + 3 : points[1].left - 3;
      if (rotation === 180) gap.y = side < 0 ? points[1].bottom + 3 : points[1].top - 3;
      if (rotation === 270) gap.x = side < 0 ? points[1].left - 3 : points[1].right + 3;
      for (let step = 0; step < 8; step++) {
        const point = step % 2 ? end : gap;
        await page.mouse.move(point.x, point.y);
        await page.waitForTimeout(25);
        assert.equal(await selectionText(), expected,
          `Selection jumped in line gap: rotation=${rotation}, backwards=${backwards}, step=${step}`);
        assert.equal(await page.locator('.reader-selection-popup').isVisible(), false,
          'Selection popup must wait until the drag ends');
      }
      await page.mouse.up();
      await page.waitForTimeout(50);
      assert.equal(await selectionText(), expected, 'Releasing the pointer must preserve selected text');
      assert.equal(await page.locator('.reader-page-area').evaluate(el => el.classList.contains('reader-selecting')), false);
    }
    // Redrawing after zoom must register the new text layer, not retain old ones.
    await page.locator('[data-action="zoom-in"]').click();
    await page.waitForFunction(() => !document.querySelector('.reader-page-wrapper.rendering'));
    assert.equal(await page.locator('.reader-text-boundary').count(), 1);
    await page.evaluate(() => window.reader.close());
    assert.equal(await page.locator('.zotero-pdf-reader').count(), 0);
    assert.deepEqual(errors, []);
    await page.close();
    console.log(`PASS rotation=${rotation}: forward/backward gap drag, release, zoom, cleanup`);
  }
} finally {
  await browser?.close();
  await server.close();
}
