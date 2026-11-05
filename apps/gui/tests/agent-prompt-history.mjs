// Regression: MessageInjected events must not leak into the Ctrl+R prompt
// history. The old handler recorded event.UserMessageAcknowledged while
// handling MessageInjected (undefined there), so the history array held
// undefined entries and the history dialog crashed the tree on render.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1426';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1426', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  for (let attempt = 0; !output.includes(origin); attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  const pageErrors = [];
  page.on('pageerror', error => pageErrors.push(String(error)));
  await page.addInitScript(() => {
    const original = window.fetch;
    window.fetch = (input, init) => {
      if (String(input).endsWith('/api/v1/events')) {
        return Promise.resolve(new Response(new ReadableStream({
          start(controller) { window.sendTestEvent = (event) => controller.enqueue(new TextEncoder().encode(event)); },
        }), { headers: { 'Content-Type': 'text/event-stream' } }));
      }
      return original(input, init);
    };
  });
  await page.route('**/api/v1/state', route => route.fulfill({ json: {
    last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: {},
    display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
  } }));
  await page.route('**/api/v1/bib/**', route => route.fulfill({ json: { collections: [], articles: [], total: 0 } }));
  await page.goto(origin);
  await page.getByRole('tab', { name: 'alpha', exact: true }).waitFor();

  // One injected (non-user) message followed by one real user prompt.
  await page.evaluate(() => window.sendTestEvent([
    `id: 1\nevent: agent\ndata: ${JSON.stringify({ agent: '/root/alpha', event: { MessageInjected: { text: 'injected notice', origin: 'System' } } })}\n\n`,
    `id: 2\nevent: agent\ndata: ${JSON.stringify({ agent: '/root/alpha', event: { UserMessageAcknowledged: 'real user prompt' } })}\n\n`,
  ].join('')));
  await page.getByText('real user prompt').waitFor({ state: 'visible' });

  // Ctrl+R opens the prompt history over the active agent tab.
  await page.keyboard.press('Control+r');
  const dialog = page.getByRole('dialog');
  await dialog.waitFor({ state: 'visible' });
  await dialog.getByText('real user prompt').waitFor({ state: 'visible' });
  // The injected notice is a system notification, never a recallable prompt.
  assert.equal(await dialog.getByText('injected notice').count(), 0);

  // Switching tabs with the dialog state present must not unmount the tree.
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Search agents' }).waitFor({ state: 'visible' });
  assert.deepEqual(pageErrors, []);
  console.log('agent-prompt-history: passed');
} finally {
  await browser?.close();
  server.kill();
}
