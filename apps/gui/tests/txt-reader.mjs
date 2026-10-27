// Run with npm run test:txt. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';

const fixture = [
  'INTRODUCTION',
  'The fermentation of glucose proceeds through glycolysis and yields pyruvate.',
  'A second paragraph keeps its hard line breaks',
  'exactly as they appear in the stored file.',
  '',
  '2. Methods',
  'All assays were run in triplicate at thirty-seven degrees.',
  'The calibration curve for lactate was linear across the measured range.',
].join('\n');

const origin = 'http://127.0.0.1:1422';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1422', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
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
  let annotations = [];
  await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html lang="zh-CN"><body></body></html>' }));
  await page.route('**/api/v1/**', async route => {
    const request = route.request();
    const url = request.url();
    if (url.endsWith('/fulltext/raw')) {
      return route.fulfill({ contentType: 'text/plain; charset=utf-8', body: fixture });
    }
    if (request.method() === 'POST') {
      annotations.push({ ...request.postDataJSON(), id: String(annotations.length) });
    }
    if (request.method() === 'PATCH') {
      const id = url.match(/annotations\/([^/?]+)/)?.[1];
      const patch = request.postDataJSON();
      const index = annotations.findIndex(annotation => annotation.id === id);
      annotations[index] = { ...annotations[index], ...patch };
      return route.fulfill({ json: { annotation: annotations[index] } });
    }
    await route.fulfill({ json: { annotations } });
  });
  await page.goto(origin);
  await page.evaluate(async () => {
    await import('/src/app.css');
    // These isolated fixtures bypass Vite's HTML transform.
    const { default: RefreshRuntime } = await import('/@react-refresh');
    RefreshRuntime.injectIntoGlobalHook(window);
    window.$RefreshReg$ = () => {};
    window.$RefreshSig$ = () => type => type;
    window.__vite_plugin_react_preamble_installed__ = true;
    const { openTxtReader } = await import('/src/txt-reader.ts');
    window.reader = await openTxtReader({ articleId: 'txt-fixture', title: 'TXT regression' });
  });
  await page.waitForFunction(() => document.querySelector('.reader-txt-document')?.firstChild);

  // Document rendering preserves the raw text byte for byte.
  const rendered = await page.evaluate(() => document.querySelector('.reader-txt-document').textContent);
  assert.equal(rendered, fixture);
  assert.match(await page.locator('[data-output=status]').textContent(), /characters · 2 sections/);
  assert.equal(await page.locator('[data-output=font-size]').textContent(), '18 px');

  // Outline heuristics pick up the ALL-CAPS and numbered headings.
  const outline = await page.locator('.reader-sidebar-pane[data-pane=outline]').textContent();
  assert.ok(outline.includes('INTRODUCTION'), `Outline missing heading: ${outline}`);
  assert.ok(outline.includes('2. Methods'), `Outline missing numbered heading: ${outline}`);

  // Selection anchors to character offsets and paints a highlight overlay.
  const word = 'fermentation';
  const wordIndex = fixture.indexOf(word);
  await page.evaluate(([word, index]) => {
    const node = document.querySelector('.reader-txt-document').firstChild;
    const range = document.createRange();
    range.setStart(node, index);
    range.setEnd(node, index + word.length);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    document.querySelector('.reader-reflow').dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
  }, [word, wordIndex]);
  await page.waitForFunction(() => !document.querySelector('.reader-selection-popup').hidden);
  await page.locator('[data-color]').first().evaluate(button => button.click());
  await page.waitForFunction(() => document.querySelectorAll('.reader-reflow-highlight').length > 0);
  const posted = annotations.at(-1);
  assert.equal(posted.kind, 'highlight');
  assert.equal(posted.content, word);
  assert.equal(posted.rects[0].space, 'char');
  assert.equal(posted.rects[0].left, wordIndex);
  assert.equal(posted.rects[0].width, word.length);
  const mark = await page.evaluate(() => {
    const node = document.querySelector('.reader-reflow-highlight');
    const text = document.querySelector('.reader-txt-document').getBoundingClientRect();
    const box = node.getBoundingClientRect();
    return { id: node.dataset.annotationId, left: box.left - text.left, height: box.height };
  });
  assert.equal(mark.id, posted.id);
  assert.ok(mark.height > 4, 'Highlight mark has no height');

  // Search lists occurrences with snippets; clicking flashes the match.
  const term = 'lactate';
  await page.locator('[data-action=open-search]').evaluate(button => button.click());
  await page.fill('.reader-search-input', term);
  await page.press('.reader-search-input', 'Enter');
  await page.waitForFunction(() => document.querySelectorAll('[data-output=search-results] button').length > 0);
  assert.match(await page.locator('[data-output=search-results]').textContent(), new RegExp(term));
  await page.locator('[data-output=search-results] button').first().evaluate(button => button.click());
  await page.waitForFunction(() => Boolean(document.querySelector('.reader-txt-flash')));
  assert.match(await page.locator('[data-output=status]').textContent(), /1 match/);

  // Section navigation flashes the first heading.
  await page.locator('[data-action=section-next]').evaluate(button => button.click());
  await page.waitForFunction(() => Boolean(document.querySelector('.reader-txt-flash')));
  assert.match(await page.locator('[data-output=progress]').textContent(), /%$/);

  // Appearance: font size buttons, width buttons, theme toggle.
  await page.locator('[data-action=font-in]').evaluate(button => button.click());
  assert.equal(await page.locator('[data-output=font-size]').textContent(), '19 px');
  await page.locator('[data-action=width-narrow]').evaluate(button => button.click());
  assert.equal(await page.getAttribute('[data-action=width-narrow]', 'aria-pressed'), 'true');
  await page.locator('[data-action=toggle-theme]').evaluate(button => button.click());
  assert.equal(await page.getAttribute('.reader-reflow', 'data-reading-theme'), 'dark');

  // The annotation list navigates back to the highlight.
  await page.locator('.reader-rail-button[data-pane=annotations]').evaluate(button => button.click());
  await page.waitForFunction(() => document.querySelectorAll('.reader-annotation-list-item').length > 0);
  await page.locator('.reader-annotation-list-item').first().evaluate(button => button.click());
  await page.waitForFunction(() => document.querySelector('.reader-annotation-editor textarea'));

  assert.deepEqual(errors, []);
  await page.close();
  console.log('PASS txt reader: rendering, outline, char-anchored highlights, search, sections, appearance, annotation list');
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, 'exit');
}
