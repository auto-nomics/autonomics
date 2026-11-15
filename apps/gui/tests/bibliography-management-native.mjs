// Real REST/storage integration for the same migration runner used by the GUI.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { build } from 'esbuild';
const host = spawn('../../target/debug/examples/bibliography_management_native', [], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = ''; host.stdout.on('data', value => output += value); host.stderr.on('data', value => output += value);
try {
  for (let n = 0; !/BIB_MANAGEMENT_ADDR=([^\s]+)/.test(output); n++) {
    if (host.exitCode !== null || n > 400) throw new Error(output || 'Integration host did not start');
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const origin = 'http://' + output.match(/BIB_MANAGEMENT_ADDR=([^\s]+)/)[1];
  const request = async (method, path, body) => {
    const response = await fetch(origin + '/api/v1' + path, { method, headers: body ? { 'Content-Type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined });
    const result = await response.json();
    if (!response.ok) throw new Error(`${response.status}: ${result.error}`);
    return result;
  };
  const bundled = async name => {
    const result = await build({ entryPoints: [`src/features/bibliography/${name}.ts`], bundle: true, platform: 'node', format: 'esm', write: false });
    return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
  };
  const { parseZoteroBundle } = await bundled('zotero-bundle-model');
  const { migrateZoteroBundle } = await bundled('zotero-bundle-runner');
  const contents = JSON.stringify({ items: [{ key: 'BOOK', data: { itemType: 'book', title: 'Native migration book', publisher: 'Evidence Press', creators: [{ creatorType: 'author', lastName: 'Li', firstName: 'Mei' }], collections: ['CHILD'], notes: [{ key: 'NOTE', note: 'Original migration note' }], attachments: [{ key: 'FILE', title: 'Supporting text', path: 'files/BOOK/support.txt' }] } }], collections: { ROOT: { name: 'Imported root' }, CHILD: { name: 'Imported child', parentCollection: 'ROOT', items: ['BOOK'] } } });
  const plan = parseZoteroBundle(contents);
  const bytes = 'Native attachment evidence\n中文内容\n';
  const file = new File([bytes], 'support.txt', { type: 'text/plain' });
  Object.defineProperty(file, 'webkitRelativePath', { value: 'Export/files/BOOK/support.txt' });
  const upload = async (id, file, options) => {
    const form = new FormData(); form.append('file', file, file.name); form.append('preferred', String(options.preferred)); if (options.title) form.append('title', options.title);
    const response = await fetch(`${origin}/api/v1/bib/articles/${encodeURIComponent(id)}/attachments`, { method: 'POST', body: form });
    const result = await response.json(); if (!response.ok) throw new Error(`${response.status}: ${result.error}`); return result;
  };
  const migrate = () => migrateZoteroBundle({ contents, plan, files: [file], sourceLabel: 'native-fixture-library', request, upload, noteText: value => value, progress: () => {} });
  const first = await migrate();
  assert.deepEqual(first.errors, []); assert.equal(first.articles, 1); assert.equal(first.collections, 2); assert.equal(first.notes, 1); assert.equal(first.attachments, 1);
  const second = await migrate();
  assert.deepEqual(second.errors, []); assert.equal(second.articles, 0); assert.equal(second.collections, 0); assert.equal(second.notes, 0); assert.equal(second.attachments, 0);
  const library = await request('GET', '/bib/articles'); const article = library.articles[0];
  assert.equal(article.extra_fields.publisher, 'Evidence Press'); assert.deepEqual(article.pub_types, ['Book']);
  const detail = await request('GET', `/bib/articles/${article.id}`);
  assert.equal(detail.annotations.length, 1); assert.equal(detail.attachments.length, 1);
  assert.ok(detail.annotations[0].tags.some(tag => tag.startsWith('zotero-note:')));
  const raw = `/api/v1/bib/articles/${article.id}/attachments/${detail.attachments[0].key}/raw`;
  assert.equal(await (await fetch(origin + raw)).text(), bytes);
  await request('DELETE', `/bib/articles/${article.id}`);
  assert.equal((await fetch(origin + raw)).status, 404);
  assert.equal((await request('GET', '/bib/articles')).total, 0);
  await request('POST', `/bib/articles/${article.id}/restore`);
  assert.equal(await (await fetch(origin + raw)).text(), bytes);
  const restored = await request('GET', `/bib/articles/${article.id}`);
  await request('PUT', `/bib/articles/${article.id}`, { ...restored.article, title: 'Revised native book', expected_revision: restored.revision });
  await assert.rejects(request('PUT', `/bib/articles/${article.id}`, { ...restored.article, title: 'Stale update', expected_revision: restored.revision }), /409/);
  console.log('PASS real API: Zotero metadata/collection/note/attachment migration, idempotent retry, type fields, raw download, trash/restore, stale revision protection');
} finally { host.kill(); if (host.exitCode === null) await once(host, 'exit'); }
