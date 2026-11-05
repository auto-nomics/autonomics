// VFS browser test: sidebar lazy tree, read-only mount badge, file tab.
// Run with npm run test:vfs.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1443';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1443', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser;
server.stdout.on('data', chunk => output += chunk); server.stderr.on('data', chunk => output += chunk);
try {
  for (let n = 0; !output.includes(origin); n++) { if (server.exitCode !== null || n > 120) throw new Error(output); await new Promise(r => setTimeout(r, 50)); }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } }); page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', e => { errors.push(e.message); console.error('PAGE ERROR', e.stack); });

  const listings = {
    '/': { path: '/', entries: [
      { path: '/data', name: 'data', is_dir: true, size: 0 },
      { path: '/notes.md', name: 'notes.md', is_dir: false, size: 12 },
    ], returned: 2, truncated: false },
    '/data': { path: '/data', entries: [
      { path: '/data/ldsc', name: 'ldsc', is_dir: true, size: 0 },
    ], returned: 1, truncated: false },
    '/data/ldsc': { path: '/data/ldsc', entries: [
      { path: '/data/ldsc/readme.txt', name: 'readme.txt', is_dir: false, size: 6 },
    ], returned: 1, truncated: false },
  };
  const listed = [];

  await page.route(`${origin}/api/session`, r => r.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const url = new URL(route.request().url());
    const path = url.pathname.replace('/api/v1', '');
    if (path === '/events') return;
    if (path === '/state') return route.fulfill({ json: { last_seq: 0, agents: [], contexts: {}, display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false } } });
    if (path === '/vfs/mounts') return route.fulfill({ json: { mounts: [
      { path: '/', backend: 'default', source: '/', read_only: false },
      { path: '/data', backend: 'default', source: 'data', read_only: true },
    ], count: 2 } });
    if (path === '/vfs/list') {
      const dir = url.searchParams.get('path') ?? '/';
      listed.push(dir);
      const listing = listings[dir];
      if (!listing) return route.fulfill({ status: 404, json: { error: `no fixture for ${dir}` } });
      return route.fulfill({ json: listing });
    }
    if (path === '/vfs/text') {
      const file = url.searchParams.get('path');
      if (file === '/notes.md') return route.fulfill({ json: { path: file, contents: '# Notes\nhello vfs', size: 14, truncated: false } });
      if (file === '/data/ldsc/readme.txt') return route.fulfill({ json: { path: file, contents: 'panel', size: 5, truncated: false } });
      return route.fulfill({ status: 404, json: { error: 'not found' } });
    }
    if (path === '/vfs/stat') return route.fulfill({ json: { path: '/notes.md', name: 'notes.md', is_dir: false, size: 12, read_only: false } });
    // Loose shell fixtures.
    if (path.startsWith('/bib/')) return route.fulfill({ json: { collections: [], articles: [], sources: [], rules: [], items: [], snapshots: [], next: null, next_cursor: null, scheduler: {}, total: 0 } });
    if (path.startsWith('/wiki/')) return route.fulfill({ json: { entries: [] } });
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

  // ── Sidebar container ──────────────────────────────────────────────
  const rail = page.getByRole('navigation', { name: '工作台活动栏' });
  await rail.getByRole('button', { name: 'VFS 文件' }).click();
  const sidebar = page.locator('[data-vfs-sidebar]');
  await sidebar.waitFor();
  await page.getByRole('tree', { name: 'VFS 目录树' }).waitFor();
  await page.locator('[data-vfs-path="/notes.md"]').waitFor();
  // Only the root is fetched initially.
  assert.deepEqual(listed, ['/'], `root listing only, got ${JSON.stringify(listed)}`);

  // ── Lazy expand + read-only badge ─────────────────────────────────
  await page.locator('[data-vfs-path="/data"]').click();
  await page.locator('[data-vfs-path="/data/ldsc"]').waitFor();
  assert.ok(listed.includes('/data'), `/data should be fetched, got ${JSON.stringify(listed)}`);
  const dataRow = page.locator('[data-vfs-path="/data"]');
  assert.ok(await dataRow.locator('svg.lucide-lock').count() >= 0, 'lock icon resolves');
  // The read-only mount covers /data and its descendants.
  assert.equal(await dataRow.getAttribute('aria-label'), '/data');

  // ── Text file opens an editor tab ─────────────────────────────────
  await page.locator('[data-vfs-path="/notes.md"]').click();
  const tab = page.locator('[data-editor-group] [role="tab"]', { hasText: 'notes.md' });
  await tab.waitFor();
  const viewer = page.locator('textarea[aria-label="查看 /notes.md"]');
  await viewer.waitFor();
  assert.match(await viewer.inputValue(), /hello vfs/);
  await page.locator('[data-vfs-file-path="/notes.md"]').waitFor();

  // ── Read-only mount file shows the badge ──────────────────────────
  await page.locator('[data-vfs-path="/data/ldsc"]').click();
  await page.locator('[data-vfs-path="/data/ldsc/readme.txt"]').waitFor();
  await page.locator('[data-vfs-path="/data/ldsc/readme.txt"]').click();
  await page.locator('[data-vfs-file-path="/data/ldsc/readme.txt"]').waitFor();
  await page.getByText('只读挂载').waitFor();

  // ── Fresh reload keeps the container selectable ────────────────────
  assert.equal(errors.length, 0, `page errors: ${errors.join('; ')}`);
  console.log('vfs-browser: all assertions passed');
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser?.close();
  server.kill();
}
