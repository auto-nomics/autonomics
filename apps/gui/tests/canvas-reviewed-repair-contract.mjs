import assert from 'node:assert/strict';
import {reviewedRepairTargets,checkReviewedRepair} from './helpers/canvas-live-reviewed-repair.mjs';
const document={files:{asset:{dataURL:'retained'}},elements:[
 {id:'frame',type:'frame',x:-20,y:-20,width:400,height:400},
 {id:'anchor',type:'rectangle',customData:{visualAssetElementId:'image',visualLabelIds:['caption']}},
 {id:'image',type:'image',customData:{semanticAnchorId:'anchor'}},
 {id:'caption',type:'text',x:0,y:40,width:80,height:20,frameId:'frame',fontSize:20,originalText:'Receptor label',text:'Receptor label'},
 {id:'line',type:'arrow',x:40,y:0,points:[[0,0],[0,100]],startBinding:{elementId:'anchor'}},
 {id:'erk',type:'text',x:200,y:140,width:100,height:40,frameId:'frame',containerId:'node',originalText:'ERK1/2 p-2T MAPK',text:'ERK1/2 p-2T MAPK',customData:{planningSource:{version:7}}},
 {id:'node',type:'rectangle',x:190,y:130,width:120,height:70},
]};
const targets=reviewedRepairTargets(document),next=structuredClone(document);
next.elements.find(e=>e.id==='erk').text=targets.expected_erk_text;next.elements.find(e=>e.id==='erk').originalText=targets.expected_erk_text;
next.elements.find(e=>e.id==='caption').x=100;
checkReviewedRepair(document,next,targets);
const outsideContainer=structuredClone(next);outsideContainer.elements.find(e=>e.id==='erk').x=20;assert.throws(()=>checkReviewedRepair(document,outsideContainer,targets),/original container/);
for(const mutate of [d=>d.elements.find(e=>e.id==='caption').x=0,d=>d.elements.find(e=>e.id==='caption').x=400,d=>d.elements.find(e=>e.id==='line').points[1][1]=110,d=>d.files.asset.dataURL='changed',d=>d.elements.find(e=>e.id==='erk').customData.planningSource.version=8,d=>d.elements.find(e=>e.id==='caption').fontSize=16,d=>d.elements.find(e=>e.id==='erk').text='still wrong']){
 const invalid=structuredClone(next);mutate(invalid);assert.throws(()=>checkReviewedRepair(document,invalid,targets));
}
console.log('reviewed repair: explicit text correction and label-only clearance accepted; crossing, outside-panel placement, unrelated geometry, files, history, font and visible-text changes rejected');
