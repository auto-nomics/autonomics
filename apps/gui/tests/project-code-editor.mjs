import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

const origin = 'http://127.0.0.1:1441';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', ...(process.env.PROJECT_EDITOR_PREVIEW ? ['preview'] : []), '--host', '127.0.0.1', '--port', '1441', '--strictPort']);
let output = '', browser;
server.stdout.on('data', chunk => output += chunk); server.stderr.on('data', chunk => output += chunk);
try {
  for (let n = 0; !output.includes(origin); n++) { if (server.exitCode !== null || n > 120) throw new Error(output); await new Promise(r => setTimeout(r, 50)); }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  page.setDefaultTimeout(20000);
  const errors = [], saves = [], requests = [], terminalInputs = [], languageMessages = [], scriptMessages = [], languageRequests = [], debugMessages = [], debugRequests = [];
  await page.route('http://127.0.0.2:34999/**', route => route.fulfill({ contentType: 'text/html', body: '<p>isolated workbench</p>' }));
  const files = new Map([['src/main.py', 'print(1)\n'], ['src/other.py', 'def target():\n    pass\n'], ['src/watch.py', 'original\n'], ['src/example.tsx', 'const greeting: string = "hello";\ngree'], ['src/example.jsx', 'const view = <div />;\nview'], ['src/example.c', 'int greeting = 1;\ngree'], ['src/example.cpp', 'int greeting = 1;\ngree'], ['src/example.go', 'package main\nvar greeting = 1\n']]);
  const directories = new Set(['src']);
  let languageExited = false, languageMissing = false, scriptMissing = false, debugFail = false, rebuilds = 0, badEditNext = false;
  let bridgeOpen = false, bridgeDirty = false;
  const bridgeCalls = [];
  // Monaco cancels outstanding editor requests when a model is disposed.
  page.on('pageerror', error => { if (error.name === 'Canceled' && error.message === 'Canceled') return; errors.push(error.message); console.error(error.stack); });
  await page.route(`${origin}/api/session`, r => r.fulfill({ json: {} }));
  await page.route('**/api/v1/**', route => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '');
    requests.push(`${route.request().method()} ${path}`);
    if (path === '/plugin-projects/alpha/vscode-control') {
      const body = route.request().postDataJSON(); bridgeCalls.push(body);
      return route.fulfill({ json: body.action === 'state' ? { ready: true, dirty: bridgeDirty } : { opened: bridgeOpen } });
    }
    if (path === '/events') return;
    if (path === '/plugin-projects/alpha/container' && route.request().method() === 'POST' && new URL(route.request().url()).searchParams.get('rebuild') === 'true') { rebuilds++; return route.fulfill({ json: { connected: true } }); }
    if (path === '/plugin-projects/alpha/snapshot') return route.fulfill({ json: { entries: [...directories].map(item => ({ path: item, kind: 'directory', stamp: item })).concat([...files].map(([item, contents]) => ({ path: item, kind: 'file', stamp: contents }))) } });
    if (/^\/plugin-projects\/alpha\/language\/(typescript|javascript|c|cpp|go)$/.test(path) && route.request().method() === 'POST') { if (scriptMissing) return route.fulfill({ status: 503, json: { error: '容器内未安装 typescript 语言服务器' } }); scriptMessages.length = 0; return route.fulfill({ json: { client: 'script-client' } }); }
    if (/^\/plugin-projects\/alpha\/language\/(typescript|javascript|c|cpp|go)\/script-client$/.test(path)) {
      if (route.request().method() === 'GET') { const offset = Number(new URL(route.request().url()).searchParams.get('offset')); return route.fulfill({ json: { messages: scriptMessages.slice(offset), offset: scriptMessages.length, truncated: false, exited: false } }); }
      if (route.request().method() === 'DELETE') return route.fulfill({ json: { ok: true } });
      const message = route.request().postDataJSON(); languageRequests.push(message);
      if (message.method === 'initialize') scriptMessages.push({ jsonrpc: '2.0', id: message.id, result: { capabilities: { completionProvider: {} } } });
      if (message.method === 'textDocument/didOpen') scriptMessages.push({ jsonrpc: '2.0', method: 'textDocument/publishDiagnostics', params: { uri: message.params.textDocument.uri, diagnostics: [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 5 } }, severity: 1, message: 'container typescript diagnostic' }] } });
      if (message.method === 'textDocument/completion') scriptMessages.push({ jsonrpc: '2.0', id: message.id, result: [{ label: 'greeting_from_container', kind: 3 }] });
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/plugin-projects/alpha/debug/python' && route.request().method() === 'POST') { debugMessages.length = 0; return route.fulfill({ json: { client: 'debug-client' } }); }
    if (path === '/plugin-projects/alpha/debug/python/debug-client') {
      if (route.request().method() === 'GET') { const offset = Number(new URL(route.request().url()).searchParams.get('offset')); return route.fulfill({ json: { messages: debugMessages.slice(offset), offset: debugMessages.length, truncated: false, exited: false } }); }
      if (route.request().method() === 'DELETE') return route.fulfill({ json: { ok: true } });
      const message = route.request().postDataJSON(); debugRequests.push(message);
      if (message.command === 'launch') debugMessages.push(debugFail ? { type: 'response', request_seq: message.seq, success: false, message: '启动失败' } : { type: 'event', event: 'initialized' });
      else {
        const body = message.command === 'stackTrace' ? { stackFrames: [{ id: 11, name: 'main', line: 1, column: 1, source: { path: '/work/src/main.py', name: 'main.py' } }] } : message.command === 'scopes' ? { scopes: [{ name: 'Locals', variablesReference: 21 }] } : message.command === 'variables' ? { variables: message.arguments.variablesReference === 21 ? [{ name: 'sample', value: '{...}', variablesReference: 22 }] : [{ name: 'count', value: '42', type: 'int', variablesReference: 0 }] } : {};
        debugMessages.push({ type: 'response', request_seq: message.seq, success: true, body });
        if (message.command === 'configurationDone') { const launch = debugRequests.find(item => item.command === 'launch'); debugMessages.push({ type: 'response', request_seq: launch.seq, success: true }, { type: 'event', event: 'stopped', body: { threadId: 1, reason: 'breakpoint' } }); }
        if (message.command === 'continue') debugMessages.push({ type: 'event', event: 'continued' });
      }
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/plugin-projects/alpha/language/python' && route.request().method() === 'POST') { if (languageMissing) return route.fulfill({ status: 503, json: { error: '容器内未安装 python 语言服务器' } }); languageMessages.length = 0; return route.fulfill({ json: { client: 'test-client' } }); }
    if (path === '/plugin-projects/alpha/language/python/test-client') {
      if (route.request().method() === 'GET') {
        const offset = Number(new URL(route.request().url()).searchParams.get('offset'));
        return route.fulfill({ json: { messages: languageMessages.slice(offset), offset: languageMessages.length, truncated: false, exited: languageExited } });
      }
      if (route.request().method() === 'DELETE') return route.fulfill({ json: { ok: true } });
      const message = route.request().postDataJSON();
      languageRequests.push(message);
      if (message.method === 'initialize') languageMessages.push({ jsonrpc: '2.0', id: message.id, result: { capabilities: { completionProvider: {}, hoverProvider: true, definitionProvider: true, documentFormattingProvider: true, renameProvider: true, codeActionProvider: true } } }, { jsonrpc: '2.0', id: 888, method: 'client/registerCapability', params: { registrations: [{ id: 'python-watch', method: 'workspace/didChangeWatchedFiles', registerOptions: { watchers: [{ globPattern: '**/*.py', kind: 1 }, { globPattern: { baseUri: 'file:///work/src', pattern: '**/*.py' }, kind: 6 }] } }] } });
      if (message.method === 'textDocument/didOpen') languageMessages.push({ jsonrpc: '2.0', method: 'textDocument/publishDiagnostics', params: { uri: message.params.textDocument.uri, diagnostics: [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 5 } }, severity: 1, message: 'test diagnostic' }] } });
      if (message.method === 'textDocument/completion') languageMessages.push({ jsonrpc: '2.0', id: message.id, result: [{ label: 'container_completion', kind: 3 }] });
      if (message.method === 'textDocument/definition') languageMessages.push({ jsonrpc: '2.0', id: message.id, result: { uri: 'file:///work/src/other.py', range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } } } });
      if (message.method === 'textDocument/formatting') languageMessages.push({ jsonrpc: '2.0', id: message.id, result: [{ range: { start: { line: 0, character: 0 }, end: { line: 1, character: 8 } }, newText: 'formatted = 1\n' }] });
      if (message.method === 'textDocument/rename') languageMessages.push({ jsonrpc: '2.0', id: message.id, result: { changes: { 'file:///work/src/main.py': [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 9 } }, newText: message.params.newName }], 'file:///work/src/other.py': [{ range: { start: { line: 0, character: 4 }, end: { line: 0, character: 10 } }, newText: message.params.newName }] } } });
      if (message.method === 'textDocument/codeAction') {
        languageMessages.push({ jsonrpc: '2.0', id: message.id, result: [{ title: '修复符号', kind: 'quickfix', edit: { changes: { 'file:///work/src/main.py': [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 7 } }, newText: 'fixed' }] } } }] });
        if (badEditNext) { badEditNext = false; languageMessages.push({ jsonrpc: '2.0', id: 777, method: 'workspace/applyEdit', params: { edit: { changes: { 'file:///etc/passwd': [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }, newText: 'unsafe' }] } } } }); }
      }
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/plugin-projects/beta/container') return route.fulfill({ status: 503, json: { error: 'container unavailable' } });
    if (path === '/plugin-projects/alpha/terminal/input') { terminalInputs.push(route.request().postDataJSON().data); return route.fulfill({ json: { ok: true } }); }
    if (path === '/plugin-projects/alpha/terminal' && route.request().method() === 'GET') {
      const offset = Number(new URL(route.request().url()).searchParams.get('offset'));
      return route.fulfill({ json: { data: offset ? [] : [...new TextEncoder().encode('container shell ready')], offset: 21, truncated: false, exited: false } });
    }
    if (path === '/plugin-projects/alpha/file/rename') { const body = route.request().postDataJSON(); files.set(body.new_path, files.get(body.path)); files.delete(body.path); return route.fulfill({ json: {} }); }
    if (path === '/plugin-projects/alpha/directory/rename') { const body = route.request().postDataJSON(); for (const old of [...directories]) if (old === body.path || old.startsWith(`${body.path}/`)) { directories.delete(old); directories.add(body.new_path + old.slice(body.path.length)); } for (const [old, text] of [...files]) if (old.startsWith(`${body.path}/`)) { files.delete(old); files.set(body.new_path + old.slice(body.path.length), text); } return route.fulfill({ json: {} }); }
    if (path === '/plugin-projects/alpha/directory' && route.request().method() === 'DELETE') { const folder = new URL(route.request().url()).searchParams.get('path'); for (const old of [...directories]) if (old === folder || old.startsWith(`${folder}/`)) directories.delete(old); for (const old of [...files.keys()]) if (old.startsWith(`${folder}/`)) files.delete(old); return route.fulfill({ json: {} }); }
    if (path === '/plugin-projects/alpha/directory' && route.request().method() === 'POST') { directories.add(route.request().postDataJSON().path); return route.fulfill({ json: {} }); }
    if (path === '/plugin-projects/alpha/file' && route.request().method() === 'POST') { files.set(route.request().postDataJSON().path, ''); return route.fulfill({ json: {} }); }
    if (path === '/plugin-projects/alpha/file' && route.request().method() === 'DELETE') { files.delete(new URL(route.request().url()).searchParams.get('path')); return route.fulfill({ json: {} }); }
    if (path === '/plugin-projects/alpha/file' && route.request().method() === 'PUT') { const body = route.request().postDataJSON(); saves.push(body); files.set(body.path, body.contents); return route.fulfill({ json: {} }); }
    if (path === '/plugin-projects/alpha/file') return route.fulfill({ json: { contents: files.get(new URL(route.request().url()).searchParams.get('path')) } });
    const responses = {
      '/state': { last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: { '/root/alpha': { id: 's1', title: 'Alpha', created_at: 2, last_active: 3 } }, display_settings: {} },
      '/plugin-projects': { projects: [{ name: 'alpha', status: 'draft', error: null }, { name: 'beta', status: 'draft', error: null }] },
      '/plugin-projects/alpha/container': { connected: true },
      '/plugin-projects/alpha/vscode-session': { url: 'http://127.0.0.2:34999/test/' },
      '/plugins/environments': { environments: [] },
      '/plugin-projects/alpha/files': { files: [...files.keys()], directories: [...directories] },
    };
    return route.fulfill({ json: responses[path] ?? {} });
  });
  await page.goto(origin);
  await page.getByRole('button', { name: '插件项目', exact: true }).click();
  const side = page.getByRole('complementary', { name: '插件项目导航' });
  await side.getByRole('button', { name: /alpha/ }).click();
  await page.getByTitle('alpha VS Code 工作台').waitFor();
  await page.frameLocator('iframe[title="alpha VS Code 工作台"]').getByText('isolated workbench').waitFor();
  await side.getByRole('button', { name: 'src' }).click();
  await side.getByRole('button', { name: 'main.py' }).click();
  assert.equal(await page.getByTitle('alpha VS Code 工作台').isVisible(), false);
  const editor = page.locator('.monaco-editor textarea.inputarea');
  await editor.waitFor(); await editor.focus();
  await page.locator('.monaco-editor .view-line [class*="mtk"]').first().waitFor();
  await page.getByRole('status').filter({ hasText: 'python 语言服务已连接' }).waitFor();
  await page.locator('.monaco-editor .squiggly-error').first().waitFor();
  await page.keyboard.press('Control+Space');
  await page.getByText('container_completion', { exact: true }).first().waitFor();
  await page.keyboard.press('Escape');
  await page.keyboard.press('F12');
  await page.getByText('src/other.py', { exact: true }).waitFor();
  await side.getByRole('button', { name: 'main.py' }).click();
  await page.getByRole('status').filter({ hasText: 'python 语言服务已连接' }).waitFor();
  await editor.focus();
  await page.keyboard.press('Control+End'); await page.keyboard.type('print(2)');
  await page.getByRole('tablist', { name: '打开的项目文件' }).getByRole('tab', { name: 'other.py' }).click();
  await page.locator('[aria-label="编辑 src/other.py"]').waitFor();
  page.once('dialog', dialog => { assert.match(dialog.message(), /未保存/); return dialog.dismiss(); });
  await page.getByRole('button', { name: '关闭 src/main.py' }).click();
  assert.equal(await page.getByRole('button', { name: '关闭 src/main.py' }).count(), 1);
  await page.getByRole('tablist', { name: '打开的项目文件' }).getByRole('tab', { name: /main.py/ }).click();
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.includes('print(2)'));
  const rebuilt = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).searchParams.get('rebuild') === 'true');
  page.once('dialog', dialog => { assert.match(dialog.message(), /未保存/); return dialog.accept(); });
  await side.getByRole('button', { name: '重建开发容器' }).click();
  await rebuilt;
  await page.waitForFunction(() => document.querySelector('[aria-label="重建开发容器"]') && !document.querySelector('[aria-label="重建开发容器"]').disabled);
  assert.equal(rebuilds, 1, JSON.stringify(requests));
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.includes('print(2)'));
  await editor.focus();
  await page.keyboard.press('Control+S');
  await page.waitForFunction(() => document.querySelector('[aria-label="编辑 src/main.py"]'));
  await page.waitForTimeout(500);
  assert.equal(saves.length, 1);
  assert.equal(saves[0].path, 'src/main.py');
  assert.equal(saves[0].original, 'print(1)\n');
  assert.ok(saves[0].contents.includes('print(2)'));
  await editor.focus(); await page.keyboard.press('Control+Shift+i');
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.includes('formatted'));
  const formattedSave = page.waitForResponse(response => response.request().method() === 'PUT' && new URL(response.url()).pathname.endsWith('/plugin-projects/alpha/file'));
  await page.keyboard.press('Control+s');
  await formattedSave;
  assert.equal(saves.length, 2);
  assert.equal(saves[1].contents, 'formatted = 1\n');
  const gutter = await page.locator('.monaco-editor .glyph-margin').first().boundingBox();
  await page.mouse.click(gutter.x + 5, gutter.y + 8);
  await page.locator('.project-breakpoint').waitFor();
  await page.getByRole('button', { name: '开始调试' }).click();
  await page.getByRole('status').filter({ hasText: '已暂停：breakpoint' }).waitFor();
  await page.getByRole('region', { name: '调用栈' }).getByText(/main.py:1/).waitFor();
  await page.getByRole('button', { name: '展开 sample' }).click();
  await page.getByText('count').waitFor();
  await page.getByRole('button', { name: '定位 main 源码' }).click();
  assert.equal(debugRequests.find(item => item.command === 'scopes').arguments.frameId, 11);
  assert.ok(debugRequests.some(item => item.command === 'variables' && item.arguments.variablesReference === 22));
  assert.equal(debugRequests.find(item => item.command === 'launch').arguments.program, '/work/src/main.py');
  assert.deepEqual(debugRequests.find(item => item.command === 'setBreakpoints').arguments.breakpoints, [{ line: 1 }]);
  await page.getByRole('button', { name: '继续', exact: true }).click();
  await page.getByRole('status').filter({ hasText: '运行中' }).waitFor();
  assert.equal(debugRequests.find(item => item.command === 'continue').arguments.threadId, 1);
  await page.getByRole('button', { name: '停止', exact: true }).click();
  debugFail = true;
  await page.getByRole('button', { name: '开始调试' }).click();
  await page.getByRole('status').filter({ hasText: '启动失败' }).waitFor({ timeout: 5000 });
  debugFail = false;
  languageExited = true;
  await page.getByRole('status').filter({ hasText: '语言服务器已退出' }).waitFor();
  languageExited = false; languageMissing = true;
  await page.getByRole('button', { name: '重试', exact: true }).click();
  await page.getByRole('status').filter({ hasText: '容器内未安装 python 语言服务器' }).waitFor();
  languageMissing = false;
  await page.getByRole('button', { name: '重试', exact: true }).click();
  await page.getByRole('status').filter({ hasText: 'python 语言服务已连接' }).waitFor();
  assert.ok(requests.indexOf('POST /plugin-projects/alpha/container') < requests.indexOf('GET /plugin-projects/alpha/files'));
  await side.getByRole('tab', { name: '项目' }).click();
  await side.getByRole('button', { name: /beta/ }).click();
  await page.getByRole('alert').filter({ hasText: 'container unavailable' }).waitFor();
  assert.equal(requests.includes('GET /plugin-projects/beta/files'), false);
  await page.getByRole('button', { name: '终端', exact: true }).click();
  const terminal = page.locator('.xterm-helper-textarea');
  await terminal.waitFor(); await terminal.focus(); await page.keyboard.type('echo test');
  await page.waitForFunction(() => document.querySelector('.xterm-screen')?.textContent?.includes('container shell ready'));
  await page.waitForTimeout(300);
  assert.equal(terminalInputs.join(''), 'echo test');
  await side.getByRole('tab', { name: '文件' }).click();
  page.once('dialog', dialog => dialog.accept('empty-folder'));
  await side.getByRole('button', { name: '新建文件夹' }).click();
  await side.getByRole('button', { name: 'empty-folder' }).waitFor();
  assert.equal(await side.getByRole('button', { name: 'src', exact: true }).getAttribute('aria-expanded'), 'true');
  page.once('dialog', dialog => dialog.accept('new.txt'));
  await side.getByRole('button', { name: '新建文件', exact: true }).click();
  await page.locator('[aria-label="编辑 new.txt"]').waitFor();
  await page.waitForFunction(() => !document.querySelector('[aria-label="重命名文件"]')?.disabled);
  await editor.focus(); await page.keyboard.type('unsaved content');
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.includes('unsaved'));
  page.once('dialog', dialog => dialog.accept('renamed.txt'));
  await side.getByRole('button', { name: '重命名文件' }).click();
  await page.locator('[aria-label="编辑 renamed.txt"]').waitFor();
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.includes('unsaved'));
  page.once('dialog', dialog => { assert.match(dialog.message(), /未保存/); return dialog.dismiss(); });
  await side.getByRole('button', { name: '删除文件' }).click();
  assert.equal(files.has('renamed.txt'), true);
  page.once('dialog', dialog => dialog.accept());
  await side.getByRole('button', { name: '删除文件' }).click();
  await page.getByRole('button', { name: '关闭 renamed.txt' }).waitFor({ state: 'detached' });
  assert.equal(files.has('renamed.txt'), false);
  page.once('dialog', dialog => dialog.accept('work'));
  await side.getByRole('button', { name: '新建文件夹' }).click();
  await side.getByRole('button', { name: 'work', exact: true }).waitFor();
  page.once('dialog', dialog => { assert.equal(dialog.defaultValue(), 'work/'); return dialog.accept('work/item.txt'); });
  await side.getByRole('button', { name: '新建文件', exact: true }).click();
  await page.locator('[aria-label="编辑 work/item.txt"]').waitFor();
  await page.waitForFunction(() => !document.querySelector('[aria-label="重命名文件"]')?.disabled);
  await editor.focus(); await page.keyboard.type('keep unsaved');
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.includes('unsaved'));
  await side.getByRole('button', { name: 'work', exact: true }).click();
  page.once('dialog', dialog => dialog.accept('moved'));
  await side.getByRole('button', { name: '重命名文件夹' }).click();
  await page.locator('[aria-label="编辑 moved/item.txt"]').waitFor();
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.includes('unsaved'));
  await side.getByRole('button', { name: 'moved', exact: true }).waitFor();
  assert.equal(await side.getByRole('button', { name: 'moved', exact: true }).getAttribute('aria-expanded'), 'true');
  page.once('dialog', dialog => { assert.match(dialog.message(), /未保存/); return dialog.dismiss(); });
  await side.getByRole('button', { name: '删除文件夹' }).click();
  assert.equal(files.has('moved/item.txt'), true);
  page.once('dialog', dialog => dialog.accept());
  await side.getByRole('button', { name: '删除文件夹' }).click();
  await page.getByRole('button', { name: '关闭 moved/item.txt' }).waitFor({ state: 'detached' });
  assert.equal(files.has('moved/item.txt'), false);
  await side.getByRole('button', { name: 'main.py' }).click();
  await page.getByRole('status').filter({ hasText: 'python 语言服务已连接' }).waitFor();
  await editor.focus(); await page.keyboard.press('Control+Home');
  page.once('dialog', dialog => dialog.accept('renamed'));
  await page.keyboard.press('F2');
  await page.getByRole('status').filter({ hasText: '重命名已应用' }).waitFor();
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.replace(/\u00a0/g, ' ').includes('renamed = 1'));
  assert.equal(files.get('src/main.py'), 'formatted = 1\n');
  await page.getByRole('tablist', { name: '打开的项目文件' }).getByRole('tab', { name: /other.py/ }).click();
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.replace(/\u00a0/g, ' ').includes('def renamed'));
  assert.equal(files.get('src/other.py'), 'def target():\n    pass\n');
  await page.getByRole('tablist', { name: '打开的项目文件' }).getByRole('tab', { name: /main.py/ }).click();
  await page.getByRole('status').filter({ hasText: 'python 语言服务已连接' }).waitFor();
  await editor.focus(); await page.keyboard.press('Control+Home');
  page.once('dialog', dialog => dialog.accept('another'));
  await page.keyboard.press('F2');
  await page.getByRole('status').filter({ hasText: '请先保存其他已修改文件' }).waitFor();
  assert.ok(await page.locator('.monaco-editor .view-lines').textContent().then(text => text.replace(/\u00a0/g, ' ').includes('renamed = 1')));
  badEditNext = true;
  await editor.focus(); await page.keyboard.press('Control+.');
  await page.getByText('修复符号', { exact: true }).first().waitFor();
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.replace(/\u00a0/g, ' ').includes('fixed = 1'));
  assert.ok(languageRequests.some(message => message.method === 'textDocument/codeAction'));
  for (let attempt = 0; !languageRequests.some(message => message.id === 777 && message.result?.applied === false) && attempt < 100; attempt++) await page.waitForTimeout(50);
  assert.ok(languageRequests.some(message => message.id === 777 && message.result?.applied === false));
  for (let attempt = 0; !languageRequests.some(message => message.method === 'textDocument/didChange' && message.params.contentChanges[0].text.startsWith('fixed')) && attempt < 100; attempt++) await page.waitForTimeout(50);
  const version = languageRequests.filter(message => message.method === 'textDocument/didChange').at(-1).params.textDocument.version;
  languageMessages.push({ jsonrpc: '2.0', id: 778, method: 'workspace/applyEdit', params: { edit: { documentChanges: [{ textDocument: { uri: 'file:///work/src/main.py', version }, edits: [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 5 } }, newText: 'done' }] }] } } });
  for (let attempt = 0; !languageRequests.some(message => message.id === 778) && attempt < 100; attempt++) await page.waitForTimeout(50);
  assert.ok(languageRequests.some(message => message.id === 778 && message.result?.applied === true));
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.replace(/\u00a0/g, ' ').includes('done = 1'));
  languageMessages.push({ jsonrpc: '2.0', id: 779, method: 'workspace/applyEdit', params: { edit: { documentChanges: [{ textDocument: { uri: 'file:///work/src/main.py', version }, edits: [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 4 } }, newText: 'stale' }] }] } } });
  for (let attempt = 0; !languageRequests.some(message => message.id === 779) && attempt < 100; attempt++) await page.waitForTimeout(50);
  assert.ok(languageRequests.some(message => message.id === 779 && message.result?.applied === false));
  assert.ok(await page.locator('.monaco-editor .view-lines').textContent().then(text => text.replace(/\u00a0/g, ' ').includes('done = 1')));
  await side.getByRole('button', { name: 'example.tsx' }).click();
  await page.getByRole('status').filter({ hasText: 'typescript 语言服务已连接' }).waitFor();
  await page.locator('.monaco-editor .squiggly-error').first().waitFor();
  await editor.focus(); await page.keyboard.press('Control+End'); await page.keyboard.press('Control+Space');
  await page.getByText('greeting_from_container', { exact: true }).first().waitFor();
  assert.ok(languageRequests.some(message => message.method === 'textDocument/didOpen' && message.params.textDocument.languageId === 'typescriptreact'));
  await page.keyboard.press('Escape');
  await side.getByRole('button', { name: 'example.jsx' }).click();
  await page.getByRole('status').filter({ hasText: 'javascript 语言服务已连接' }).waitFor();
  assert.ok(languageRequests.some(message => message.method === 'textDocument/didOpen' && message.params.textDocument.languageId === 'javascriptreact'));
  scriptMissing = true;
  await side.getByRole('button', { name: 'example.tsx' }).click();
  await page.getByRole('status').filter({ hasText: '容器内未安装 typescript 语言服务器' }).waitFor();
  scriptMissing = false;
  await page.getByRole('button', { name: '重试', exact: true }).click();
  await page.getByRole('status').filter({ hasText: 'typescript 语言服务已连接' }).waitFor();
  await side.getByRole('button', { name: 'example.cpp' }).click();
  await page.getByRole('status').filter({ hasText: 'cpp 语言服务已连接' }).waitFor();
  await editor.focus(); await page.keyboard.press('Control+End'); await page.keyboard.press('Control+Space');
  await page.getByText('greeting_from_container', { exact: true }).first().waitFor();
  await page.keyboard.press('Escape');
  await side.getByRole('button', { name: 'example.c', exact: true }).click();
  await page.getByRole('status').filter({ hasText: 'c 语言服务已连接' }).waitFor();
  await side.getByRole('button', { name: 'example.go' }).click();
  await page.getByRole('status').filter({ hasText: 'go 语言服务已连接' }).waitFor();
  for (const languageId of ['c', 'cpp', 'go']) assert.ok(languageRequests.some(message => message.method === 'textDocument/didOpen' && message.params.textDocument.languageId === languageId));
  await side.getByRole('button', { name: 'watch.py' }).click();
  await page.getByRole('status').filter({ hasText: 'python 语言服务已连接' }).waitFor();
  await page.waitForResponse(response => new URL(response.url()).pathname.endsWith('/plugin-projects/alpha/snapshot'));
  await page.waitForTimeout(100);
  files.set('src/watch.py', 'changed externally\n');
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.replace(/\u00a0/g, ' ').includes('changed externally'));
  for (let attempt = 0; !languageRequests.some(message => message.method === 'workspace/didChangeWatchedFiles' && message.params.changes.some(change => change.uri === 'file:///work/src/watch.py' && change.type === 2)) && attempt < 100; attempt++) await page.waitForTimeout(50);
  assert.ok(languageRequests.some(message => message.method === 'workspace/didChangeWatchedFiles' && message.params.changes.some(change => change.uri === 'file:///work/src/watch.py' && change.type === 2)), JSON.stringify(languageRequests.filter(message => message.id === 888 || message.method === 'workspace/didChangeWatchedFiles')));
  await editor.focus(); await page.keyboard.press('Control+End'); await page.keyboard.type('local edit');
  files.set('src/watch.py', 'new disk contents\n');
  await page.getByRole('status').filter({ hasText: '该文件已在容器中更改' }).waitFor();
  assert.ok(await page.locator('.monaco-editor .view-lines').textContent().then(text => text.replace(/\u00a0/g, ' ').includes('local edit')));
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('button', { name: '重新读取' }).click();
  await page.waitForFunction(() => document.querySelector('.monaco-editor .view-lines')?.textContent?.replace(/\u00a0/g, ' ').includes('new disk contents'));
  files.set('src/from-terminal.txt', 'created externally');
  await side.getByRole('button', { name: 'from-terminal.txt' }).waitFor();
  assert.equal(languageRequests.some(message => message.method === 'workspace/didChangeWatchedFiles' && message.params.changes.some(change => change.uri === 'file:///work/src/from-terminal.txt')), false);
  files.delete('src/watch.py');
  await page.getByRole('status').filter({ hasText: '该文件已在容器中删除' }).waitFor();
  assert.ok(await page.locator('.monaco-editor .view-lines').textContent().then(text => text.replace(/\u00a0/g, ' ').includes('new disk contents')));
  for (let attempt = 0; !languageRequests.some(message => message.method === 'workspace/didChangeWatchedFiles' && message.params.changes.some(change => change.uri === 'file:///work/src/watch.py' && change.type === 3)) && attempt < 100; attempt++) await page.waitForTimeout(50);
  assert.ok(languageRequests.some(message => message.method === 'workspace/didChangeWatchedFiles' && message.params.changes.some(change => change.uri === 'file:///work/src/watch.py' && change.type === 3)));
  languageMessages.push({ jsonrpc: '2.0', id: 889, method: 'client/unregisterCapability', params: { unregisterations: [{ id: 'python-watch', method: 'workspace/didChangeWatchedFiles' }] } });
  for (let attempt = 0; !languageRequests.some(message => message.id === 889) && attempt < 100; attempt++) await page.waitForTimeout(50);
  assert.ok(languageRequests.some(message => message.id === 889));
  files.set('src/after-unregister.py', 'created');
  await side.getByRole('button', { name: 'after-unregister.py' }).waitFor();
  assert.equal(languageRequests.some(message => message.method === 'workspace/didChangeWatchedFiles' && message.params.changes.some(change => change.uri === 'file:///work/src/after-unregister.py')), false);
  bridgeOpen = true;
  await side.getByRole('button', { name: 'after-unregister.py' }).click();
  await page.getByTitle('alpha VS Code 工作台').waitFor({ state: 'visible' });
  assert.ok(bridgeCalls.some(call => call.action === 'open' && call.path === 'src/after-unregister.py'));
  bridgeDirty = true;
  await page.waitForTimeout(2500);
  page.once('dialog', dialog => { assert.match(dialog.message(), /未保存/); return dialog.dismiss(); });
  await page.getByRole('button', { name: '关闭 插件项目' }).click();
  assert.equal(await page.getByTitle('alpha VS Code 工作台').count(), 1);
  assert.deepEqual(errors, []);
  console.log('project-code-editor: all assertions passed');
} finally { if (browser) await browser.close(); server.kill(); }
