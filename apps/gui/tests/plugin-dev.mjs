// Unified plugin workspace: one entry, merged inventory, creation, code editing,
// lifecycle tools, container controls, rollback/uninstall and legacy tab migration.
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

  const writes = { saves: [], validates: 0, containers: [], actions: [], creates: [] };
  let installed = true, created = false, broken = true;
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
    if (path === '/plugin-projects') {
      if (route.request().method() === 'POST') { writes.creates.push(route.request().postDataJSON()); created = true; return route.fulfill({ json: {} }); }
      return route.fulfill({ json: { projects: [{ name: 'hello-plugin', status: 'Draft', image: 'reg/env@sha256:abc', error: null }, { name: 'broken-plugin', status: null, image: null, error: broken ? 'invalid manifest' : null }, ...(created ? [{ name: 'new-plugin', status: 'Draft', image: null, error: null }] : [])] } });
    }
    if (path === '/plugins')
      return route.fulfill({ json: { plugins: installed ? [{ name: 'hello-plugin', status: 'installed', image: 'reg/env@sha256:abc', kinds: ['hello_plugin'], registered: true }, { name: 'installed-only', status: 'installed', image: null, kinds: [], registered: true }] : [] } });
    if (path === '/plugins/environments')
      return route.fulfill({ json: { environments: [{ id: 'sh-dev', reference: 'reg/env@sha256:abc' }] } });
    if (/^\/plugin-projects\/[^/]+\/(files|dev-files)$/.test(path))
      return route.fulfill({ json: { files: ['manifest.toml', 'scripts/demo.sh', 'README.md'], directories: ['scripts'] } });
    if (path === '/plugin-projects/hello-plugin/file' || path === '/plugin-projects/broken-plugin/dev-file') {
      const file = url.searchParams.get('path');
      if (route.request().method() === 'GET') return route.fulfill({ json: { path: file, contents: diskContents[file] ?? '' } });
      const body = route.request().postDataJSON();
      if (body.original !== (diskContents[body.path] ?? '')) {
        return route.fulfill({ status: 409, json: { error: 'File changed on disk; reload it before saving' } });
      }
      diskContents[body.path] = body.contents;
      if (path.includes('broken-plugin')) broken = false;
      writes.saves.push(body.path);
      return route.fulfill({ json: { path: body.path, contents: body.contents } });
    }
    if (path === '/plugin-projects/hello-plugin/nodes')
      return route.fulfill({ json: { nodes: [{ kind: 'hello_plugin', desc: 'demo', inputs: [{ label: 'input_text', type: 'file' }], outputs: [{ path: 'demo_out.txt', label: 'demo_out', optional: false }], params: {} }], count: 1 } });
    if (/^\/plugin-projects\/[^/]+\/container$/.test(path)) {
      writes.containers.push(`${route.request().method()} ${path}`);
      return route.fulfill({ json: { connected: route.request().method() !== 'DELETE' } });
    }
    if (path.endsWith('/snapshot')) return route.fulfill({ json: { entries: [] } });
    if (/^\/plugins\/[^/]+\/(rollback|uninstall)$/.test(path)) {
      writes.actions.push(path); if (path.endsWith('/uninstall')) installed = false;
      return route.fulfill({ json: {} });
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

  const rail = page.getByRole('navigation', { name: '工作台活动栏' });
  assert.equal(await rail.getByRole('button', { name: '插件项目', exact: true }).count(), 0);
  assert.equal(await rail.getByRole('button', { name: '插件管理', exact: true }).count(), 1);
  await rail.getByRole('button', { name: '插件管理' }).click();
  const inventory = page.getByRole('region', { name: '插件总览' });
  await inventory.locator('[data-plugin="hello-plugin"]').waitFor();
  assert.equal(await inventory.locator('[data-plugin="hello-plugin"]').count(), 1, 'workspace and installation share a row');
  assert.equal(await inventory.locator('[data-plugin="hello-plugin"]').getByText('已注册').count(), 1);
  assert.equal(await inventory.locator('[data-plugin="installed-only"]').getByRole('button', { name: '打开项目' }).count(), 0);
  await inventory.locator('[data-plugin="hello-plugin"]').getByRole('button', { name: '打开项目' }).click();
  await page.locator('[data-dev-session-state="running"]').waitFor();
  assert.deepEqual(writes.containers, ['POST /plugin-projects/hello-plugin/container']);
  const side = page.getByRole('complementary', { name: '插件导航' });
  await side.getByRole('button', { name: 'scripts', exact: true }).click();
  await side.getByRole('button', { name: 'demo.sh', exact: true }).click();
  const editor = page.locator('.monaco-editor textarea.inputarea');
  await editor.waitFor(); await editor.focus();
  await page.keyboard.press('Control+End'); await page.keyboard.type('# edited');
  await page.getByRole('button', { name: '校验', exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: '校验', exact: true }).isDisabled(), true);
  // Browsing the overview preserves unsaved code and uses the same editor tab.
  await page.getByRole('button', { name: '插件总览', exact: true }).click();
  await page.getByRole('button', { name: '返回项目', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.replace(/\u00a0/g, ' ').includes('# edited'));
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('button[title="保存"]')?.disabled);
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
  await page.getByRole('button', { name: /^(试跑|运行中…)$/ }).last().click();
  await page.locator('[data-test-run-result]').waitFor();
  await page.locator('[data-test-output="demo_out.txt"]').waitFor();
  assert.match(await page.locator('[data-test-output="demo_out.txt"]').textContent(), /Hello, 插件!/);

  await page.getByRole('button', { name: '激活', exact: true }).click();
  await page.getByRole('status').filter({ hasText: '已激活，节点已注册。' }).waitFor();
  await page.getByRole('button', { name: '停止容器', exact: true }).click();
  await page.locator('[data-dev-session-state="exited"]').waitFor();
  await page.getByRole('button', { name: '重新连接', exact: true }).click();
  await page.locator('[data-dev-session-state="running"]').waitFor();
  assert.ok(writes.containers.includes('DELETE /plugin-projects/hello-plugin/container'));
  await page.getByRole('button', { name: '插件总览', exact: true }).click();
  page.once('dialog', dialog => dialog.accept());
  await inventory.getByRole('button', { name: '回滚 hello-plugin', exact: true }).click();
  for (let i = 0; i < 100 && !writes.actions.length; i++) await page.waitForTimeout(50);
  page.once('dialog', dialog => dialog.accept());
  await inventory.getByRole('button', { name: '卸载 hello-plugin', exact: true }).click();
  await inventory.locator('[data-plugin="hello-plugin"]:not([data-installed-plugin])').waitFor();
  assert.deepEqual(writes.actions, ['/plugins/hello-plugin/rollback', '/plugins/hello-plugin/uninstall']);
  await page.getByRole('button', { name: '新建插件', exact: true }).last().click();
  await page.getByLabel('插件名称', { exact: true }).fill('new-plugin');
  await page.getByLabel('用途', { exact: true }).fill('demo purpose');
  await page.getByRole('button', { name: '创建并打开项目' }).click();
  await page.getByRole('heading', { name: '插件管理 · new-plugin', exact: true }).waitFor();
  assert.deepEqual(writes.creates, [{ name: 'new-plugin', environment_id: 'sh-dev', purpose: 'demo purpose' }]);
  await rail.getByRole('button', { name: '插件管理', exact: true }).click();
  assert.equal(await page.getByRole('tab', { name: '插件管理', exact: true }).count(), 1, 'entry reuses the workspace');

  // Invalid manifests stay repairable without a development container.
  await page.getByRole('button', { name: '插件总览', exact: true }).click();
  await inventory.locator('[data-plugin="broken-plugin"]').getByRole('button', { name: '打开项目' }).click();
  await page.getByRole('status').filter({ hasText: '修复模式' }).waitFor();
  await side.getByRole('button', { name: 'manifest.toml', exact: true }).click();
  await editor.waitFor(); await editor.focus();
  await page.keyboard.press('Control+End'); await page.keyboard.type('# repaired');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  for (let i = 0; i < 100 && broken; i++) await page.waitForTimeout(50);
  assert.equal(broken, false);
  await page.getByRole('button', { name: '重新连接', exact: true }).click();
  await page.locator('[data-dev-session-state="running"]').waitFor();
  assert.ok(writes.containers.includes('POST /plugin-projects/broken-plugin/container'));

  // A saved layout containing both old entries restores one workspace.
  await page.evaluate(() => {
    const state = JSON.parse(localStorage.getItem('autonomics.workbench.v2'));
    state.groups.main = { editors: [
      { id: 'old-manager', kind: 'plugin-manager', params: {}, state: null, pinned: false },
      { id: 'old-projects', kind: 'plugin-projects', params: {}, state: null, pinned: false },
      { id: 'old-dev', kind: 'plugin-dev', params: { name: 'hello-plugin' }, state: null, pinned: false },
    ], activeId: 'old-dev' };
    state.groups.active = 'main'; state.groups.secondary = null; state.groups.extra = {};
    localStorage.setItem('autonomics.workbench.v2', JSON.stringify(state));
  });
  await page.reload();
  await page.getByRole('heading', { name: '插件管理 · hello-plugin', exact: true }).waitFor();
  assert.equal(await page.getByRole('tab', { name: '插件管理', exact: true }).count(), 1);

  if (process.env.PLUGIN_TEST_SCREENSHOT) await page.screenshot({ path: process.env.PLUGIN_TEST_SCREENSHOT });
  // The mobile drawer exposes the same workspace entry.
  await page.setViewportSize({ width: 390, height: 844 });
  const drawer = page.getByRole('complementary', { name: '工作台侧栏', exact: true });
  if (!(await drawer.isVisible())) await page.keyboard.press('Control+b');
  await drawer.waitFor({ state: 'visible' });
  assert.equal(await drawer.getByRole('button', { name: '插件项目', exact: true }).count(), 0);
  await drawer.getByRole('button', { name: '插件管理', exact: true }).click();
  await drawer.getByRole('button', { name: '折叠侧栏', exact: true }).click();
  assert.equal(await page.getByRole('tab', { name: '插件管理', exact: true }).count(), 1);
  assert.equal(errors.length, 0, `page errors: ${errors.join('; ')}`);
  console.log('plugin-dev: all assertions passed');
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser?.close();
  server.kill();
}
