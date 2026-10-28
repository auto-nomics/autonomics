// Run with npm run test:agent-trace. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
// Covers the Trace-based rendering of reasoning, tool calls and tool results.
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
  page.on('dialog', dialog => dialog.accept());

  await page.route(`${origin}/api/session`, route => route.fulfill({ json: {} }));
  await page.route('**/api/v1/events', () => new Promise(() => {}));
  await page.route('**/api/v1/state', route => route.fulfill({ json: {
    last_seq: 0,
    agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }],
    sessions: { '/root/alpha': [{ id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 }] },
    // collapse_*: false means thinking/tool rows start open.
    display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
  } }));
  await page.route('**/api/v1/agents/a1/sessions/s1/history', route => route.fulfill({ json: { messages: [
    { id: 'm1', role: 'user', content: [{ type: 'text', text: 'alpha prompt' }] },
    { id: 'm2', role: 'assistant', content: [
      { type: 'thinking', thinking: 'quiet reasoning' },
      { type: 'tool_use', name: 'list_dir', input: { path: '/tmp', opts: { depth: 2, all: true } } },
      { type: 'tool_result', content: 'a.txt\nb.txt' },
      { type: 'tool_result', content: { files: ['a.txt', 'b.txt'], count: 2 } },
      { type: 'tool_result', is_error: true, content: 'boom' },
      { type: 'text', text: 'alpha answer' },
    ] },
  ] } }));
  await page.route('**/api/v1/bib/**', route => route.fulfill({ json: { collections: [], articles: [], total: 0 } }));

  await page.goto(origin);
  await page.getByPlaceholder('输入消息…').waitFor({ state: 'visible' });
  await page.getByText('alpha answer').waitFor({ state: 'visible' });

  // The four consecutive steps render as one Trace list between the text messages.
  const trace = page.locator('[data-slot="trace"]');
  assert.equal(await trace.count(), 1, 'thinking + tool steps must group into a single Trace');

  const lines = trace.locator('[data-slot="trace-line"]');
  assert.equal(await lines.count(), 5, 'one line per thinking/tool item');
  await assert.doesNotReject(() => trace.getByText('Thought process').waitFor({ state: 'visible' }));
  await assert.doesNotReject(() => trace.getByText('list_dir').waitFor({ state: 'visible' }));
  await assert.doesNotReject(() => trace.getByText('Result').first().waitFor({ state: 'visible' }));

  // Rows are collapsible buttons; collapse_*: false starts them open.
  const expanded = await trace.locator('[data-slot="trace-collapsible-trigger"][aria-expanded="true"]').count();
  assert.equal(expanded, 5, 'thinking, tool call and all three result rows start open');

  // Tool call input renders as structured field rows, nested objects included.
  await assert.doesNotReject(() => trace.getByText('path', { exact: true }).waitFor({ state: 'visible' }));
  // '/tmp' matches twice: the collapsed-row summary and the expanded field leaf.
  await assert.doesNotReject(() => trace.getByText('/tmp', { exact: true }).first().waitFor({ state: 'visible' }));
  await assert.doesNotReject(() => trace.getByText('depth', { exact: true }).waitFor({ state: 'visible' }));
  await assert.doesNotReject(() => trace.getByText('true', { exact: true }).waitFor({ state: 'visible' }));

  // A JSON tool result renders its fields; the plain-text result stays raw.
  await assert.doesNotReject(() => trace.getByText('files', { exact: true }).waitFor({ state: 'visible' }));
  await assert.doesNotReject(() => trace.getByText('count', { exact: true }).waitFor({ state: 'visible' }));
  await assert.doesNotReject(() => trace.getByText('b.txt').first().waitFor({ state: 'visible' }));

  // A failed result carries the error status on its line.
  const failedLine = trace.locator('[data-slot="trace-line"][data-status="error"]');
  assert.equal(await failedLine.count(), 1, 'exactly one failed tool result');
  await assert.doesNotReject(() => failedLine.getByText('Failed').waitFor({ state: 'visible' }));

  // Toggling collapses the detail and flips aria-expanded.
  await trace.getByText('Thought process').click();
  await page.waitForTimeout(260); // height transition
  const thinkingTrigger = trace.locator('[data-slot="trace-collapsible-trigger"]', { hasText: 'Thought process' });
  assert.equal(await thinkingTrigger.getAttribute('aria-expanded'), 'false', 'thinking row collapses on click');
  assert.equal(await trace.getByText('quiet reasoning').count(), 0, 'collapsed detail is unmounted');

  assert.deepEqual(errors, []);
  console.log('agent-trace: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
}
