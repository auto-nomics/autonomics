import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1451';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1451', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser;
server.stdout.on('data', chunk => output += chunk); server.stderr.on('data', chunk => output += chunk);
try {
  for (let n = 0; !output.includes(origin); n++) { if (server.exitCode !== null || n > 120) throw new Error(output); await new Promise(resolve => setTimeout(resolve, 50)); }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  page.setDefaultTimeout(10000);
  await page.addInitScript(() => localStorage.setItem('autonomics.openTabs', JSON.stringify(['/root/alpha', '/root/beta'])));
  const errors = [], requests = { alpha: 0, beta: 0 };
  page.on('pageerror', error => errors.push(error.message));
  await page.route(`${origin}/api/session`, route => route.fulfill({ json: {} }));
  await page.route('**/api/v1/**', route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/events')) return new Promise(() => {});
    if (path.endsWith('/state')) return route.fulfill({ json: { last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }, { path: '/root/beta', agent_id: 'a2', status: 'Idle' }], contexts: { '/root/alpha': { id: 's1' }, '/root/beta': { id: 's2' } }, display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false } } });
    if (path.includes('/sessions/s1/history')) {
      requests.alpha++;
      return route.fulfill({ json: { messages: [
        ...Array.from({ length: 90 }, (_, index) => ({ id: `m${index}`, role: 'assistant', content: [{ type: 'text', text: `Message ${index}: ${'long transcript content '.repeat(12)}` }] })),
        { id: 'trace', role: 'assistant', content: [{ type: 'thinking', thinking: 'cached reasoning' }, { type: 'text', text: 'alpha tail' }] },
      ] } });
    }
    if (path.includes('/sessions/s2/history')) { requests.beta++; return route.fulfill({ json: { messages: [{ id: 'beta', role: 'assistant', content: [{ type: 'text', text: 'beta response' }] }] } }); }
    if (path.endsWith('/storage/agents')) return route.fulfill({ json: [] });
    return route.fulfill({ json: {} });
  });
  await page.goto(origin);
  const alpha = page.locator('[data-agent-workspace="/root/alpha"]');
  const beta = page.locator('[data-agent-workspace="/root/beta"]');
  const alphaTab = page.locator('[role="tab"][title="/root/alpha"]');
  const betaTab = page.locator('[role="tab"][title="/root/beta"]');
  await alpha.getByText('alpha tail').waitFor();
  await betaTab.waitFor();
  const thinking = alpha.getByRole('button', { name: /Thought process/ });
  await thinking.click();
  assert.equal(await thinking.getAttribute('aria-expanded'), 'false');
  await alpha.getByPlaceholder('输入消息…').fill('alpha unsent draft');
  const viewport = alpha.locator('[data-slot="scroll-area-viewport"]').first();
  await viewport.evaluate(el => { el.scrollTop = 250; window.cachedAgentViewport = el; });
  await page.waitForTimeout(150);
  const position = await viewport.evaluate(el => el.scrollTop);
  assert.ok(position > 0);
  await alpha.getByRole('button', { name: 'Search agents' }).click();
  await alpha.locator('[data-slot="command-input"]').fill('alpha');
  await betaTab.click();
  await beta.getByText('beta response').waitFor();
  await beta.getByPlaceholder('输入消息…').fill('beta unsent draft');
  await alphaTab.click();
  await alpha.getByText('alpha tail').waitFor();
  assert.equal(await alpha.getByPlaceholder('输入消息…').inputValue(), 'alpha unsent draft');
  assert.equal(await alpha.locator('[data-slot="command-input"]').inputValue(), 'alpha', 'picker state must survive a tab switch');
  await alpha.locator('[data-slot="command-input"]').press('Escape');
  assert.equal(await thinking.getAttribute('aria-expanded'), 'false', 'trace expansion must survive a tab switch');
  assert.equal(await viewport.evaluate(el => el === window.cachedAgentViewport), true, 'transcript DOM must stay mounted');
  assert.ok(Math.abs((await viewport.evaluate(el => el.scrollTop)) - position) < 3, 'scroll position must survive a tab switch');
  assert.equal(requests.alpha, 1, 'revisiting the tab must not refetch its history');
  await betaTab.click();
  assert.equal(await beta.getByPlaceholder('输入消息…').inputValue(), 'beta unsent draft');
  assert.equal(requests.beta, 1);
  await alphaTab.click();
  await page.getByRole('button', { name: '拆分编辑器' }).click();
  await page.locator('[data-editor-group="secondary"] [data-agent-workspace="/root/alpha"]').waitFor();
  assert.equal(await viewport.evaluate(el => el === window.cachedAgentViewport), true, 'moving a tab must preserve its transcript DOM');
  assert.ok(Math.abs((await viewport.evaluate(el => el.scrollTop)) - position) < 3, 'moving a tab must preserve its scroll position');
  assert.deepEqual(errors, []);
  console.log('agent-tab-cache: all assertions passed');
} finally { await browser?.close(); server.kill(); }
