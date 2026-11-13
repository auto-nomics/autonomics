import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {checkCheckpointIllustrationOwnership} from './helpers/canvas-checkpoint-ownership.mjs';
assert.ok(process.env.VISUALIZATION_OWNERSHIP_BEFORE&&process.env.VISUALIZATION_OWNERSHIP_AFTER&&process.env.VISUALIZATION_OWNERSHIP_FAILED,'explicit saved scenes required');
const load=async path=>JSON.parse(await readFile(path,'utf8')).document;
const before=await load(process.env.VISUALIZATION_OWNERSHIP_BEFORE),after=await load(process.env.VISUALIZATION_OWNERSHIP_AFTER),failed=await load(process.env.VISUALIZATION_OWNERSHIP_FAILED),active=['node-3','node-4'];
checkCheckpointIllustrationOwnership(before,after,active);assert.throws(()=>checkCheckpointIllustrationOwnership(before,failed,active));
for(const mutate of [d=>{d.elements.find(e=>e.customData?.semanticBadgeAnchorId==='node-3').customData.badgeMeaning='';},d=>{d.elements.find(e=>e.customData?.semanticBadgeAnchorId==='node-3').groupIds=[];},d=>{d.elements.find(e=>e.customData?.semanticBadgeAnchorId==='node-3').frameId=null;},d=>{d.elements.find(e=>e.customData?.semanticAnchorId==='node-3').customData.asset.name='inorganic-phosphate';}]){const d=structuredClone(after);mutate(d);assert.throws(()=>checkCheckpointIllustrationOwnership(before,d,active));}
console.log('checkpoint ownership: real bound composites accepted; failed unowned scene, missing meaning/group/frame and phosphate primary rejected');
