import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { extractPDF } from '../extract.mjs';

// Host extensions injected after the golden capture (figure export) must be
// stripped before the deep compare, like processor version and wall-clock time.
function stripHostExtensions(node) {
  if (Array.isArray(node)) {
    for (const child of node) stripHostExtensions(child);
    return;
  }
  if (!node || typeof node !== 'object') {
    return;
  }
  delete node.imageRef;
  if (Array.isArray(node.content)) {
    stripHostExtensions(node.content);
  }
}

for (const name of ['synthetic-list-links', 'synthetic-cross-page-mixed-notes', 'synthetic-printed-contents-outline', 'paint-scene-pathologies', 'cross-page-width-tolerance']) {
  test(`matches Zotero's complete semantic golden: ${name}`, { timeout: 120000 }, async () => {
    const bytes = await readFile(new URL(`fixtures/${name}.pdf`, import.meta.url));
    const expected = JSON.parse(await readFile(new URL(`fixtures/${name}.json`, import.meta.url), 'utf8'));
    const { structure, text, extractor } = await extractPDF(bytes);
    // Upstream's golden helper excludes processor version and wall-clock time.
    const actual = structuredClone(structure);
    delete actual.metadata.processor.version;
    actual.metadata.dateCreated = '2000-01-01T00:00:00.000Z';
    stripHostExtensions(actual);
    assert.deepEqual(actual, expected);
    assert.equal(extractor, 'zotero-sdt');
    assert.ok(text.includes('\n\n'), 'paragraph separators must survive the fulltext projection');
    assert.ok(structure.content.some(block => block.type === 'paragraph'));
    if (name.startsWith('synthetic')) assert.ok(structure.content.some(block => block.type === 'heading'));
    assert.ok(structure.content.some(block => block.anchor?.pageRects?.length));
  });
}

test('exports PNG figures for image blocks and links them via imageRef', { timeout: 120000 }, async () => {
  const bytes = await readFile(new URL('fixtures/paint-scene-pathologies.pdf', import.meta.url));
  const { structure, images } = await extractPDF(bytes);
  const figureBlocks = structure.content.filter(block => block.type === 'image' && block.flowClass !== 'excluded');
  assert.ok(figureBlocks.length > 0, 'fixture must contain non-auxiliary image blocks');
  for (const block of figureBlocks) {
    assert.match(block.imageRef, /^z-[0-9a-f]{16}\.png$/, 'every figure block carries an imageRef');
  }
  assert.ok(images.length > 0, 'at least one image payload is returned');
  assert.ok(images.length <= 64);
  const names = new Set(images.map(image => image.name));
  for (const block of figureBlocks) {
    assert.ok(names.has(block.imageRef), 'imageRef resolves to a returned payload');
  }
  for (const image of images) {
    assert.match(image.name, /^z-[0-9a-f]{16}\.png$/);
    const png = Buffer.from(image.data, 'base64');
    assert.deepEqual(png.subarray(0, 8), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), 'PNG magic');
    assert.ok(png.byteLength > 0 && png.byteLength <= 15 * 1024 * 1024);
  }
  // Content-addressed names are stable across runs of the same PDF.
  const second = await extractPDF(bytes);
  assert.deepEqual(second.images.map(image => image.name).sort(), [...names].sort());
});

test('invalid PDF fails without producing fabricated paragraphs', async () => {
  await assert.rejects(extractPDF(Buffer.from('this is not a PDF')));
});
