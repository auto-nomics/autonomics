import assert from 'node:assert/strict';
import {illustrationFitStatus} from '../src/features/canvas/canvas-selection-context.ts';
const fixture=()=>[{id:'a',type:'rectangle',customData:{visualAssetElementId:'i',visualAnchorFit:{fileId:'f',imageBounds:[10,20,100,200],normalizedBounds:[0.1,0.1,0.8,0.8],imageTransform:{angle:0,scale:[1,1],crop:null}}}},{id:'i',type:'image',fileId:'f',x:10,y:20,width:100,height:200,angle:0,scale:[1,1],crop:null,customData:{semanticAnchorId:'a'}}];
for(const id of ['a','i'])assert.equal(illustrationFitStatus(fixture(),id).status,'current_snapshot');
for(const [change,status] of [[d=>d[1].fileId='new','stale_file'],[d=>d[1].width=101,'stale_geometry'],[d=>d[1].scale=[-1,1],'stale_geometry'],[d=>d[1].angle=1,'stale_geometry'],[d=>d[1].crop={x:1},'stale_geometry'],[d=>delete d[0].customData.visualAnchorFit,'not_recorded'],[d=>d[0].customData.visualAnchorFit.imageBounds=[0,0,-1,1],'invalid_record'],[d=>d[0].customData.visualAnchorFit.normalizedBounds=[0,0,2,1],'invalid_record']]){const d=fixture();change(d);assert.equal(illustrationFitStatus(d,'i').status,status);}
for(const change of [d=>d.push(structuredClone(d[0])),d=>d.push(structuredClone(d[1])),d=>d[1].customData.semanticAnchorId='wrong',d=>d[1].isDeleted=true]){const d=fixture();change(d);assert.equal(illustrationFitStatus(d,'i'),null);}
assert.equal(illustrationFitStatus(fixture(),'missing'),null);
console.log('fit status: current snapshot, stale source/geometry, invalid/missing records and ambiguous ownership verified');
const {canvasSelectionContext}=await import('../src/features/canvas/canvas-selection-context.ts');
for(const focus of ['a','i']){
 const current=canvasSelectionContext(fixture(),{[focus]:true}).objects[0];assert.equal(current.illustration_fit.status,'current_snapshot');assert.equal(current.illustration_fit.scientific_validity_verified,false);
 const changed=fixture();changed[1].fileId='edited-svg';assert.equal(canvasSelectionContext(changed,{[focus]:true}).objects[0].illustration_fit.status,'stale_file');
}
if(process.env.VISUALIZATION_FIT_STATUS_SCENE){
const {readFile}=await import('node:fs/promises');
const saved=JSON.parse(await readFile(process.env.VISUALIZATION_FIT_STATUS_SCENE,'utf8')).document;
const fitted=saved.elements.filter(e=>e.customData?.visualAnchorFit);assert.equal(fitted.length,8);
for(const anchor of fitted){assert.equal(illustrationFitStatus(saved.elements,anchor.id).status,'current_snapshot');const changed=structuredClone(saved.elements),image=changed.find(e=>e.id===anchor.customData.visualAssetElementId);image.fileId='new-file-for-snapshot-test';assert.equal(illustrationFitStatus(changed,anchor.id).status,'stale_file');}
console.log('fit status: agent selection channel and actual saved 8-component snapshot verified; simulated file changes detected');

}
