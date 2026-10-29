// Run with npm run test:workspace. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
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
  // Hold the event stream open: an immediately-completing body makes the
  // gateway client cycle reconnects and re-run hydration mid-test.
  await page.route('**/api/v1/events', () => new Promise(() => {}));
  await page.route('**/api/v1/state', route => route.fulfill({ json: {
    last_seq: 0,
    agents: [
      { path: '/root/alpha', agent_id: 'a1', status: 'Idle' },
      { path: '/root/beta', agent_id: 'a2', status: 'Streaming' },
    ],
    sessions: {
      '/root/alpha': [{ id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 }],
      '/root/beta': [{ id: 's2', title: 'Beta main', created_at: 1, last_active: 3 }],
    },
    display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
  } }));
  await page.route('**/api/v1/agents/a1/sessions/s1/history', route => route.fulfill({ json: { messages: [
    { id: 'm1', role: 'user', content: [{ type: 'text', text: 'alpha prompt' }] },
    { id: 'm2', role: 'assistant', content: [{ type: 'text', text: 'alpha answer' }] },
  ] } }));
  await page.route('**/api/v1/agents/a2/sessions/s2/history', route => route.fulfill({ json: { messages: [
    { id: 'm3', role: 'user', content: [{ type: 'text', text: 'beta prompt' }] },
    { id: 'm4', role: 'assistant', content: [{ type: 'text', text: 'beta answer' }] },
  ] } }));
  await page.route('**/api/v1/bib/**', route => route.fulfill({ json: { collections: [], articles: [], total: 0 } }));

  await page.goto(origin);
  const composer = page.getByPlaceholder('输入消息…');

  // Docked: the workspace lives in the agent view's center column.
  await composer.waitFor({ state: 'visible' });
  await assert.doesNotReject(() => page.getByText('alpha answer').waitFor({ state: 'visible' }));
  const dockedInFloat = await composer.evaluate(node => Boolean(node.closest('.fixed')));
  assert.equal(dockedInFloat, false, 'workspace must not be floating while on the agent view');
  await composer.fill('draft persists');

  // Floating is now an explicit choice; the default is a docked assistant.
  await page.getByRole('button', { name: 'Bibliography' }).click();
  await page.getByLabel('Agent 显示方式').selectOption('floating');
  await composer.waitFor({ state: 'visible' });
  const floating = await composer.evaluate(node => Boolean(node.closest('.fixed')));
  assert.equal(floating, true, 'workspace must float over the bibliography view');
  assert.equal(await composer.inputValue(), 'draft persists', 'draft survives the mode switch');
  await assert.doesNotReject(() => page.getByText('alpha answer').waitFor({ state: 'visible' }));

  // The floating tab bar still switches agents and loads their history.
  await page.getByRole('button', { name: 'beta', exact: true }).click();
  await assert.doesNotReject(() => page.getByText('beta answer').waitFor({ state: 'visible' }));

  // Collapse shrinks the floating shell into a mini window; the workspace
  // keeps its state off-document.
  const shell = page.locator('[data-floating-shell]');
  await page.getByRole('button', { name: '收起 agent 浮窗' }).click();
  await shell.locator('textarea').waitFor({ state: 'detached' });
  assert.equal(await composer.count(), 0, 'composer must leave the document while collapsed');
  await page.waitForTimeout(260); // let the size transition settle before measuring
  const mini = await shell.boundingBox();
  assert.ok(mini && mini.width < 300 && mini.height < 60, 'collapsed shell must shrink to a mini window');

  // Expand again: draft and transcript are restored without a refetch.
  await page.getByRole('button', { name: '展开 agent 浮窗' }).click();
  await composer.waitFor({ state: 'visible' });
  assert.equal(await composer.inputValue(), 'draft persists', 'draft survives collapse');
  await assert.doesNotReject(() => page.getByText('beta answer').waitFor({ state: 'visible' }));

  // The floating panel can be dragged by its header; the position lands
  // in localStorage and carries over to the pill.
  const panel = page.locator('[data-floating-shell]');
  await page.waitForTimeout(260); // let the entrance animation settle before measuring
  const header = panel.locator('[title="拖拽移动 · 双击复位到右下角"]');
  const hb = await header.boundingBox();
  assert.ok(hb, 'panel header must be measurable');
  const before = await panel.boundingBox();
  const grab = { x: hb.x + 40, y: hb.y + 18 };
  await page.mouse.move(grab.x, grab.y);
  await page.mouse.down();
  await page.mouse.move(grab.x - 300, grab.y - 60, { steps: 8 });
  await page.mouse.up();
  const after = await panel.boundingBox();
  assert.ok(Math.abs(after.x - (before.x - 300)) <= 8, 'panel must follow the pointer horizontally');
  assert.ok(Math.abs(after.y - (before.y - 60)) <= 8, 'panel must follow the pointer vertically');
  assert.equal(await composer.inputValue(), 'draft persists', 'dragging must not disturb the draft');
  const stored = await page.evaluate(() => localStorage.getItem('autonomics.workspace.float.pos'));
  assert.ok(stored && Math.abs(JSON.parse(stored).x - after.x) < 4, 'dragged position must persist');

  // Collapsing keeps the dragged position instead of snapping to the corner.
  await page.getByRole('button', { name: '收起 agent 浮窗' }).click();
  await shell.locator('textarea').waitFor({ state: 'detached' });
  await page.waitForTimeout(260); // let the size transition settle before measuring
  const miniBox = await shell.boundingBox();
  assert.ok(miniBox.x < hb.x - 100, 'mini window must stay where the panel was dragged');

  // Dragging the mini window moves it without triggering the expand click.
  const down = { x: miniBox.x + miniBox.width / 2, y: miniBox.y + miniBox.height / 2 };
  await page.mouse.move(down.x, down.y);
  await page.mouse.down();
  await page.mouse.move(down.x - 40, down.y - 20, { steps: 4 });
  await page.mouse.up();
  assert.equal(await composer.count(), 0, 'dragging the mini window must not expand it');

  // A plain click still expands, and the panel reopens at the dragged spot.
  await page.getByRole('button', { name: '展开 agent 浮窗' }).click();
  await panel.waitFor({ state: 'visible' });
  await page.waitForTimeout(260); // let the entrance animation settle before measuring
  const reopened = await panel.boundingBox();
  assert.ok(Math.abs(reopened.x - (after.x - 40)) <= 6, 'panel must reopen at the dragged position');

  // Back on the agent view the workspace re-docks into the center column.
  await page.getByRole('button', { name: 'Agent', exact: true }).click();
  await composer.waitFor({ state: 'visible' });
  const redocked = await composer.evaluate(node => Boolean(node.closest('.fixed')));
  assert.equal(redocked, false, 'workspace must re-dock on the agent view');
  assert.equal(await composer.inputValue(), 'draft persists', 'draft survives the round trip');
  await assert.doesNotReject(() => page.getByText('beta answer').waitFor({ state: 'visible' }));

  assert.deepEqual(errors, []);
  console.log('floating-workspace: all assertions passed');
} finally {
  await browser?.close();
  server.kill();
}
