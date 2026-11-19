import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const origin = 'http://127.0.0.1:1448';
const owner = 'a2666ff7-a305-456e-8f4e-a7733b7bc2f0';
const pngBase64 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNQaHjwHwAExAKAc00zmAAAAABJRU5ErkJggg==';
const png = Buffer.from(pngBase64, 'base64');
const entry = { id: owner, title: '图片粘贴', content: '# 图片粘贴\n\n保留原有文字\n\n最后一段', alias: [], tags: [], ctime: 1, mtime: 1 };
let browser, count = 0, hold = false, failures = 0;
const gates = [];
const requests = [];
const errors = [];
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1448', '--strictPort']);
let output = '';
vite.stdout.on('data', data => output += data);
vite.stderr.on('data', data => output += data);
const until = async predicate => {
  const deadline = Date.now() + 30000;
  while (!predicate()) {
    assert.ok(Date.now() < deadline, 'condition timeout');
    await new Promise(resolve => setTimeout(resolve, 20));
  }
};
async function mount(page, native = false) {
  page.on('pageerror', error => errors.push(error.message));
  await page.route(origin + '/', route => route.fulfill({ contentType: 'text/html', body: '<html><body></body></html>' }));
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname;
    const method = route.request().method();
    if (path.endsWith('/raw')) return route.fulfill({ contentType: 'image/png', body: png });
    if (path.endsWith('/attachments') && method === 'POST') {
      const filename = /filename="([^"]+)"/.exec(route.request().postDataBuffer().toString())?.[1];
      const attachment_id = `b2666ff7-a305-456e-8f4e-${String(++count).padStart(12, '0')}`;
      requests.push(filename);
      if (hold) await new Promise(resolve => gates.push(resolve));
      if (failures > 0) { failures--; return route.fulfill({ status: 503, json: { error: '测试上传失败' } }); }
      return route.fulfill({ json: { attachment_id, filename, mime: 'image/png' } });
    }
    if (path.endsWith('/content') && method === 'PUT') {
      entry.content = route.request().postDataJSON().content;
      return route.fulfill({ json: {} });
    }
    return route.fulfill({ json: path.endsWith(owner) ? entry : { attachments: [], in_links: [], out_links: [], unlinked_mentions: [], versions: [] } });
  });
  if (native) await page.addInitScript(({ entry, pngBase64 }) => {
    window.nativeCalls = [];
    window.revocations = [];
    const revoke = URL.revokeObjectURL;
    URL.revokeObjectURL = url => { window.revocations.push(url); revoke.call(URL, url); };
    window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
      window.nativeCalls.push({ command, args });
      if (command === 'api') return { data: args.path.endsWith(entry.id) ? entry : {} };
      if (command === 'upload_wiki_image') return { attachment_id: 'c2666ff7-a305-456e-8f4e-a7733b7bc2f0', filename: args.filename, mime: 'image/png' };
      if (command === 'wiki_image_raw') return { data_base64: pngBase64, content_type: 'image/png' };
      throw Error(command);
    } };
  }, { entry: { ...entry, content: '# 桌面粘贴\n\n正文\n\n末尾' }, pngBase64 });
  await page.goto(origin);
  page.setDefaultTimeout(20000);
  await page.evaluate(async owner => {
    await import('/src/app.css');
    const { default: refresh } = await import('/@react-refresh');
    refresh.injectIntoGlobalHook(window);
    window.$RefreshReg$ = () => {};
    window.$RefreshSig$ = () => value => value;
    window.__vite_plugin_react_preamble_installed__ = true;
    const { mount } = await import('/tests/wiki-images-mount.tsx');
    mount(owner, true);
  }, owner);
  await page.locator('[data-wiki-pane="first"] .vditor-ir [contenteditable="true"]').waitFor();
}
const first = page => page.locator('[data-wiki-pane="first"]');
const saveButton = page => first(page).getByRole('button', { name: '保存', exact: true });
const content = page => first(page).locator('.vditor-ir [contenteditable="true"]');
const draft = page => page.evaluate(async owner => (await import('/src/features/wiki/wiki-document.ts')).findWikiDocument(owner).state.content, owner);
async function paste(page, names, variant = 'items', target = content(page)) {
  return target.evaluate((el, { names, variant, pngBase64 }) => {
    const data = new DataTransfer();
    for (const name of names) data.items.add(new File([Uint8Array.from(atob(pngBase64), c => c.charCodeAt(0))], name, { type: 'image/png' }));
    if (variant === 'items') Object.defineProperty(data, 'files', { value: [] });
    if (variant === 'files') Object.defineProperty(data, 'items', { value: [] });
    const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
    el.dispatchEvent(event);
    return event.defaultPrevented;
  }, { names, variant, pngBase64 });
}
async function settled(page) {
  await saveButton(page).waitFor();
  await page.waitForFunction(() => !document.querySelector('[data-wiki-pane="first"] button[title="保存 Ctrl+S"]').disabled);
}

