import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
const origin = 'http://127.0.0.1:1439';
const owner = 'a2666ff7-a305-456e-8f4e-a7733b7bc2f0', original = 'b2666ff7-a305-456e-8f4e-a7733b7bc2f0', missing = 'c2666ff7-a305-456e-8f4e-a7733b7bc2f0';
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNQaHjwHwAExAKAc00zmAAAAABJRU5ErkJggg==', 'base64');
const entry = { id: owner, title: '图片笔记', content: `# 图片笔记\n\n持续编辑起点\n\n![legacy](wiki-attach://${original}/old.png)\n\n![canonical](wiki-attach://${owner}/${original})\n\n![http](/api/v1/wiki/entries/${owner}/attachments/${original}/raw)\n\n![missing](wiki-attach://${owner}/${missing})\n\n<img src=x onerror="window.imageXss=true">\n`, alias: [], tags: [], ctime: 1, mtime: 2 };
const attachments = [{ attachment_id: original, filename: 'old.png', mime: 'image/png', size: png.length, sha256: 'test' }];
let count = 0, hold = false, failures = 0, gates = [], holdSaves = false, saveGates = [];
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1439', '--strictPort']);
let output = '', browser;
vite.stdout.on('data', v => output += v); vite.stderr.on('data', v => output += v);
const until = async fn => { for (let i = 0; !fn(); i++) { if (i > 500) throw Error('condition timeout'); await new Promise(r => setTimeout(r, 20)); } };
async function mount(page, native = false) {
  await page.route(origin + '/', r => r.fulfill({ contentType: 'text/html', body: '<html><body></body></html>' }));
  await page.route('**/api/v1/**', async r => {
    const path = new URL(r.request().url()).pathname, method = r.request().method();
    if (path.endsWith('/raw')) return r.fulfill(path.includes(missing) ? { status: 404, body: '' } : { contentType: 'image/png', body: png });
    if (path.endsWith('/attachments') && method === 'POST') {
      count++;
      const id = `d2666ff7-a305-456e-8f4e-${String(count).padStart(12, '0')}`;
      const filename = /filename="([^"]+)"/.exec(r.request().postDataBuffer().toString())?.[1] ?? 'image.png';
      if (hold) await new Promise(resolve => gates.push(resolve));
      if (failures > 0) { failures--; return r.fulfill({ status: 400, json: { error: '模拟上传失败' } }); }
      const meta = { attachment_id: id, filename, mime: 'image/png', size: png.length, sha256: 'test' };
      attachments.push(meta); return r.fulfill({ json: meta });
    }
    if (path.endsWith('/attachments')) return r.fulfill({ json: { attachments } });
    if (path.endsWith('/content') && method === 'PUT') { const content = r.request().postDataJSON().content; if (holdSaves) await new Promise(resolve => saveGates.push(resolve)); entry.content = content; return r.fulfill({ json: {} }); }
    if (path === `/api/v1/wiki/entries/${owner}`) return r.fulfill({ json: entry });
    return r.fulfill({ json: { entry: { id: owner, title: entry.title }, in_links: [], out_links: [], unlinked_mentions: [], versions: [] } });
  });
  if (native) await page.addInitScript(({ pngBase64, entry }) => {
    window.nativeCalls = []; window.revocations = [];
    const revoke = URL.revokeObjectURL;
    URL.revokeObjectURL = url => { window.revocations.push(url); revoke.call(URL, url); };
    window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
      window.nativeCalls.push({ command, args });
      if (command === 'api') {
        if (args.method === 'PUT' && args.path.endsWith('/content')) { window.nativeContent = args.body.content; return { data: {} }; }
        return { data: args.path.endsWith('/attachments') ? { attachments: [] } : args.path.endsWith(entry.id) ? entry : { in_links: [], out_links: [], unlinked_mentions: [], versions: [] } };
      }
      if (command === 'wiki_image_raw') return { data_base64: pngBase64, content_type: 'image/png' };
      if (command === 'upload_wiki_image') return { attachment_id: 'e2666ff7-a305-456e-8f4e-a7733b7bc2f0', filename: args.filename, mime: 'image/png' };
      throw Error(command);
    } };
  }, { pngBase64: png.toString('base64'), entry });
  await page.goto(origin);
  await page.evaluate(async owner => {
    await import('/src/app.css');
    const { default: refresh } = await import('/@react-refresh');
    refresh.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => t => t; window.__vite_plugin_react_preamble_installed__ = true;
    const { mount } = await import('/tests/wiki-images-mount.tsx'); mount(owner);
  }, owner);
  await page.getByRole('heading', { name: entry.title }).first().waitFor();
}
try {
  await until(() => output.includes(origin) || vite.exitCode !== null); if (vite.exitCode !== null) throw Error(output);
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || '/usr/bin/chromium', args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1300, height: 1000 } });
  const errors = []; page.on('pageerror', e => errors.push(e.message));
  await mount(page);
  for (const alt of ['legacy', 'canonical', 'http']) await page.waitForFunction(alt => document.querySelector(`img[alt="${alt}"]`)?.naturalWidth > 0, alt);
  await page.getByText('图片不存在或无法访问：missing', { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.imageXss), undefined);
  await page.locator('img[alt="canonical"]').click(); await page.getByRole('dialog').waitFor(); await page.keyboard.press('Escape');
  const edit = () => page.getByTitle('编辑模式', { exact: true }).click();
  const save = async () => { await page.waitForFunction(() => !document.querySelector('button[title="保存 Ctrl+S"]').disabled); await page.getByRole('button', { name: '保存', exact: true }).click(); await page.getByRole('heading', { name: entry.title }).first().waitFor(); };
  const upload = files => page.locator('input[type="file"]').setInputFiles(files.map(name => ({ name, mimeType: 'image/png', buffer: png })));
  await edit(); await page.locator('.wiki-block', { hasText: '持续编辑起点' }).click();
  hold = true; await upload(['first [1] _x_ $y$.png', 'second.png']); await until(() => gates.length === 2);
  const textarea = page.getByRole('textbox', { name: '块内容(markdown 源文)' });
  await textarea.press('Control+Home'); await textarea.pressSequentially('上传期间保留');
  assert.equal(await page.getByRole('button', { name: '保存', exact: true }).isDisabled(), true);
  gates[1](); await page.getByText('second.png：上传中…', { exact: true }).waitFor({ state: 'detached' });
  gates[0](); hold = false; gates = []; await page.getByText('first [1] _x_ $y$.png：上传中…', { exact: true }).waitFor({ state: 'detached' });
  await page.keyboard.press('Escape'); await save();
  assert.ok(entry.content.includes('上传期间保留持续编辑起点')); assert.ok(entry.content.indexOf('first') < entry.content.indexOf('second')); assert.ok(!entry.content.includes('wiki-image-upload:'));
  await page.getByRole('img', { name: 'first [1] _x_ $y$.png', exact: true }).waitFor();
  await page.waitForFunction(() => [...document.querySelectorAll('.wiki-md img')].some(img => img.alt.includes('first') && img.naturalWidth > 0));
  await mount(page); await page.waitForFunction(() => [...document.querySelectorAll('.wiki-md img')].some(img => img.alt.includes('second') && img.naturalWidth > 0));
  await edit(); await page.getByRole('button', { name: '插入已有图片', exact: true }).click(); await page.getByRole('button', { name: '插入 old.png', exact: true }).click(); assert.equal(count, 2);
  failures = 1; await upload(['retry.png']); await page.getByRole('button', { name: '重试', exact: true }).click(); await page.getByText('retry.png：上传中…', { exact: true }).waitFor({ state: 'detached' });
  hold = true; await upload(['cancel.png']); await until(() => gates.length > 0);
  await page.locator('.wiki-image-pending').first().waitFor();
  await page.getByRole('button', { name: '移除占位', exact: true }).click(); gates[0](); hold = false; gates = [];
  await page.keyboard.press('Escape'); await save(); assert.ok(!entry.content.includes('cancel.png'));
  assert.ok(entry.content.includes('retry.png'));
  assert.ok(attachments.some(item => item.filename === 'cancel.png'), 'cancelled insertion retains the uploaded attachment');
  await edit();
  await page.locator('.wiki-toolbar').evaluate((el, base64) => {
    for (const [name, type] of [['pasted.png', 'paste'], ['dropped.png', 'drop']]) {
      const dt = new DataTransfer(); dt.items.add(new File([Uint8Array.from(atob(base64), c => c.charCodeAt(0))], name, { type: 'image/png' }));
      el.dispatchEvent(type === 'paste' ? new ClipboardEvent(type, { clipboardData: dt, bubbles: true, cancelable: true }) : new DragEvent(type, { dataTransfer: dt, bubbles: true, cancelable: true }));
    }
  }, png.toString('base64'));
  await save(); assert.ok(entry.content.includes('pasted.png') && entry.content.includes('dropped.png'));
  // Removing an upload's entire target block must not resurrect it on completion.
  await edit(); await page.locator('.wiki-block', { hasText: '上传期间保留持续编辑起点' }).click();
  hold = true; await upload(['deleted-block.png']); await until(() => gates.length > 0);
  await page.keyboard.press('Escape');
  await page.locator('.wiki-block', { hasText: '上传期间保留持续编辑起点' }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: '删除段落' }).click();
  gates[0](); hold = false; gates = [];
  await page.getByText('deleted-block.png：上传中…', { exact: true }).waitFor({ state: 'detached' });
  await save(); assert.ok(!entry.content.includes('deleted-block.png'));
  // Closing an editor drops its placeholders; the completed upload remains reusable.
  await edit(); hold = true; await upload(['closed-editor.png']); await until(() => gates.length > 0);
  await page.evaluate(async () => { (await import('/tests/wiki-images-mount.tsx')).unmount(); });
  gates[0](); hold = false; gates = []; await until(() => attachments.some(item => item.filename === 'closed-editor.png'));
  await page.evaluate(async owner => { (await import('/tests/wiki-images-mount.tsx')).mount(owner); }, owner);
  await edit(); await save();
  assert.ok(!entry.content.includes('closed-editor.png') && !entry.content.includes('wiki-image-upload:'));
  // Discarding a draft during upload must keep the accepted baseline.
  const baseline = entry.content;
  await edit(); hold = true; await upload(['discarded.png']); await until(() => gates.length > 0);
  page.once('dialog', dialog => dialog.accept());
  await page.getByTitle('阅读模式', { exact: true }).click();
  gates[0](); hold = false; gates = []; await until(() => attachments.some(item => item.filename === 'discarded.png'));
  await edit(); await save(); assert.equal(entry.content, baseline);
  const before = count; await page.getByText('附件', { exact: true }).click(); await page.getByRole('button', { name: '插入正文', exact: true }).first().click(); await save(); assert.equal(count, before);
  // Starting an upload while a save is pending keeps the newer draft in edit mode.
  await edit(); await page.locator('.wiki-block').first().click();
  const savingText = page.getByRole('textbox', { name: '块内容(markdown 源文)' });
  await savingText.fill((await savingText.inputValue()) + '\n\n保存期间编辑');
  holdSaves = true; await page.getByRole('button', { name: '保存', exact: true }).click();
  await until(() => saveGates.length > 0); await upload(['during-save.png']);
  await page.getByText('during-save.png：上传中…', { exact: true }).waitFor({ state: 'detached' });
  saveGates[0](); holdSaves = false; saveGates = [];
  await page.waitForFunction(() => !document.querySelector('button[title="保存 Ctrl+S"]')?.disabled);
  assert.equal(await page.locator('input[type="file"]').count(), 1, 'newer edits survive the previous save');
  await page.keyboard.press('Escape'); await save(); assert.ok(entry.content.includes('during-save.png'));
  // Split views share pending-upload state and preserve edits from the other pane.
  await page.evaluate(async owner => { const fixture = await import('/tests/wiki-images-mount.tsx'); fixture.unmount(); fixture.mount(owner, true); }, owner);
  const firstPane = page.locator('[data-wiki-pane="first"]'), secondPane = page.locator('[data-wiki-pane="second"]');
  await firstPane.getByRole('button', { name: '保存', exact: true }).waitFor();
  hold = true;
  await firstPane.locator('input[type="file"]').setInputFiles({ name: 'split.png', mimeType: 'image/png', buffer: png });
  await until(() => gates.length > 0);
  assert.equal(await secondPane.getByRole('button', { name: '保存', exact: true }).isDisabled(), true);
  await secondPane.locator('.wiki-block').first().click();
  const sharedText = secondPane.getByRole('textbox', { name: '块内容(markdown 源文)' });
  await sharedText.fill((await sharedText.inputValue()) + '\n\n并排保留');
  gates[0](); hold = false; gates = [];
  await firstPane.getByText('split.png：上传中…', { exact: true }).waitFor({ state: 'detached' });
  await firstPane.getByRole('button', { name: '保存', exact: true }).click();
  await firstPane.getByRole('heading', { name: entry.title }).first().waitFor();
  assert.ok(entry.content.includes('并排保留') && entry.content.includes('split.png'));
  assert.deepEqual(errors, []);
  const native = await browser.newPage(); await mount(native, true);
  await native.waitForFunction(() => document.querySelector('img[data-wiki-attachment]')?.src.startsWith('blob:') && document.querySelector('img[data-wiki-attachment]').naturalWidth > 0);
  assert.equal(await native.evaluate(id => window.nativeCalls.filter(call => call.command === 'wiki_image_raw' && call.args.attachmentId === id).length, original), 1);
  await native.getByTitle('编辑模式', { exact: true }).click(); await native.locator('input[type="file"]').setInputFiles({ name: 'desktop.png', mimeType: 'image/png', buffer: png });
  await native.waitForFunction(() => window.nativeCalls.some(call => call.command === 'upload_wiki_image'));
  assert.equal(await native.evaluate(() => window.nativeCalls.find(call => call.command === 'upload_wiki_image').args.dataBase64), png.toString('base64'));
  await native.waitForFunction(() => !document.querySelector('button[title="保存 Ctrl+S"]').disabled);
  await native.getByRole('button', { name: '保存', exact: true }).click(); await native.waitForFunction(() => window.nativeContent?.includes('desktop.png'));
  assert.ok(await native.evaluate(() => window.revocations.length > 0));
  console.log('wiki images: portable/legacy refs, preview, concurrent edits, ordered uploads, retry/removal, reuse, paste/drop, reload and desktop bridges passed');
} finally { await browser?.close(); vite.kill('SIGTERM'); }
