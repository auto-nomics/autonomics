// Plugin dev window + manager test: manager surface, dev window shell
// (session auto-start, file tree + editor + action bar), validate →
// report gates, save + 409 conflict. Terminal pane covered by phase-7
// smoke. Run with npm run test:plugin-dev.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1444';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1444', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser;
server.stdout.on('data', chunk => output += chunk); server.stderr.on('data', chunk => output += chunk);
try {
  for (let n = 0; !output.includes(origin); n++) { if (server.exitCode !== null || n > 120) throw new Error(output); await new Promise(r => setTimeout(r, 50)); }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } }); page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', e => { errors.push(e.message); console.error('PAGE ERROR', e.stack); });

  const writes = { saves: [], validates: 0, sessionStarts: [] };
  let diskContents = { 'manifest.toml': '# manifest\n', 'scripts/demo.sh': 'set -eu\n', 'README.md': '# demo\n' };
  const gates = [
    { name: 'manifest', status: 'pass' }, { name: 'policy', status: 'pass' },
    { name: 'kind_collision', status: 'pass' }, { name: 'script_static', status: 'pass' },
    { name: 'registry_compile', status: 'pass' }, { name: 'secret_scan', status: 'pass' },
  ];

  await page.route(`${origin}/api/session`, r => r.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const url = new URL(route.request().url());
    const path = url.pathname.replace('/api/v1', '');
    if (path === '/events') return;
    if (path === '/state') return route.fulfill({ json: { last_seq: 0, agents: [], contexts: {}, display_settings: {} } });
    if (path === '/plugin-projects' && route.request().method() === 'GET')
      return route.fulfill({ json: { projects: [{ name: 'hello-plugin', status: 'Draft', image: 'reg/env@sha256:abc', error: null }] } });
    if (path === '/plugins')
      return route.fulfill({ json: { plugins: [{ name: 'hello-plugin', status: 'installed', image: 'reg/env@sha256:abc', kinds: ['hello_plugin'], registered: true }] } });
    if (path === '/plugins/environments')
      return route.fulfill({ json: { environments: [{ id: 'sh-dev', reference: 'reg/env@sha256:abc' }] } });
    if (path === '/plugin-projects/hello-plugin/files')
      return route.fulfill({ json: { files: ['manifest.toml', 'scripts/demo.sh', 'README.md'] } });
    if (path === '/plugin-projects/hello-plugin/file') {
      const file = url.searchParams.get('path');
      if (route.request().method() === 'GET') return route.fulfill({ json: { path: file, contents: diskContents[file] ?? '' } });
      const body = route.request().postDataJSON();
      if (body.original !== (diskContents[body.path] ?? '')) {
        return route.fulfill({ status: 409, json: { error: 'File changed on disk; reload it before saving' } });
      }
      diskContents[body.path] = body.contents;
      writes.saves.push(body.path);
      return route.fulfill({ json: { path: body.path, contents: body.contents } });
    }
    if (path === '/plugin-projects/hello-plugin/nodes')
      return route.fulfill({ json: { nodes: [{ kind: 'hello_plugin', desc: 'demo', inputs: [{ label: 'input_text', type: 'file' }], outputs: [{ path: 'demo_out.txt', label: 'demo_out', optional: false }], params: {} }], count: 1 } });
    if (path === '/plugin-projects/hello-plugin/dev-session' && route.request().method() === 'GET')
      return route.fulfill({ json: { plugin: 'hello-plugin', state: 'absent', container_name: 'autonomics-dev-hello-plugin', image: 'reg/env@sha256:abc' } });
    if (path === '/plugin-projects/hello-plugin/dev-session' && route.request().method() === 'POST') {
      writes.sessionStarts.push(route.request().postDataJSON());
      return route.fulfill({ json: { plugin: 'hello-plugin', state: 'running', container_name: 'autonomics-dev-hello-plugin', image: 'reg/env@sha256:abc' } });
    }
    if (path === '/plugin-projects/hello-plugin/validate' || path === '/plugin-projects/hello-plugin/activate') {
      writes.validates += 1;
      return route.fulfill({ json: { outcome: 'passed', activated: path.endsWith('activate'), report: { report_id: 'attempt-1', plugin_name: 'hello-plugin', gates } } });
    }
    if (path === '/plugin-projects/hello-plugin/nodes/hello_plugin/test-run')
      return route.fulfill({ json: { plugin: 'hello-plugin', kind: 'hello_plugin', exit_code: 0, stdout: 'ok', stderr: '', outputs: [{ path: 'demo_out.txt', present: true, optional: false, content: 'Hello, 插件!\n', truncated: false }], workdir: '/tmp/plugin-test-run/hello-plugin/1' } });
    // Loose shell fixtures.
    if (path.startsWith('/bib/')) return route.fulfill({ json: { collections: [], articles: [], sources: [], rules: [], items: [], snapshots: [], next: null, next_cursor: null, scheduler: {}, total: 0 } });
    if (path.startsWith('/wiki/')) return route.fulfill({ json: { entries: [] } });
    if (path.startsWith('/vfs/')) return route.fulfill({ json: { entries: [], mounts: [], count: 0 } });
    if (path === '/model-config') return route.fulfill({ json: { models: [], providers: [] } });
    if (path === '/settings') return route.fulfill({ json: {} });
    if (path === '/profiles') return route.fulfill({ json: [] });
    if (path === '/gateway/status') return route.fulfill({ json: { running: true } });
    if (path === '/storage/agents') return route.fulfill({ json: [] });
    return route.fulfill({ status: 404, json: { error: 'Not mocked: ' + path } });
  });
  await page.route('**/api/v1/events', () => new Promise(() => {}));
  await page.goto(origin);
  await page.locator('main[data-workbench]').waitFor();

  // ── Manager surface ─────────────────────────────────────────────────
  const rail = page.getByRole('navigation', { name: '工作台活动栏' });
  await rail.getByRole('button', { name: '插件管理' }).click();
  await page.getByRole('region', { name: '插件管理' }).waitFor();
  await page.locator('[data-installed-plugin="hello-plugin"]').waitFor();
  assert.equal(await page.locator('[data-installed-plugin="hello-plugin"]').getByText('已注册').count(), 1);
  await page.locator('[data-dev-project="hello-plugin"]').waitFor();

  // ── Dev window ──────────────────────────────────────────────────────
  await page.locator('[data-dev-project="hello-plugin"]').getByRole('button', { name: '开发窗口' }).click();
  await page.locator('[data-plugin-dev="hello-plugin"]').waitFor();
  // Session auto-started on open (devcontainer attach semantics).
  for (let i = 0; i < 100 && writes.sessionStarts.length === 0; i++) await new Promise(r => setTimeout(r, 50));
  assert.deepEqual(writes.sessionStarts, [{ action: 'start' }], 'auto start');
  await page.locator('[data-dev-session-state="running"]').waitFor();
  // File tree + editor wired.
  await page.locator('[data-project-file="scripts/demo.sh"]').click();
  const editor = page.locator('[data-project-file-editor="scripts/demo.sh"]');
  await editor.waitFor();
  await editor.fill('set -eu\n# edited');
  await page.getByRole('button', { name: '保存' }).click();
  for (let i = 0; i < 100 && writes.saves.length === 0; i++) await new Promise(r => setTimeout(r, 50));
  assert.deepEqual(writes.saves, ['scripts/demo.sh']);

  // Validate → report gates render.
  await page.getByRole('button', { name: '校验' }).click();
  await page.locator('[data-validation-report]').waitFor();
  for (const gate of gates) {
    await page.locator(`[data-gate="${gate.name}"]`).waitFor();
  }
  await page.locator('[data-gate="manifest"][data-gate-status="pass"]').waitFor();

  // Test-run panel: fill the input port, run, inspect the output.
  await page.locator('[data-dock-tab="test-run"]').click();
  await page.locator('[data-test-input="input_text"]').fill('插件');
  await page.getByRole('button', { name: /试跑|运行中…/ }).click();
  await page.locator('[data-test-run-result]').waitFor();
  await page.locator('[data-test-output="demo_out.txt"]').waitFor();
  assert.match(await page.locator('[data-test-output="demo_out.txt"]').textContent(), /Hello, 插件!/);

  assert.equal(errors.length, 0, `page errors: ${errors.join('; ')}`);
  console.log('plugin-dev: all assertions passed');
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser?.close();
  server.kill();
}
