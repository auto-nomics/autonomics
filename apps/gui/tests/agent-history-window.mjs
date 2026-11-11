import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1454';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1454', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser;
server.stdout.on('data', chunk => output += chunk);
server.stderr.on('data', chunk => output += chunk);
try {
  for (let n = 0; !output.includes(origin); n++) {
    if (server.exitCode !== null || n > 120) throw new Error(output);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    localStorage.setItem('autonomics.openTabs', JSON.stringify(['/root/alpha']));
    // Record every mounted row, including rows removed by a later effect.
    // Checking only the settled DOM misses a synchronous full-history render.
    window.mountedHistoryRows = new Set();
    new MutationObserver(records => {
      for (const record of records) for (const node of record.addedNodes) {
        if (!(node instanceof Element)) continue;
        const rows = [...node.querySelectorAll('.md')];
        if (node.matches('.md')) rows.push(node);
        for (const row of rows) {
          const match = row.textContent.match(/^History row (\d+)/);
          if (match) window.mountedHistoryRows.add(Number(match[1]));
        }
      }
    }).observe(document, { childList: true, subtree: true });
  });
  const count = 1600;
  let releaseHistory;
  const release = new Promise(resolve => { releaseHistory = resolve; });
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/events')) return new Promise(() => {});
    if (path.endsWith('/state')) return route.fulfill({ json: {
      last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }],
      contexts: { '/root/alpha': { id: 's1' } },
    } });
    if (path.endsWith('/sessions/s1/history')) {
      await release;
      return route.fulfill({ json: Array.from({ length: count }, (_, index) => ({
        id: `m${index}`, role: 'assistant', content: [{ type: 'text', text: `History row ${index}\n\n**Persisted answer** with $p = mv$.\n\n${'Transcript content. '.repeat(8)}` }],
      })) });
    }
    if (path.endsWith('/storage/agents')) return route.fulfill({ json: [] });
    return route.fulfill({ json: {} });
  });
  await page.goto(origin);
  const workspace = page.locator('[data-agent-workspace="/root/alpha"]');
  const composer = workspace.getByPlaceholder('输入消息…');
  await composer.fill('draft while loading');
  releaseHistory();
  await workspace.getByText(`History row ${count - 1}`, { exact: true }).waitFor();
  const mounted = await page.evaluate(() => [...window.mountedHistoryRows]);
  assert.equal(mounted.length, 60, 'the first commit must mount only the tail, never the entire history');
  assert.ok(mounted.every(index => index >= count - 60));
  assert.equal(await composer.inputValue(), 'draft while loading');

  const viewport = workspace.locator('[data-slot="scroll-area-viewport"]').first();
  await viewport.evaluate(el => { el.scrollTop = 250; });
  await page.waitForTimeout(100);
  const tailOffset = await viewport.evaluate(el => el.scrollHeight - el.scrollTop);
  const earlier = workspace.getByRole('button', { name: /Load earlier messages/ });
  // Trigger without Playwright scrolling the button into view first.
  await earlier.evaluate(el => el.click());
  await workspace.getByText(`History row ${count - 140}`, { exact: true }).waitFor({ state: 'attached' });
  assert.equal(await workspace.locator('.md').count(), 140);
  assert.ok(Math.abs(await viewport.evaluate(el => el.scrollHeight - el.scrollTop) - tailOffset) < 3, 'prepending must preserve the visible scroll anchor');

  for (let n = 0; n < 20 && await earlier.count(); n++) {
    await earlier.evaluate(el => el.click());
    await page.waitForTimeout(20);
    assert.ok(await workspace.locator('.md').count() <= 500, 'earlier-message windows must stay bounded');
  }
  await workspace.getByText('History row 0', { exact: true }).waitFor({ state: 'attached' });
  await workspace.getByRole('button', { name: 'Scroll to bottom' }).click();
  await workspace.getByText(`History row ${count - 1}`, { exact: true }).waitFor();
  assert.equal(await workspace.locator('.md').count(), 60);
  await composer.fill('workbench remains usable');
  assert.deepEqual(errors, []);
  console.log('agent-history-window: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
}
