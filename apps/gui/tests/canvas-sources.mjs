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

const inset = await createSourceManifest({ scene: JSON.stringify({ elements: [{ id: 'detail', type: 'image', customData: { modifications: [{ kind: 'inset', source_element_id: 'source', source_revision: 3, region: { x: 0.1, y: 0.2, width: 0.3, height: 0.4, note: 'PRIVATE-INSET-NOTE' }, crop: { x: 10, y: 20, width: 30, height: 40, naturalWidth: 100, naturalHeight: 100 }, private_note: 'PRIVATE-INSET-NOTE' }] } }] }), title: '', unsaved_changes: false });
assert.equal(inset.assets[0].modifications[0].source_element_id, 'source');
assert.equal(inset.assets[0].modifications[0].source_revision, 3);
assert.deepEqual(inset.assets[0].modifications[0].region, { x: 0.1, y: 0.2, width: 0.3, height: 0.4 });
assert.equal(inset.assets[0].modifications[0].crop.naturalWidth, 100);
assert.ok(!JSON.stringify(inset).includes('PRIVATE-INSET-NOTE'));

const refresh = await createSourceManifest({ scene: JSON.stringify({ elements: [{ id: 'refreshed', type: 'image', customData: { modifications: [{ kind: 'inset_refresh', source_element_id: 'source', source_revision: 5, previous_source_revision: 3, previous_file_id: 'older', previous_sha256: 'a'.repeat(64), region: { x: 0.1, y: 0.2, width: 0.3, height: 0.4 }, crop: { x: 10, y: 20, width: 30, height: 40, naturalWidth: 100, naturalHeight: 100 }, private_note: 'PRIVATE-REFRESH-NOTE' }] } }] }), title: '', unsaved_changes: false });
assert.equal(refresh.assets[0].modifications[0].previous_source_revision, 3);
assert.equal(refresh.assets[0].modifications[0].source_revision, 5);
assert.equal(refresh.assets[0].modifications[0].previous_file_id, 'older');
assert.equal(refresh.assets[0].modifications[0].previous_sha256, 'a'.repeat(64));
assert.ok(!JSON.stringify(refresh).includes('PRIVATE-REFRESH-NOTE'));

const scientific = await createSourceManifest({scene:JSON.stringify({elements:[{id:'relation',type:'arrow',startBinding:{elementId:'receptor'},endBinding:{elementId:'signal'},startArrowhead:null,endArrowhead:'bar',customData:{direction:'forward',inhibitory:true,evidence:['User supplied α reference',{doi:'10.example/fixture',url:'https://example.org/paper',privateNote:'PRIVATE-RELATION-NOTE'},{url:'javascript:alert(1)',dataURL:'PRIVATE-RELATION-BYTES'}],privateNote:'PRIVATE-RELATION-NOTE'}},{id:'deleted-arrow',type:'arrow',isDeleted:true},{id:'inset-callout',type:'arrow',customData:{inset:{}}},{id:'annotation',type:'arrow',customData:{annotationKind:'review'}},{id:'missing-evidence',type:'arrow'}]}),title:'Relations',based_on_revision:4,unsaved_changes:false});
assert.equal(scientific.relation_sources.verification,'not_verified');assert.equal(scientific.relation_sources.total,2);
const recorded=scientific.relation_sources.relations[0];assert.equal(recorded.element_id,'relation');assert.equal(recorded.start_element_id,'receptor');assert.equal(recorded.end_arrowhead,'bar');assert.equal(recorded.declared_inhibitory,true);
assert.equal(recorded.references[0].text,'User supplied α reference');assert.deepEqual(recorded.references[1].fields,{doi:'10.example/fixture',url:'https://example.org/paper'});assert.equal(recorded.references[1].omitted_fields,true);assert.deepEqual(recorded.references[2].fields,{});
assert.equal(scientific.relation_sources.relations[1].evidence_status,'not_recorded');assert.ok(!JSON.stringify(scientific).includes('PRIVATE-RELATION'));
const bounded=await createSourceManifest({scene:JSON.stringify({elements:Array.from({length:1001},(_,i)=>({id:String(i),type:'arrow',customData:{evidence:i===0?Array.from({length:101},()=> 'x'.repeat(2001)):[]}}))}),title:'Bounded',unsaved_changes:false});
assert.equal(bounded.relation_sources.relations.length,1000);assert.equal(bounded.relation_sources.truncated,true);assert.equal(bounded.relation_sources.relations[0].references_truncated,true);assert.equal(bounded.relation_sources.relations[0].references[0].truncated,true);
console.log('canvas-sources: relation identities, actual/declarative endpoints, explicit references, unverified/omitted status, private metadata exclusion and bounded completeness passed');

