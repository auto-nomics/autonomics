// Pure export contract; native browser tests additionally decode PNG and inspect SVG XML.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { crc32 } from 'node:zlib';
import { createSourceManifest, embedPngSources } from '../src/features/canvas/canvas-source-manifest.ts';
const data = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"><path fill="#22aa66"/></svg>');
const hash = createHash('sha256').update(data).digest('hex');
const provenance = { source: 'BioIcons', asset: { id: 429, name: 'Receptor <review>', author: 'Léa 中文', author_url: 'javascript:alert(1)', license: 'cc-by-4.0', license_url: 'https://creativecommons.org/licenses/by/4.0/', source_url: 'https://bioicons.com/?query=receptor' }, source_sha256: 'original-source-hash', sha256: hash, normalization: ['Text outlined with fixed font'], modifications: [{ kind: 'recolor', colors: { '#ff0000': '#22aa66' }, based_on_sha256: 'old-hash', result_sha256: hash, user_note: 'PRIVATE-MODIFICATION-NOTE' }], user_note: 'PRIVATE-NOTE', previewDataURL: 'DO-NOT-EXPORT' };
const scene = JSON.stringify({ elements: [{ id: 'a', type: 'image', fileId: 'shared', customData: provenance }, { id: 'b', type: 'image', fileId: 'shared', customData: { ...provenance, sha256: '0'.repeat(64) } }, { id: 'missing', type: 'image', fileId: 'absent' }, { id: 'deleted', type: 'image', isDeleted: true, fileId: 'deleted', customData: { asset: { author: 'DELETED-AUTHOR' } } }, { id: 'shape', type: 'rectangle', customData: { user_note: 'PRIVATE-SHAPE-NOTE' } }], files: { shared: { mimeType: 'image/svg+xml', dataURL: `data:image/svg+xml;base64,${data.toString('base64')}` }, orphan: { dataURL: 'ORPHAN-DATA' } } });
const manifest = await createSourceManifest({ scene, title: 'Example', project_id: 'p', figure_id: 'f', based_on_revision: 7, unsaved_changes: true });
assert.equal(manifest.schema, 'autonomics.figure.asset-sources/v1'); assert.equal(manifest.assets.length, 3);
assert.equal(manifest.unsaved_changes, true); assert.equal(manifest.based_on_revision, 7);
assert.equal(manifest.scene_json_sha256, createHash('sha256').update(scene).digest('hex'));
assert.equal(manifest.assets[0].asset_id, '429'); assert.deepEqual(manifest.assets[0].authors, ['Léa 中文']);
assert.deepEqual(manifest.assets[0].author_urls, []); assert.equal(manifest.assets[0].actual_sha256, hash);
assert.equal(manifest.assets[0].hash_matches_recorded, true); assert.equal(manifest.assets[1].hash_matches_recorded, false);
assert.equal(manifest.assets[2].file_status, 'missing'); assert.deepEqual(manifest.assets[2].missing_fields, ['authors', 'licenses', 'source_urls']);
assert.equal(manifest.credits.length, 2, 'same citation is deduplicated without dropping per-instance hash checks');
const encoded = JSON.stringify(manifest);
for (const secret of ['PRIVATE-NOTE', 'PRIVATE-MODIFICATION-NOTE', 'PRIVATE-SHAPE-NOTE', 'DELETED-AUTHOR', 'ORPHAN-DATA', 'DO-NOT-EXPORT', data.toString('base64')]) assert.ok(!encoded.includes(secret));
const nih = await createSourceManifest({ scene: JSON.stringify({ elements: [{ id: 'nih', type: 'image', customData: { source: 'NIH BioArt', asset: { title: 'Cell', creators: ['NIH Artist'], licenses: ['Public Domain'], source_url: 'https://bioart.niaid.nih.gov/bioart/82' } } }] }), title: '', unsaved_changes: false });
assert.deepEqual(nih.assets[0].licenses, ['Public Domain']); assert.deepEqual(nih.assets[0].license_urls, [], 'do not infer a CC0 licence from Public Domain');
assert.ok(nih.credits[0].includes('NIH Artist'));
const local = await createSourceManifest({ scene: JSON.stringify({ elements: [{ id: 'local', type: 'image', customData: { modifications: [{ kind: 'path_edit', edits: [{ part_id: 'snapshot-part-1', translate: [12, -8], points: [{ index: 2, position: [3, 4], private_note: 'PRIVATE-PATH-NOTE' }], stroke: '#224466', opacity: 0.4, private_note: 'PRIVATE-PATH-NOTE' }] }] } }] }), title: '', unsaved_changes: false });
assert.equal(local.assets[0].modifications[0].edits[0].stroke, '#224466');
assert.equal(local.assets[0].modifications[0].edits[0].opacity, 0.4);
assert.deepEqual(local.assets[0].modifications[0].edits[0].translate, [12, -8]);
assert.deepEqual(local.assets[0].modifications[0].edits[0].points, [{ index: 2, position: [3, 4] }]);
assert.ok(!JSON.stringify(local).includes('PRIVATE-PATH-NOTE'));
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==', 'base64');
function chunks(bytes) {
  const result = []; let offset = 8;
  while (offset + 12 <= bytes.length) { const length = bytes.readUInt32BE(offset), end = offset + length + 12; assert.ok(end <= bytes.length); const type = bytes.subarray(offset + 4, offset + 8).toString(); result.push({ type, body: bytes.subarray(offset + 8, end - 4), raw: bytes.subarray(offset, end), crc: bytes.readUInt32BE(end - 4) }); offset = end; }
  return result;
}
const out = Buffer.from(await (await embedPngSources(new Blob([png]), manifest)).arrayBuffer());
const original = chunks(png), result = chunks(out), metadata = result.filter(c => c.type === 'iTXt');
assert.equal(metadata.length, 1); assert.equal(crc32(Buffer.concat([Buffer.from('iTXt'), metadata[0].body])), metadata[0].crc);
assert.deepEqual(result.filter(c => c.type !== 'iTXt').map(c => c.raw), original.map(c => c.raw), 'all original image chunks remain byte-for-byte intact');
assert.deepEqual(JSON.parse(metadata[0].body.subarray('autonomics.asset-sources'.length + 5).toString('utf8')), manifest);
const replaced = Buffer.from(await (await embedPngSources(new Blob([out]), { ...manifest, title: 'Updated' })).arrayBuffer());
assert.equal(chunks(replaced).filter(c => c.type === 'iTXt').length, 1);
assert.equal(JSON.parse(chunks(replaced).find(c => c.type === 'iTXt').body.subarray('autonomics.asset-sources'.length + 5).toString()).title, 'Updated');
await assert.rejects(embedPngSources(new Blob(['not png']), manifest));
console.log('canvas-sources: snapshot attribution, unknown provenance, instance hashes, privacy, UTF-8 PNG metadata and unchanged image chunks passed');
