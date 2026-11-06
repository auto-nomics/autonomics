import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
const origin = 'http://127.0.0.1:1462';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1462', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser;
server.stdout.on('data', data => output += data); server.stderr.on('data', data => output += data);
try {
  for (let i = 0; !output.includes(origin); i++) { if (server.exitCode !== null || i > 120) throw new Error(output); await new Promise(resolve => setTimeout(resolve, 50)); }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1200, height: 600 } });
  await page.route(`${origin}/api/session`, route => route.fulfill({ json: {} }));
  await page.route('**/api/v1/**', route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/events')) return new Promise(() => {});
    if (path.endsWith('/state')) return route.fulfill({ json: { last_seq: 0, agents: [], contexts: {}, display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false } } });
    if (path.endsWith('/model-config')) return route.fulfill({ json: { active_model: null, models: [], providers: Array.from({ length: 20 }, (_, i) => ({ id: i, name: `Provider ${i}`, provider_type: 'custom' })) } });
    if (path.endsWith('/storage/agents')) return route.fulfill({ json: [] });
    return route.fulfill({ json: {} });
  });
  await page.goto(origin);
  await page.getByRole('navigation', { name: '工作台活动栏' }).getByRole('button', { name: 'CONFIG' }).click();
  const viewport = page.getByLabel('Configuration', { exact: true });
  await page.getByText('Provider 0', { exact: true }).waitFor();
  const dimensions = await viewport.evaluate(el => ({ height: el.clientHeight, content: el.scrollHeight }));
  assert.ok(dimensions.content > dimensions.height, 'settings content must overflow its viewport');
  await viewport.hover();
  await page.mouse.wheel(0, 600);
  await page.waitForTimeout(200);
  assert.ok(await viewport.evaluate(el => el.scrollTop) > 0, 'mouse wheel must scroll settings');
  console.log('config-scroll: all assertions passed');
} finally { await browser?.close(); server.kill(); }
