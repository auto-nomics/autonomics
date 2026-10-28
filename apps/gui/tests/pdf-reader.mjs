// Run with npm run test:pdf. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';

function fixture(rotation) {
  const stream = 'BT /F1 20 Tf 1 0 0 1 82 700 Tm (Alignment regression test) Tj ET';
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    `<< /Type /Page /Parent 2 0 R /MediaBox [10 20 622 812] /Rotate ${rotation} /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>`,
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
  pdf += `trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`;
  return Buffer.from(pdf);
}

const origin = 'http://127.0.0.1:1421';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1421', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
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
  for (const dpr of [1, 1.5, 2]) {
    for (const rotation of [0, 90, 180, 270]) {
      // With the sidebar closed these widths produce exactly 100% fit width.
      const pageWidth = rotation % 180 ? 792 : 612;
      const page = await browser.newPage({ viewport: { width: pageWidth + 96, height: 1100 }, deviceScaleFactor: dpr });
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      let annotations = [];
      await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html lang="zh-CN"><body></body></html>' }));
      await page.route('**/api/v1/**', async route => {
        const request = route.request();
        if (request.url().endsWith('/fulltext/raw')) return route.fulfill({ contentType: 'application/pdf', body: fixture(rotation) });
        if (request.method() === 'POST') {
          const annotation = { ...request.postDataJSON(), id: String(annotations.length) };
          annotations.push(annotation);
          return route.fulfill({ status: 201, json: { annotation } });
        }
        await route.fulfill({ json: { annotations } });
      });
      await page.goto(origin);
      await page.evaluate(async () => {
        localStorage.setItem('autonomics.pdf.sidebarOpen', '0');
        await import('/src/app.css');
        // These isolated fixtures bypass Vite's HTML transform.
        const { default: RefreshRuntime } = await import('/@react-refresh');
        RefreshRuntime.injectIntoGlobalHook(window);
        window.$RefreshReg$ = () => {};
        window.$RefreshSig$ = () => type => type;
        window.__vite_plugin_react_preamble_installed__ = true;
        const { openPdfReader } = await import('/src/features/reader/pdf-reader.ts');
        await openPdfReader({ articleId: 'regression', title: 'Alignment regression' });
      });
      const ready = () => page.waitForFunction(() => document.querySelector('.reader-text-layer span') && !document.querySelector('.reader-page-wrapper.rendering'));
      await ready();
      // Allow the initial ResizeObserver notification to settle.
      await page.waitForTimeout(200);
      await ready();
      assert.equal(await page.locator('[data-output=zoom]').textContent(), '100%');
      for (const zoomed of [false, true]) {
        if (zoomed) {
          await page.locator('[data-action=zoom-in]').evaluate(button => button.click());
          await ready();
        }
        const geometry = await page.evaluate(() => {
          const layer = document.querySelector('.reader-text-layer');
          const wrapper = layer.parentElement;
          const span = layer.querySelector('span');
          const range = document.createRange();
          range.selectNodeContents(span.firstChild);
          getSelection().removeAllRanges();
          getSelection().addRange(range);
          const rect = range.getBoundingClientRect();
          const origin = wrapper.getBoundingClientRect();
          wrapper.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
          // Measure painted pixels independently of the text layer.
          const canvas = wrapper.querySelector('canvas');
          const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
          let left = canvas.width, top = canvas.height, right = 0, bottom = 0;
          for (let y = 0; y < canvas.height; y++) for (let x = 0; x < canvas.width; x++) {
            const index = (y * canvas.width + x) * 4;
            if (pixels[index] < 100 && pixels[index + 1] < 100 && pixels[index + 2] < 100) {
              left = Math.min(left, x); top = Math.min(top, y);
              right = Math.max(right, x + 1); bottom = Math.max(bottom, y + 1);
            }
          }
          return {
            span: span.outerHTML,
            selection: { left: rect.left - origin.left, top: rect.top - origin.top, width: rect.width, height: rect.height },
            ink: { width: (right - left) * origin.width / canvas.width, height: (bottom - top) * origin.height / canvas.height },
          };
        });
        const axis = rotation % 180 ? 'height' : 'width';
        assert.ok(Math.abs(geometry.selection[axis] - geometry.ink[axis]) < 5,
          `Glyph/selection mismatch DPR=${dpr} rotation=${rotation}: ${JSON.stringify(geometry)}`);
        await page.waitForFunction(() => !document.querySelector('.reader-selection-popup').hidden);
        const annotationId = annotations.length;
        await page.locator('[data-color]').first().evaluate(button => button.click());
        await page.waitForFunction(count => document.querySelectorAll(`[data-annotation-id="${count}"]`).length > 0, annotationId);
        const highlight = await page.evaluate(() => {
          const nodes = [...document.querySelectorAll('.reader-annotation')];
          const latest = nodes.at(-1).dataset.annotationId;
          const rects = nodes.filter(node => node.dataset.annotationId === latest).map(node => node.getBoundingClientRect());
          const origin = document.querySelector('.reader-page-wrapper').getBoundingClientRect();
          const left = Math.min(...rects.map(rect => rect.left));
          const top = Math.min(...rects.map(rect => rect.top));
          return { left: left - origin.left, top: top - origin.top, width: Math.max(...rects.map(rect => rect.right)) - left, height: Math.max(...rects.map(rect => rect.bottom)) - top };
        });
        for (const key of ['left', 'top', 'width', 'height']) assert.ok(Math.abs(highlight[key] - geometry.selection[key]) < 1,
          `Highlight mismatch DPR=${dpr} rotation=${rotation} ${key}`);
      }
      await page.evaluate(() => { for (let index = 0; index < 8; index++) document.querySelector('[data-action=zoom-in]').click(); });
      await ready();
      assert.equal(await page.locator('.reader-text-layer span').count(), 1, 'Stale text layers after rapid zoom');
      assert.ok(!(await page.locator('[data-output=status]').textContent()).includes('failed'));
      assert.deepEqual(errors, []);
      await page.close();
      console.log(`PASS DPR=${dpr}, rotation=${rotation}: 100%, zoom, glyph bounds, highlights, rapid zoom`);
    }
  }
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, 'exit');
}
