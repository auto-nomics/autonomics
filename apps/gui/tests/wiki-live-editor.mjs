import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';
const origin = 'http://127.0.0.1:1441';
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1441', '--strictPort']);
const entry = { id: 'live-test', title: 'Live test', content: '# Heading\n\nFirst **bold** paragraph\n\nLast paragraph', alias: [], tags: [], ctime: 1, mtime: 1 };
let browser;
try {
  await new Promise((resolve, reject) => { vite.stdout.on('data', data => { if (data.toString().includes(origin)) resolve(); }); vite.on('exit', reject); });
  browser = await chromium.launch({ executablePath: '/usr/bin/chromium', args: ['--no-sandbox'] });
  const page = await browser.newPage();
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.route(origin + '/', route => route.fulfill({ contentType: 'text/html', body: '<html><body></body></html>' }));
  await page.route('**/api/v1/**', route => route.fulfill({ json: route.request().url().endsWith('/live-test') ? entry : { entries: [], in_links: [], out_links: [], unlinked_mentions: [], versions: [] } }));
  await page.goto(origin);
  await page.evaluate(async () => {
    await import('/src/app.css');
    const { default: refresh } = await import('/@react-refresh');
    refresh.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => t => t; window.__vite_plugin_react_preamble_installed__ = true;
    const { mount } = await import('/tests/wiki-images-mount.tsx'); mount('live-test', true);
  });
  const pane = page.locator('[data-wiki-pane="first"]');
  await pane.locator('.wiki-live-preview strong').waitFor();
  await pane.locator('.wiki-live-preview strong').click();
  const clickedOffset = await page.evaluate(() => {
    const selection = window.getSelection();
    const line = selection?.anchorNode?.parentElement?.closest('.cm-line');
    if (!selection?.anchorNode || !line) return -1;
    const range = document.createRange();
    range.setStart(line, 0); range.setEnd(selection.anchorNode, selection.anchorOffset);
    return range.toString().length;
  });
  assert.ok(clickedOffset >= 8 && clickedOffset <= 12, `clicked bold text should map inside Markdown bold span, got ${clickedOffset}`);
  await pane.locator('.cm-content').press('End');
  await pane.locator('.cm-cursor').first().waitFor({ state: 'visible' });
  const cursor = await pane.locator('.cm-cursor').first().evaluate(el => {
    const style = getComputedStyle(el);
    return { color: style.borderLeftColor, width: parseFloat(style.borderLeftWidth), height: el.getBoundingClientRect().height };
  });
  assert.ok(cursor.width > 0 && cursor.height > 0);
  assert.notEqual(cursor.color, 'rgba(0, 0, 0, 0)');
  await page.keyboard.type(' appended');
  await pane.locator('.wiki-live-preview').filter({ hasText: 'Last paragraph' }).click();
  await pane.locator('.wiki-live-preview').filter({ hasText: 'First bold paragraph appended' }).waitFor();
  const cacheRefreshed = await page.evaluate(async () => {
    const state = await import('/src/features/wiki/wiki-state.ts');
    state.cacheEntry({ id: 'rename-cache-test', title: 'Old title', content: '', alias: [], tags: [], ctime: 1, mtime: 1 });
    state.bumpWikiRevision();
    return !state.entryCache.has('rename-cache-test') && state.cachedTitle('rename-cache-test') === undefined;
  });
  assert.equal(cacheRefreshed, true);
  assert.deepEqual(errors, []);
  console.log('Live preview: render, activate, edit and re-render passed');
} finally { await browser?.close(); vite.kill(); }
