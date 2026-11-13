import assert from 'node:assert/strict';import {readFileSync,existsSync} from 'node:fs';
import {checkIllustratedScene} from './helpers/canvas-illustrated-scene-contract.mjs';
const path=process.env.CANVAS_ILLUSTRATION_REPLAY;assert.ok(path,'set an actual saved illustration evaluation directory');
const read=name=>JSON.parse(readFileSync(`${path}/${name}`,'utf8'));
const parent=read('ifng-art-parent.json'),child=read(existsSync(`${path}/ifng-art-first-child.json`)?'ifng-art-first-child.json':'ifng-art-child.json'),handoff=read('workflow-stage-handoff.json');
const target=parent.document.elements.find(e=>e.id===handoff.target_id);const image=child.document.elements.find(e=>e.type==='image'&&!e.isDeleted);
const args={child,parent,target,notes:[...handoff.note_ids,'generic-receptor-note'],assetProvider:image.customData.source==='NIH BioArt'?'bioart':Array.isArray(image.customData.asset.creators)?'bioart':'bioicons',assetId:image.customData.asset.id};
checkIllustratedScene(args);
for(const mutate of [v=>v.document.elements.find(e=>e.id===handoff.note_ids[0]).text='Deleted scope',v=>v.document.elements.find(e=>e.id===target.id).customData.evidence=['invented'],v=>v.document.elements.find(e=>e.id===image.id).customData.asset.id='other',v=>v.document.elements.find(e=>e.type==='arrow'&&(e.startBinding?.elementId===target.id||e.endBinding?.elementId===target.id)).endArrowhead=null,v=>v.document.elements.find(e=>e.type==='arrow'&&(e.startBinding?.elementId===target.id||e.endBinding?.elementId===target.id)).startArrowhead='arrow']){const invalid=structuredClone(child);mutate(invalid);assert.throws(()=>checkIllustratedScene({...args,child:invalid}));}
console.log('actual illustrated scene: scientific labels/evidence, native arrowheads, notes, asset identity and unrelated objects checked; negative edits rejected');