const redacted=await createSourceManifest({scene:JSON.stringify({elements:[{id:'r',type:'arrow',customData:{evidence:[' DATA:image/png;base64,PRIVATE-CITATION-BYTES']}}]}),title:'Redaction',unsaved_changes:false});assert.equal(redacted.relation_sources.relations[0].references[0].kind,'unsupported');assert.ok(!JSON.stringify(redacted).includes('PRIVATE-CITATION-BYTES'));

{
const previousBytes=Buffer.from('previous source bytes'),previousHash=createHash('sha256').update(previousBytes).digest('hex');
const replacementRecord={previous_file_id:'old-source',previous_provenance:{source:'Old provider',asset:{id:'old',name:'Old illustration',author:'Previous artist',license:'cc-by-3.0',source_url:'https://example.org/old',author_url:'https://user:password@example.org/private',private_note:'PRIVATE-HISTORICAL-ASSET'},sha256:previousHash,source_sha256:'original',modifications:[{kind:'recolor',colors:{'#123456':'#654321'},private_note:'PRIVATE-HISTORICAL-MODIFICATION'}],user_note:'PRIVATE-HISTORICAL-NOTE',dataURL:'PRIVATE-HISTORICAL-BYTES',asset_replacements:[{private_note:'PRIVATE-RECURSION'}]},result_file_id:'new-source',crop_policy:'normalized_source_region',created_at:'2026-11-10T00:00:00Z',private_note:'PRIVATE-HISTORY-ROW'};
const historyScene={elements:[{id:'replaced',type:'image',fileId:'new-source',customData:{...provenance,asset_replacements:[replacementRecord]}}],files:{'new-source':{mimeType:'image/svg+xml',dataURL:`data:image/svg+xml;base64,${data.toString('base64')}`},'old-source':{mimeType:'image/png',dataURL:`data:image/png;base64,${previousBytes.toString('base64')}`}}};
const historical=await createSourceManifest({scene:JSON.stringify(historyScene),title:'History',unsaved_changes:false});
assert.equal(historical.assets.length,1,'archived sources are not extra current art');
assert.deepEqual(historical.assets[0].authors,['Léa 中文']);assert.ok(historical.credits.every(line=>!line.includes('Previous artist')),'historical author never attributed as current');
const exportedHistory=historical.assets[0].replacement_history;assert.equal(exportedHistory.status,'recorded');assert.equal(exportedHistory.verification,'not_verified');assert.equal(exportedHistory.total,1);assert.equal(exportedHistory.start_index,0);assert.equal(exportedHistory.truncated,false);
const prior=exportedHistory.entries[0];assert.equal(prior.metadata_status,'recorded');assert.equal(prior.previous_file_id,'old-source');assert.equal(prior.result_file_id,'new-source');assert.deepEqual(prior.previous_source.authors,['Previous artist']);assert.deepEqual(prior.previous_source.licenses,['cc-by-3.0']);assert.equal(prior.previous_source.actual_sha256,previousHash);assert.equal(prior.previous_source.hash_matches_recorded,true);assert.deepEqual(prior.previous_source.author_urls,[]);assert.equal(prior.crop_policy,'normalized_source_region');assert.equal(prior.result_file.file_status,'available');assert.equal(prior.result_file.actual_sha256,hash);assert.equal(prior.result_file.hash_matches_file_id,null,'generic file IDs do not assert hash identity');
for(const privateText of ['PRIVATE-HISTORICAL-ASSET','PRIVATE-HISTORICAL-MODIFICATION','PRIVATE-HISTORICAL-NOTE','PRIVATE-HISTORICAL-BYTES','PRIVATE-RECURSION','PRIVATE-HISTORY-ROW',previousBytes.toString('base64')])assert.ok(!JSON.stringify(historical).includes(privateText));
const missingHistory=structuredClone(historyScene);delete missingHistory.files['old-source'];const missingPrior=(await createSourceManifest({scene:JSON.stringify(missingHistory),title:'',unsaved_changes:false})).assets[0].replacement_history.entries[0].previous_source;assert.equal(missingPrior.file_status,'missing');assert.equal(missingPrior.hash_matches_recorded,null);
const tamperedHistory=structuredClone(historyScene);tamperedHistory.files['old-source'].dataURL='data:image/png;base64,Y2hhbmdlZA==';assert.equal((await createSourceManifest({scene:JSON.stringify(tamperedHistory),title:'',unsaved_changes:false})).assets[0].replacement_history.entries[0].previous_source.hash_matches_recorded,false);
const missingResult=structuredClone(historyScene);delete missingResult.files['new-source'];assert.equal((await createSourceManifest({scene:JSON.stringify(missingResult),title:'',unsaved_changes:false})).assets[0].replacement_history.entries[0].result_file.file_status,'missing');
const wrongResultId=structuredClone(historyScene);wrongResultId.elements[0].customData.asset_replacements[0].result_file_id='f'.repeat(64);wrongResultId.files['f'.repeat(64)]=historyScene.files['new-source'];assert.equal((await createSourceManifest({scene:JSON.stringify(wrongResultId),title:'',unsaved_changes:false})).assets[0].replacement_history.entries[0].result_file.hash_matches_file_id,false);
const bounded=structuredClone(historyScene);bounded.elements[0].customData.asset_replacements=Array.from({length:55},(_,index)=>({...replacementRecord,previous_file_id:index===0?'not-exported':replacementRecord.previous_file_id}));const limited=(await createSourceManifest({scene:JSON.stringify(bounded),title:'',unsaved_changes:false})).assets[0].replacement_history;assert.equal(limited.total,55);assert.equal(limited.entries.length,50);assert.equal(limited.start_index,5);assert.equal(limited.entries[0].index,5);assert.equal(limited.entries.at(-1).index,54);assert.equal(limited.truncated,true);
for(const invalid of [{},'unsupported',false]){const scene=structuredClone(historyScene);scene.elements[0].customData.asset_replacements=invalid;const history=(await createSourceManifest({scene:JSON.stringify(scene),title:'',unsaved_changes:false})).assets[0].replacement_history;assert.equal(history.status,'unsupported');assert.equal(history.total,null);}
const invalidRows=structuredClone(historyScene);invalidRows.elements[0].customData.asset_replacements=[null,{},'unsupported'];const unsupported=(await createSourceManifest({scene:JSON.stringify(invalidRows),title:'',unsaved_changes:false})).assets[0].replacement_history;assert.equal(unsupported.unsupported_entries_in_export,3);assert.ok(unsupported.entries.every(entry=>entry.metadata_status==='unsupported'));
const nullCollision=structuredClone(historyScene);nullCollision.elements.push({id:'missing-id',type:'image',customData:{}},{id:'valid-missing-name',type:'image',fileId:'missing',customData:{sha256:hash}});nullCollision.files.missing=historyScene.files['new-source'];const collisionManifest=await createSourceManifest({scene:JSON.stringify(nullCollision),title:'',unsaved_changes:false});assert.equal(collisionManifest.assets.find(asset=>asset.element_id==='missing-id').file_status,'missing');assert.equal(collisionManifest.assets.find(asset=>asset.element_id==='valid-missing-name').hash_matches_recorded,true,'null file identity must not poison an actual file named missing');
const historyPng=Buffer.from(await(await embedPngSources(new Blob([png]),historical)).arrayBuffer());assert.deepEqual(JSON.parse(chunks(historyPng).find(chunk=>chunk.type==='iTXt').body.subarray('autonomics.asset-sources'.length+5).toString()),historical);
console.log('canvas-sources: historical/current attribution separation, archived hash/missing/tamper status, bounded recent history, invalid records, private field exclusion and PNG transport passed');

}
