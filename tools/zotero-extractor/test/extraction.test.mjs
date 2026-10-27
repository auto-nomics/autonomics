import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { extractPDF } from '../extract.mjs';

for (const name of ['synthetic-list-links', 'synthetic-cross-page-mixed-notes', 'synthetic-printed-contents-outline', 'paint-scene-pathologies', 'cross-page-width-tolerance']) {
  test(`matches Zotero's complete semantic golden: ${name}`, { timeout: 120000 }, async () => {
    const bytes = await readFile(new URL(`fixtures/${name}.pdf`, import.meta.url));
    const expected = JSON.parse(await readFile(new URL(`fixtures/${name}.json`, import.meta.url), 'utf8'));
    const { structure, text, extractor } = await extractPDF(bytes);
    // Upstream's golden helper excludes processor version and wall-clock time.
    const actual = structuredClone(structure);
    delete actual.metadata.processor.version;
    actual.metadata.dateCreated = '2000-01-01T00:00:00.000Z';
    assert.deepEqual(actual, expected);
    assert.equal(extractor, 'zotero-sdt');
    assert.ok(text.includes('\n\n'), 'paragraph separators must survive the fulltext projection');
    assert.ok(structure.content.some(block => block.type === 'paragraph'));
    if (name.startsWith('synthetic')) assert.ok(structure.content.some(block => block.type === 'heading'));
    assert.ok(structure.content.some(block => block.anchor?.pageRects?.length));
  });
}

test('invalid PDF fails without producing fabricated paragraphs', async () => {
  await assert.rejects(extractPDF(Buffer.from('this is not a PDF')));
});
