import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { extractPDF } from '../extract.mjs';
import { anchorQuote } from '../anchor.mjs';

test('verified PDF quote maps to exact glyph coordinates and rejects unresolved anchors', async () => {
  const { structure } = await extractPDF(await readFile(new URL('./fixtures/synthetic-list-links.pdf', import.meta.url)));
  const result = anchorQuote(structure, 'The invented sources');
  assert.equal(result.page, 1);
  assert.equal(result.rects[0].space, 'pdf');
  assert.deepEqual(result.rects[0].text_range.start, [1, 0, 0]);
  assert.deepEqual(result.rects[0].text_range.end, [1, 0, 20]);
  assert.ok(result.rects[0].width > 0 && result.rects[0].width < .2);
  assert.throws(() => anchorQuote(structure, 'Fabricated passage'), /cannot be mapped/);
  const duplicate = structuredClone(structure);
  duplicate.content.push(structuredClone(duplicate.content[1]));
  assert.throws(() => anchorQuote(duplicate, 'The invented sources'), /ambiguous/);
  const noGeometry = structuredClone(structure);
  function clear(node) { delete node.anchor; node.content?.forEach(clear); }
  noGeometry.content.forEach(clear);
  assert.throws(() => anchorQuote(noGeometry, 'The invented sources'), /no geometry/);
});
