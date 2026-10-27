// Run with npm run test:login. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
//
// The gateway daemon must never hand its bearer token to a browser.
// This drives the real SPA against a stubbed daemon and checks that
//   - the served page carries no credential (HTML, JS globals, console),
//   - an unauthenticated API call shows the login card,
//   - posting the token once logs in and boots the app,
//   - logging out revokes the session.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';

const TOKEN = 'test-token-123';
const SESSION = 'session-abc-123';
const origin = 'http://127.0.0.1:1426';

const state = {
  last_seq: 0,
  active_model_spec: null,
  profiles: [],
  agents: [],
  sessions: {},
  display_settings: {
    collapse_thinking: false,
    collapse_tool_calls: false,
    collapse_tool_results: false,
  },
  model_catalog: { providers: [], models: [], active_model: null },
};

const server = spawn(
  process.execPath,
  ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1426', '--strictPort'],
  { stdio: ['ignore', 'pipe', 'pipe'] },
);
let output = '';
server.stdout.on('data', (chunk) => { output += chunk; });
server.stderr.on('data', (chunk) => { output += chunk; });

let browser;
try {
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    if (output.includes(origin)) break;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const consoleLines = [];
  page.on('console', (message) => consoleLines.push(`${message.type()}: ${message.text()}`));
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));

  // Stub the daemon: the session cookie is the only browser credential.
  const sessions = new Set();
  const hasSession = (request) =>
    (request.headers().cookie || '').includes(`autonomics_session=${SESSION}`) && sessions.has(SESSION);

  await page.route('**/api/session', async (route) => {
    const request = route.request();
    if (request.method() === 'POST') {
      const body = request.postDataJSON();
      if (body?.token !== TOKEN) {
        await route.fulfill({ status: 401, json: { error: 'invalid token' } });
        return;
      }
      sessions.add(SESSION);
      await route.fulfill({
        status: 200,
        json: { authenticated: true, expires_in_secs: 43200 },
        headers: {
          'set-cookie': `autonomics_session=${SESSION}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200`,
        },
      });
      return;
    }
    if (request.method() === 'DELETE') {
      sessions.delete(SESSION);
      await route.fulfill({
        status: 200,
        json: { authenticated: false },
        headers: { 'set-cookie': 'autonomics_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0' },
      });
      return;
    }
    await route.fulfill(
      hasSession(request)
        ? { status: 200, json: { authenticated: true, expires_in_secs: 43200 } }
        : { status: 401, json: { authenticated: false } },
    );
  });

  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (!hasSession(request)) {
      await route.fulfill({ status: 401, json: { error: 'missing or invalid credentials' } });
      return;
    }
    if (path.endsWith('/state')) {
      await route.fulfill({ json: state });
      return;
    }
    if (path.endsWith('/profiles')) {
      await route.fulfill({ json: [] });
      return;
    }
    if (path.endsWith('/events')) {
      await route.fulfill({ status: 200, contentType: 'text/event-stream', body: '' });
      return;
    }
    await route.fulfill({ json: {} });
  });

  await page.goto(origin);

  // 1. No credential in the page, in a JS global, or in the console.
  const html = await page.content();
  assert.ok(!html.includes(TOKEN), 'the page leaked the token');
  assert.ok(!html.includes('__AUTONOMICS_GATEWAY_TOKEN__'), 'the page still bootstraps a token global');
  assert.equal(await page.evaluate(() => window.__AUTONOMICS_GATEWAY_TOKEN__), undefined);
  assert.ok(
    !(await page.evaluate(() => document.cookie)).includes('autonomics_session'),
    'the session cookie is readable from JavaScript',
  );

  // 2. An unauthenticated API call shows the login card.
  await page.getByText('需要 Gateway 令牌').waitFor();

  // 3. A wrong token is rejected.
  await page.locator('input[type="password"]').fill('not-the-token');
  await page.locator('button[type="submit"]').click();
  await page.getByText('invalid token').waitFor();

  // 4. The right token logs in and boots the app.
  await page.locator('input[type="password"]').fill(TOKEN);
  await page.locator('button[type="submit"]').click();
  await page.getByText('Gateway 已连接').waitFor();
  assert.equal(await page.getByText('需要 Gateway 令牌').count(), 0, 'the login card is still up');
  assert.equal(await page.locator('input[type="password"]').count(), 0, 'the token field survived the login');

  // 5. The token never reached the console.
  for (const line of consoleLines) {
    assert.ok(!line.includes(TOKEN), `the console leaked the token: ${line}`);
  }
  assert.deepEqual(errors, []);

  // 6. Logging out revokes the session.
  await page.getByRole('button', { name: '退出登录' }).click();
  await page.getByText('需要 Gateway 令牌').waitFor();
  assert.equal(sessions.size, 0, 'the session was not revoked');

  await page.screenshot({ path: '/tmp/autonomics-login.png', fullPage: true });
  console.log('Login flow: no token in page/globals/console, login and logout passed');
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, 'exit');
}
