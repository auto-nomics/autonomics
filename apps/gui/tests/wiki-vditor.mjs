import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const origin = 'http://127.0.0.1:1441';
const owner = 'a2666ff7-a305-456e-8f4e-a7733b7bc2f0';
const image = 'b2666ff7-a305-456e-8f4e-a7733b7bc2f0';
const target = { id: 'c2666ff7-a305-456e-8f4e-a7733b7bc2f0', title: '相关词条', content: '关联说明', alias: [], tags: [], ctime: 1, mtime: 1 };
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNQaHjwHwAExAKAc00zmAAAAABJRU5ErkJggg==', 'base64');
const initial = [
  '# 编辑器迁移', '', '第一段 **粗体**，[[相关词条]] 与 [[相关词条|别名显示]]，[[未建立]]。`[[代码字面量]]`', '',
  '- [ ] 待办事项', '- [x] 已完成事项', '',
  '| 列一 | 列二 |', '| --- | --- |', '| A | B |', '| C | D |', '',
  '行内公式 $E=mc^2$。', '', '$$', '\\int_0^1 x^2 dx', '$$', '',
  '括号公式 \\(a^2+b^2=c^2\\)。`\\(literal\\)`', '', '\\[', '\\sum_{i=1}^n i', '\\]', '',
  '```typescript', 'const answer = 42;', '// [[代码块字面量]]', '```', '',
  '```mermaid', 'graph LR', '  A-->B', '```', '',
  `![原有图片](wiki-attach://${owner}/${image})`, '', '最后一段', '',
].join('\n');
const entry = { id: owner, title: '编辑器迁移', content: initial, alias: [], tags: [], ctime: 1, mtime: 1 };
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1441', '--strictPort']);
let output = '', browser, page;
vite.stdout.on('data', data => output += data);
vite.stderr.on('data', data => output += data);
const errors = [], external = [], missing = [];
const draft = () => page.evaluate(async owner => (await import('/src/features/wiki/wiki-document.ts')).findWikiDocument(owner).state.content, owner);
const first = () => page.locator('[data-wiki-pane="first"]');
const editor = () => first().locator('.vditor-ir [contenteditable="true"]');
const mode = async name => {
  const label = { wysiwyg: '所见即所得', ir: '实时渲染', sv: '双列编辑' }[name];
  const button = first().getByRole('group', { name: 'Wiki 编辑方式' }).getByRole('button', { name: label, exact: true });
  await button.click();
  assert.equal(await button.getAttribute('aria-pressed'), 'true');
};
try {
  const deadline = Date.now() + 30000;
  while (!output.includes(origin)) {
    assert.equal(vite.exitCode, null, output); assert.ok(Date.now() < deadline, output);
    await new Promise(resolve => setTimeout(resolve, 30));
  }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || '/usr/bin/chromium', args: ['--no-sandbox'] });
  page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
  page.setDefaultTimeout(20000);
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => { if (/^https?:/.test(request.url()) && !request.url().startsWith(origin)) external.push(request.url()); });
  page.on('response', response => { if (response.status() >= 400) missing.push(`${response.status()} ${response.url()}`); });
  await page.route(origin + '/', route => route.fulfill({ contentType: 'text/html', body: '<html><body></body></html>' }));
  await page.route('**/api/v1/**', route => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    if (path.endsWith('/raw')) return route.fulfill({ contentType: 'image/png', body: png });
    if (path.endsWith('/content') && route.request().method() === 'PUT') {
      entry.content = route.request().postDataJSON().content;
      return route.fulfill({ json: {} });
    }
    if (path.endsWith('/search')) return route.fulfill({ json: { results: [{ entry_id: target.id, score: 1 }] } });
    if (path.endsWith(target.id)) return route.fulfill({ json: target });
    if (path.endsWith(owner)) return route.fulfill({ json: entry });
    if (path.endsWith('/entries')) return route.fulfill({ json: { entries: url.searchParams.get('title') === '未建立' ? [] : [target] } });
    return route.fulfill({ json: { entries: [target], attachments: [], in_links: [], out_links: [], unlinked_mentions: [], versions: [] } });
  });
  await page.goto(origin);
  await page.evaluate(async owner => {
    await import('/src/app.css');
    const { default: refresh } = await import('/@react-refresh');
    refresh.injectIntoGlobalHook(window); window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => value => value;
    window.__vite_plugin_react_preamble_installed__ = true;
    window.wikiNavigations = [];
    const wiki = await import('/src/features/wiki/wiki-state.ts');
    wiki.registerWikiOpener((id, options) => window.wikiNavigations.push({ id, options }));
    const { mount } = await import('/tests/wiki-images-mount.tsx'); mount(owner, true);
  }, owner);
  await editor().waitFor();
  await first().locator('.vditor-ir strong').waitFor();
  await first().locator('.vditor-ir table').waitFor();
  await first().locator('.vditor-ir .katex').first().waitFor();
  await first().locator('.vditor-ir .language-mermaid svg').waitFor();
  await page.waitForFunction(() => document.querySelector('[data-wiki-pane="first"] .vditor-ir img')?.naturalWidth > 0);
  assert.equal(await draft(), initial, 'opening the editor must not normalize the stored document');
  for (const name of ['wysiwyg', 'sv', 'ir']) await mode(name);
  assert.equal(await draft(), initial, 'switching modes alone must not change the draft');
  await editor().locator('[data-wikilink="相关词条"]').filter({ hasText: '别名显示' }).waitFor();
  await editor().locator('[data-wikilink="未建立"].md-wikilink--missing').waitFor();
  assert.equal(await editor().locator('[data-wikilink="代码字面量"], [data-wikilink="代码块字面量"]').count(), 0, 'code stays literal');
  await editor().locator('[data-wikilink="相关词条"]').first().click({ modifiers: ['Control'] });
  await page.waitForFunction(() => window.wikiNavigations.length === 1);
  assert.equal(await page.evaluate(() => window.wikiNavigations[0].id), target.id);
  await editor().locator('[data-wikilink="相关词条"]').filter({ hasText: '别名显示' }).click();
  assert.ok(await editor().locator('p').first().evaluate(element => element.textContent.includes('[[相关词条|别名显示]]')), 'clicking a link exposes editable source');
  await editor().locator('p').filter({ hasText: '最后一段' }).click();
  await editor().locator('[data-wikilink="相关词条"]').filter({ hasText: '别名显示' }).waitFor();
  const gutter = await editor().locator('h1').evaluate(heading => ({
    left: heading.getBoundingClientRect().left + parseFloat(getComputedStyle(heading, '::before').marginLeft),
    viewport: heading.closest('.wiki-vditor-document').getBoundingClientRect().left,
  }));
  assert.ok(gutter.left >= gutter.viewport + 8, 'heading labels have an unclipped gutter');
  const scrolling = await first().locator('.wiki-vditor-document').evaluate(async element => {
    const metadata = element.querySelector('.wiki-editor-metadata');
    element.scrollTop = 0;
    await new Promise(requestAnimationFrame);
    const before = metadata.getBoundingClientRect().top;
    element.scrollTop = 180;
    await new Promise(requestAnimationFrame);
    const result = { distance: before - metadata.getBoundingClientRect().top, scroll: element.scrollTop, position: getComputedStyle(metadata).position };
    element.scrollTop = 0;
    return result;
  });
  assert.ok(scrolling.scroll > 0 && Math.abs(scrolling.distance - scrolling.scroll) < 1, JSON.stringify(scrolling));
  assert.equal(scrolling.position, 'static', 'metadata scrolls with the document');

  await editor().press('Control+End');
  await page.keyboard.type('即时编辑内容');
  // Save immediately, before Vditor's normal undo/input debounce.
  await editor().press('Control+s');
  await first().getByRole('heading', { name: entry.title, exact: true }).waitFor();
  assert.ok(entry.content.includes('即时编辑内容'), entry.content);
  assert.ok(entry.content.includes('[[相关词条]]') && entry.content.includes('[[相关词条|别名显示]]'), entry.content);
  assert.ok(entry.content.includes(`wiki-attach://${owner}/${image}`), entry.content);
  assert.ok(!entry.content.includes('blob:') && !entry.content.includes('/attachments/'), entry.content);

  await first().getByTitle('编辑模式', { exact: true }).click();
  await editor().waitFor();
  await mode('wysiwyg');
  const richEditor = first().locator('.vditor-wysiwyg [contenteditable="true"]');
  await richEditor.press('Control+End');
  await page.keyboard.type('所见即所得编辑验证');
  await mode('sv');
  const source = first().locator('.vditor-sv');
  const preview = first().locator('.vditor-preview');
  await source.waitFor({ state: 'visible' });
  await preview.waitFor({ state: 'visible' });
  const columns = await Promise.all([source.boundingBox(), preview.boundingBox()]);
  assert.ok(columns.every(column => column.width > 100), JSON.stringify(columns));
  assert.ok(columns[1].x >= columns[0].x + columns[0].width - 1, 'source and preview sit side by side');
  assert.ok((await source.inputValue()).includes('所见即所得编辑验证'));
  await source.fill((await source.inputValue()) + '\n\n双列实时预览验证');
  await preview.getByText('双列实时预览验证', { exact: true }).waitFor();
  await mode('ir');
  await editor().getByText('双列实时预览验证', { exact: true }).waitFor();
  await editor().press('Control+End');
  await page.keyboard.type('模式切换后继续编辑');
  await first().getByRole('button', { name: '保存', exact: true }).click();
  await first().getByRole('heading', { name: entry.title, exact: true }).waitFor();
  assert.ok(entry.content.includes('模式切换后继续编辑'), entry.content);
  assert.ok(entry.content.includes('所见即所得编辑验证') && entry.content.includes('双列实时预览验证'), entry.content);
  assert.ok(entry.content.includes('[[相关词条]]'), entry.content);
  assert.ok(entry.content.includes(`wiki-attach://${owner}/${image}`), entry.content);
  assert.ok(entry.content.includes('E=mc^2') && entry.content.includes('\\int_0^1 x^2 dx'), entry.content);
  assert.ok(entry.content.includes('$a^2+b^2=c^2$') && entry.content.includes('\\sum_{i=1}^n i'), entry.content);
  assert.ok(entry.content.includes('`\\(literal\\)`'), 'math-like code stays literal');

  await first().getByTitle('编辑模式', { exact: true }).click();
  await editor().press('Control+End');
  await page.keyboard.press('Enter');
  const previousLinks = (await draft()).split('[[相关词条]]').length;
  await page.keyboard.type('[[');
  await first().locator('.vditor-hint button').filter({ hasText: target.title }).click();
  await page.waitForFunction(async ({ owner, previousLinks }) => (await import('/src/features/wiki/wiki-document.ts')).findWikiDocument(owner).state.content.split('[[相关词条]]').length > previousLinks, { owner, previousLinks });

  // The other pane remains live when one editor is destroyed and re-created.
  const second = page.locator('[data-wiki-pane="second"]');
  await second.locator('.vditor-ir [contenteditable="true"]').press('Control+End');
  await page.keyboard.type('另一分屏编辑');
  await page.waitForFunction(async owner => (await import('/src/features/wiki/wiki-document.ts')).findWikiDocument(owner).state.content.includes('另一分屏编辑'), owner);
  await editor().getByText('另一分屏编辑', { exact: false }).waitFor();
  await page.evaluate(async () => { const theme = await import('/src/lib/theme.ts'); theme.setThemeMode('light'); });
  await page.waitForFunction(() => !document.querySelector('[data-wiki-pane="first"] .vditor')?.classList.contains('vditor--dark'));
  const tableColors = () => first().locator('.vditor-ir table').evaluate(table => {
    const sample = document.createElement('span');
    sample.style.color = 'var(--foreground)';
    sample.style.backgroundColor = 'var(--muted)';
    sample.style.borderColor = 'var(--border)';
    table.append(sample);
    const expected = getComputedStyle(sample);
    const actual = getComputedStyle(table.querySelector('th'));
    const result = { color: actual.color, background: actual.backgroundColor, border: actual.borderColor, expectedColor: expected.color, expectedBackground: expected.backgroundColor, expectedBorder: expected.borderColor };
    sample.remove();
    return result;
  });
  const light = await tableColors();
  assert.equal(light.color, light.expectedColor);
  assert.equal(light.background, light.expectedBackground);
  assert.equal(light.border, light.expectedBorder);
  await page.evaluate(async () => { const theme = await import('/src/lib/theme.ts'); theme.setThemeMode('dark'); });
  await first().locator('.vditor--dark').waitFor();
  const dark = await tableColors();
  assert.equal(dark.color, dark.expectedColor);
  assert.equal(dark.background, dark.expectedBackground);
  assert.equal(dark.border, dark.expectedBorder);
  assert.notEqual(dark.background, light.background, 'table follows the active theme');
  assert.ok(!(await draft()).includes('aliases'), 'metadata must not be serialized into Markdown');

  // Native keyboard commands and the visible selector share the current mode.
  await editor().press('Control+End');
  await page.keyboard.press('Alt+Control+7');
  await first().locator('[data-wiki-mode="wysiwyg"][aria-pressed="true"]').waitFor();
  assert.equal(await page.evaluate(() => localStorage.getItem('autonomics.wiki.editingMode')), 'wysiwyg');
  await page.evaluate(async owner => {
    const fixture = await import('/tests/wiki-images-mount.tsx');
    fixture.unmount(); fixture.mount(owner, true);
  }, owner);
  await first().locator('.vditor-wysiwyg [contenteditable="true"]').waitFor({ state: 'visible' });
  assert.equal(await first().getByRole('button', { name: '所见即所得', exact: true }).getAttribute('aria-pressed'), 'true', 'the selected mode survives reopening');

  assert.deepEqual(errors, []);
  assert.deepEqual(external, [], 'editing must work without a CDN');
  assert.deepEqual(missing, [], 'all runtime assets must be bundled');
  console.log('Vditor: offline initialization, rich Markdown, immediate save, wikilinks, image references, all three modes, completion, split panes and theme passed');
} catch (error) {
  if (page) {
    console.error('page errors:', errors, 'missing:', missing);
    console.error('draft:', await draft().catch(() => 'unavailable'));
    await page.screenshot({ path: '/tmp/wiki-vditor-failure.png', fullPage: true }).catch(() => {});
  }
  throw error;
} finally { await browser?.close(); vite.kill(); }
