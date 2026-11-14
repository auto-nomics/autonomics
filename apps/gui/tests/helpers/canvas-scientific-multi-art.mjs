// Source-backed second-component materialization; no scene is imported here.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {checkLocalComponentLabelMove} from './canvas-materialized-components.mjs';
import {checkCandidateGeometryReviews} from './canvas-scene-geometry-evidence.mjs';
import {checkPortableSceneExport} from './canvas-live-source-review.mjs';
import {illustrationConnectorLabelOverlaps} from '../../src/features/canvas/canvas-connector-label-overlaps.ts';
const byId=(doc,id)=>doc.elements.find(e=>e.id===id);
export function statDimerTarget(plan){
 const dimers=plan.nodes.filter(n=>/STAT1(?!\d)/i.test(n.label)&&/GAF|二聚|dimer/i.test(n.label));
 const cytoplasmic=dimers.filter(n=>/胞质|细胞质|cyto/i.test(n.label)&&!/核内|核质|nuclear|nucleus/i.test(n.label));
 const candidates=cytoplasmic.length?cytoplasmic:dimers.filter(n=>!/核|nuclear|nucleus/i.test(n.label));
 assert.equal(candidates.length,1,'one explicit extranuclear STAT1 dimer is required; never guess another protein or nuclear state');
 return candidates[0];
}
export function checkSecondMaterialization(before,after,target,noteId){
 assert.equal(new Set(after.elements.map(e=>e.id)).size,after.elements.length,'native IDs remain unique');
 const oldImages=before.elements.filter(e=>e.type==='image'&&!e.isDeleted),images=after.elements.filter(e=>e.type==='image'&&!e.isDeleted);
 assert.equal(oldImages.length,1);assert.equal(images.length,2);
 const image=images.find(e=>e.customData?.semanticAnchorId===target.id);assert.ok(image);
 assert.notEqual(image.fileId,oldImages[0].fileId);assert.notEqual(image.customData.asset.id,oldImages[0].customData.asset.id);
 const asset=image.customData.asset;assert.match(asset.title??asset.name,/STAT/i);assert.match(asset.title??asset.name,/dimer|二聚/i);
 assert.ok(asset.creators?.length&&asset.licenses?.length);assert.equal(asset.source_url,`https://bioart.niaid.nih.gov/bioart/${asset.id}`);
 const file=after.files[image.fileId];assert.ok(file?.dataURL?.startsWith('data:image/png;base64,'));
 assert.equal(createHash('sha256').update(Buffer.from(file.dataURL.split(',')[1],'base64')).digest('hex'),image.customData.sha256);
 const anchor=byId(after,target.id),oldLabel=before.elements.find(e=>e.type==='text'&&e.containerId===target.id);assert.ok(oldLabel);
 const label=byId(after,oldLabel.id);assert.equal(anchor.customData.visualAssetElementId,image.id);assert.ok(anchor.customData.visualLabelIds.includes(label.id));assert.equal(label.containerId,null);
 for(const key of Object.keys(target.customData??{}))assert.deepEqual(anchor.customData[key],target.customData[key]);
 for(const key of ['originalText','fontSize','fontFamily','lineHeight'])assert.deepEqual(label[key],oldLabel[key]);
 assert.equal(label.text.replace(/\s/g,''),oldLabel.text.replace(/\s/g,''));
 const arrows=before.elements.filter(e=>e.type==='arrow'&&(e.startBinding?.elementId===target.id||e.endBinding?.elementId===target.id));
 const captions=before.elements.filter(e=>e.type==='text'&&arrows.some(a=>e.containerId===a.id));
 for(const old of arrows){const next=byId(after,old.id);assert.deepEqual(next.customData,old.customData);for(const key of ['startArrowhead','endArrowhead'])assert.deepEqual(next[key],old[key]);for(const key of ['startBinding','endBinding'])assert.equal(next[key].elementId,old[key].elementId);}
 for(const old of captions){const next=byId(after,old.id);for(const key of ['text','originalText','fontSize','fontFamily','lineHeight'])assert.deepEqual(next[key],old[key]);}
 const allowed=new Set([target.id,oldLabel.id,...arrows.map(e=>e.id),...captions.map(e=>e.id)]);
 for(const old of before.elements)if(!allowed.has(old.id))assert.deepEqual(byId(after,old.id),old,`second materialization preserves ${old.id}`);
 for(const [id,file]of Object.entries(before.files))assert.deepEqual(after.files[id],file);
 assert.deepEqual(Object.fromEntries(Object.entries(after).filter(([k])=>!['elements','files'].includes(k))),Object.fromEntries(Object.entries(before).filter(([k])=>!['elements','files'].includes(k))));
 const additions=after.elements.filter(e=>!byId(before,e.id));assert.equal(additions.length,2);assert.ok(additions.some(e=>e.id===image.id));
 const note=byId(after,noteId);assert.ok(note?.type==='text'&&!note.containerId&&!note.frameId&&note.fontSize===16);assert.equal(note.originalText,'STAT 素材为家族级示意，不验证 STAT1 特异结构或磷酸化位点。');assert.ok(note.y>=Math.max(...before.elements.filter(e=>!e.isDeleted).map(e=>e.y+e.height))+8);
 return {image,label};
}
