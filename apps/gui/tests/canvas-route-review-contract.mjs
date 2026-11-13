import assert from 'node:assert/strict';
import {crossesBox,crossedNodes,checkRouteReview} from './helpers/canvas-live-route-review.mjs';
const node=(id,x)=>({id,type:'rectangle',x,y:100,width:100,height:100,angle:0,customData:{scientificId:id}});
const previous={files:{r:{dataURL:'retained bytes'}},scientificPlan:{message:'Historical'},elements:[node('source',0),node('blocker',200),node('target',400),{id:'edge',type:'arrow',x:110,y:150,width:280,height:0,points:[[0,0],[280,0]],startBinding:{elementId:'source'},endBinding:{elementId:'target'},customData:{evidence:['retained source']}},{id:'caption',type:'text',containerId:'edge',text:'flow',originalText:'flow',x:220,y:140,width:60,height:25,fontFamily:9,fontSize:20},{id:'frame',type:'frame',x:0,y:0,width:500,height:300}]};
assert.deepEqual(crossedNodes(previous,'edge'),['blocker']);
const next=structuredClone(previous);Object.assign(next.elements.find(e=>e.id==='edge'),{height:100,points:[[0,0],[70,0],[70,-100],[210,-100],[210,0],[280,0]]});next.elements.find(e=>e.id==='caption').y=35;
checkRouteReview(previous,next,'edge');
const outerLoop=structuredClone(next);outerLoop.elements.find(e=>e.id==='edge').points=[[0,0],[70,0],[70,-500],[210,-500],[210,0],[280,0]];assert.throws(()=>checkRouteReview(previous,outerLoop,'edge'));
for(const mutate of [d=>d.elements.find(e=>e.id==='edge').points=[[0,0],[280,0]],d=>d.elements.find(e=>e.id==='target').x++,d=>d.elements.find(e=>e.id==='caption').text='new',d=>d.elements.find(e=>e.id==='edge').customData.evidence=[],d=>d.files.r.dataURL='changed',d=>d.elements.find(e=>e.id==='edge').x=null]){
 const bad=structuredClone(next);mutate(bad);assert.throws(()=>checkRouteReview(previous,bad,'edge'));
}
const box={x:10,y:10,width:20,height:20};
assert.ok(crossesBox([0,20],[40,20],box));assert.ok(crossesBox([20,0],[20,40],box));assert.ok(!crossesBox([0,10],[40,10],box));assert.ok(!crossesBox([0,0],[10,10],box));assert.ok(!crossesBox([0,0],[0,0],box));
console.log('route review: actual segment interiors, non-endpoint node boxes, preserved labels/geometry scope/evidence/files and malformed geometry rejection verified');