try {
  await until(() => output.includes(origin) || vite.exitCode !== null);
  assert.equal(vite.exitCode, null, output);
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || '/usr/bin/chromium', args: ['--no-sandbox'] });
  const context = await browser.newContext({ permissions: ['clipboard-read', 'clipboard-write'] });
  const page = await context.newPage();
  await mount(page);
  await content(page).click();
  await content(page).press('Control+End');

  // Exercise the browser's actual clipboard pipeline and keyboard shortcut.
  await page.evaluate(async pngBase64 => {
    const blob = new Blob([Uint8Array.from(atob(pngBase64), c => c.charCodeAt(0))], { type: 'image/png' });
    await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
  }, pngBase64);
  await page.keyboard.press('Control+V');
  await until(() => requests.length === 1);
  await settled(page);
  assert.match(await draft(page), /wiki-attach:\/\//);

  // Items-only clipboard, ordered multi-image insertion, edits while uploading,
  // and shared save state in the second editor pane.
  hold = true;
  assert.equal(await paste(page, ['first.png', 'second.png']), true);
  await until(() => gates.length === 2);
  assert.equal(await saveButton(page).isDisabled(), true);
  assert.equal(await page.locator('[data-wiki-pane="second"]').getByRole('button', { name: '保存', exact: true }).isDisabled(), true);
  await first(page).getByRole('button', { name: '双列编辑', exact: true }).click();
  const source = first(page).locator('.vditor-sv');
  assert.ok((await source.inputValue()).includes('wiki-image-upload:'));
  await source.press('Control+End');
  await page.keyboard.type('上传期间继续编辑');
  gates.pop()();
  await first(page).getByText('second.png：上传中…', { exact: true }).waitFor({ state: 'detached' });
  gates.pop()(); hold = false;
  await settled(page);
  await page.waitForFunction(() => [...document.querySelectorAll('[data-wiki-pane="first"] .vditor-preview img')].some(img => img.alt === 'first.png' && img.naturalWidth > 0));
  await first(page).getByRole('button', { name: '实时渲染', exact: true }).click();
  const edited = await draft(page);
  assert.ok(edited.includes('保留原有文字') && edited.includes('上传期间继续编辑'), edited);
  assert.ok(edited.indexOf('first.png') < edited.indexOf('second.png'));
  assert.ok(!edited.includes('wiki-image-upload:'));

  // Images can be pasted while a rendered paragraph is the event target.
  const preview = content(page).locator('p').filter({ hasText: '保留原有文字' });
  assert.equal(await paste(page, ['preview.png'], 'files', preview), true);
  await settled(page);
  assert.ok((await draft(page)).includes('preview.png'));

  // A dual representation must upload only once.
  const before = requests.length;
  await paste(page, ['both.png'], 'both'); await settled(page);
  assert.equal(requests.length, before + 1);

  // Text continues to use Vditor's normal paste behavior.
  await content(page).focus();
  await page.evaluate(() => navigator.clipboard.writeText('普通文本粘贴'));
  await page.keyboard.press('Control+V');
  await page.waitForFunction(async owner => (await import('/src/features/wiki/wiki-document.ts')).findWikiDocument(owner).state.content.includes('普通文本粘贴'), owner);

  // Failed uploads retain their insertion point and can be retried or removed.
  failures = 1;
  await paste(page, ['retry.png']);
  await first(page).getByRole('button', { name: '重试', exact: true }).click();
  await settled(page);
  assert.ok((await draft(page)).includes('retry.png'));
  failures = 1;
  await paste(page, ['remove.png']);
  await first(page).getByRole('button', { name: '重试', exact: true }).waitFor();
  await first(page).getByRole('button', { name: '移除占位', exact: true }).click();
  await settled(page);
  assert.ok(!(await draft(page)).includes('wiki-image-upload:'));

  // Cancelling an insertion while its upload is in flight must not resurrect it.
  hold = true;
  await paste(page, ['cancel.png']); await until(() => gates.length === 1);
  await first(page).getByRole('button', { name: '移除占位', exact: true }).click();
  gates.pop()(); hold = false; await settled(page);
  assert.ok(!(await draft(page)).includes('cancel.png'));

  await saveButton(page).click();
  await first(page).getByRole('heading', { name: entry.title, exact: true }).waitFor();
  assert.ok(entry.content.includes('first.png') && entry.content.includes('普通文本粘贴'));
  await page.waitForFunction(() => [...document.querySelectorAll('[data-wiki-pane="first"] img')].some(img => img.alt === 'first.png' && img.naturalWidth > 0));

  // Closing the editor during an upload removes its markers from the shared draft.
  await first(page).getByTitle('编辑模式', { exact: true }).click();
  hold = true;
  await paste(page, ['closed.png']); await until(() => gates.length === 1);
  await page.evaluate(async () => (await import('/tests/wiki-images-mount.tsx')).unmount());
  assert.ok(!(await draft(page)).includes('wiki-image-upload:'));
  gates.pop()(); hold = false;

  // Desktop uploads use the native bridge; the live preview must resolve blobs.
  const native = await browser.newPage();
  await mount(native, true);
  await paste(native, ['desktop.png']); await settled(native);
  await content(native).press('Control+Home');
  await native.waitForFunction(() => [...document.querySelectorAll('.vditor-ir img')].some(img => img.alt === 'desktop.png' && img.src.startsWith('blob:') && img.naturalWidth > 0));
  const upload = await native.evaluate(() => window.nativeCalls.find(call => call.command === 'upload_wiki_image'));
  assert.equal(upload.args.dataBase64, pngBase64);
  for (const mode of ['wysiwyg', 'sv', 'ir']) {
    const shortcut = { wysiwyg: '7', sv: '9', ir: '8' }[mode];
    await native.keyboard.press(`Alt+Control+${shortcut}`);
    await first(native).locator(mode === 'sv' ? '.vditor-sv' : `.vditor-${mode} [contenteditable="true"]`).waitFor({ state: 'visible' });
  }
  await saveButton(native).click();
  await native.waitForFunction(() => window.nativeCalls.some(call => call.command === 'api' && call.args.method === 'PUT' && call.args.path.endsWith('/content')));
  const saved = await native.evaluate(() => window.nativeCalls.find(call => call.command === 'api' && call.args.method === 'PUT' && call.args.path.endsWith('/content')).args.body.content);
  assert.ok(saved.includes('wiki-attach://') && !saved.includes('blob:') && !saved.includes('/attachments/'), saved);
  await native.evaluate(async () => (await import('/tests/wiki-images-mount.tsx')).unmount());
  await native.waitForFunction(() => window.revocations.length > 0);
  assert.deepEqual(errors, []);
  console.log('Wiki image paste: real clipboard, items/files, preview, ordering, continued edits, retry/removal, save/readback, cleanup and desktop image loading passed');
} finally {
  for (const release of gates) release();
  await browser?.close();
  vite.kill();
}
