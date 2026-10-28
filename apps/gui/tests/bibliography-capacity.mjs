// Real browser/HTTP harness: no route mocks and no simulated native bridge.
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, writeFileSync, readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';

const [apiOrigin, token, rawCount, outputPath] = process.argv.slice(2);
const count = Number(rawCount);
assert.ok(apiOrigin?.startsWith('http://127.0.0.1:') && token && count >= 100 && outputPath);
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '0'], {
  stdio: ['ignore', 'pipe', 'pipe'],
  env: { ...process.env, AUTONOMICS_HTTP_API_ADDR: new URL(apiOrigin).host, AUTONOMICS_HTTP_API_TOKEN: token },
});
let log = '', browser;
vite.stdout.on('data', bytes => log += bytes); vite.stderr.on('data', bytes => log += bytes);
const distribution = values => {
  const sorted = [...values].sort((a, b) => a - b);
  return { samples: sorted.length, p50_ms: sorted[Math.floor(sorted.length / 2)], p95_ms: sorted[Math.ceil(sorted.length * .95) - 1], max_ms: sorted.at(-1) };
};
try {
  const deadline = Date.now() + 30000;
  while (!log.match(/http:\/\/127\.0\.0\.1:\d+/)) {
    if (vite.exitCode !== null || Date.now() > deadline) throw new Error(log);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const origin = log.match(/http:\/\/127\.0\.0\.1:\d+/)[0];
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || ['/usr/bin/chromium', '/opt/google/chrome/chrome'].find(existsSync), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  page.setDefaultTimeout(30000);
  const errors = [], requests = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => {
    if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/bib/search/v2')) {
      const body = request.postDataJSON(); if (body?.mode === 'hybrid') requests.push(body);
    }
  });
  await page.addInitScript(() => {
    window.addEventListener('keydown', event => {
      if (event.key === 'Enter' && event.target?.getAttribute('aria-label') === 'Search bibliography') {
        window.__capacitySubmission = { topic: event.target.value, start: performance.now(), before: document.body.innerText };
      }
    }, true);
  });
  const initialTopic = 'immune gene expression capacityinitial';
  const initialResponse = page.waitForResponse(response => {
    if (response.request().method() !== 'POST' || !new URL(response.url()).pathname.endsWith('/bib/search/v2')) return false;
    return response.status() === 200 && response.request().postDataJSON()?.semantic_query === initialTopic;
  });
  const location = { version: 1, request: { mode: 'hybrid', semantic_query: initialTopic, query: '', include_facets: true, include_articles: true, page_size: 50 }, savedId: null };
  await page.goto(`${origin}/tests/bibliography-capacity.html?bib_search=${encodeURIComponent(JSON.stringify(location))}`);
  await page.evaluate(async () => {
    window.__capacityMountStart = performance.now();
    await import('/src/app.css');
    const { default: Refresh } = await import('/@react-refresh');
    if (!window.__vite_plugin_react_preamble_installed__) {
      Refresh.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => type => type;
      window.__vite_plugin_react_preamble_installed__ = true;
    }
    const { mountBibliography } = await import('/tests/bibliography-menu-mount.tsx');
    mountBibliography(document.getElementById('capacity-root'));
  });
  const initial = await (await initialResponse).json();
  assert.equal(initial.retrieval.eligible_articles, count);
  await page.getByText(initial.hits[0].title, { exact: true }).first().waitFor({ state: 'visible' });
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const mountToFirstVisible = await page.evaluate(() => performance.now() - window.__capacityMountStart);
  const harnessToFirstVisible = performance.now();
  await page.evaluate(async () => { await document.fonts.ready; });
  const input = page.getByRole('textbox', { name: 'Search bibliography' });
  assert.equal(await page.getByLabel('Search mode').inputValue(), 'hybrid');
  assert.equal((await page.getByLabel('Articles in library').textContent()).trim(), count.toLocaleString('en-US'));
  const samples = [], responseTimes = []; let ambiguous = 0;
  for (let attempt = 0; samples.length < 20 && attempt < 30; attempt++) {
    const topic = `immune gene expression capacityprobe${attempt}`;
    const response = page.waitForResponse(response => response.request().method() === 'POST'
      && new URL(response.url()).pathname.endsWith('/bib/search/v2')
      && response.request().postDataJSON()?.semantic_query === topic);
    await input.fill(topic); await input.press('Enter');
    const reply = await response; assert.equal(reply.status(), 200);
    const data = await reply.json();
    assert.equal(data.retrieval.eligible_articles, count); assert.ok(data.hits.length > 0);
    const stamp = await page.evaluate(() => ({ ...window.__capacitySubmission, responseSeen: performance.now() }));
    assert.equal(stamp.topic, topic);
    await page.getByText(data.hits[0].title, { exact: true }).first().waitFor({ state: 'visible' });
    const visibleAt = await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve(performance.now())))));
    if (stamp.before.includes(data.hits[0].title)) { ambiguous++; continue; }
    samples.push(visibleAt - stamp.start); responseTimes.push(stamp.responseSeen - stamp.start);
    assert.equal(requests.filter(request => request.semantic_query === topic).length, 1, 'unexpected duplicate query');
  }
  assert.equal(samples.length, 20); assert.deepEqual(errors, []);
  const screenshot = outputPath.replace(/\.json$/, '.png'); await page.screenshot({ path: screenshot });
  const result = { schema: 'bib-browser-capacity-v1', count, browser: await browser.version(), viewport: { width: 1440, height: 1000 },
    transport: 'production gatewayRequest -> Vite auth proxy -> real loopback HTTP API; no route mocks',
    initial_mount_to_first_visible_ms: mountToFirstVisible, harness_process_to_first_visible_ms: harnessToFirstVisible,
    enter_to_visible: distribution(samples), enter_to_response_observed: distribution(responseTimes), ambiguous_samples_excluded: ambiguous,
    hybrid_requests: requests.length, page_sizes: [...new Set(requests.map(request => request.page_size))],
    screenshot_sha256: createHash('sha256').update(readFileSync(screenshot)).digest('hex'),
    limitations: ['Development Vite feature mount, not isolated production app build or native IPC', 'Visibility plus two animation frames is a paint proxy, not physical display latency',
      'Automation response observation and locator polling add overhead', 'Model is a controlled HTTP fixture; no actual inference/IR quality claim',
      'Distinct synthetic topics force actual requests; samples with an already visible first title are excluded and counted'] };
  writeFileSync(outputPath, JSON.stringify(result, null, 2) + '\n');
  console.log(`Browser ${count} enter-to-visible P95 ${result.enter_to_visible.p95_ms.toFixed(1)}ms`);
} finally {
  await browser?.close(); vite.kill('SIGTERM');
  if (vite.exitCode === null) await once(vite, 'exit');
}
