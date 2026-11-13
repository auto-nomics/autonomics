import assert from 'node:assert/strict';
import {checkCompactComposition} from './helpers/canvas-compact-composition.mjs';
const plan={panels:[{id:'a'},{id:'b'}],nodes:[{id:'n1',panel:'a'},{id:'n2',panel:'b'}]};
const doc={elements:[{id:'f1',type:'frame',x:0,y:0,width:300,height:400},{id:'f2',type:'frame',x:400,y:0,width:300,height:400},{id:'n1',type:'rectangle',x:20,y:40,width:200,height:80},{id:'n2',type:'rectangle',x:420,y:40,width:200,height:80},{id:'label',type:'text',x:20,y:140,width:200,height:50,fontSize:20},{id:'edge',type:'arrow',x:350,y:200,width:100,height:0,points:[[-50,0],[50,0]]}]};
assert.equal(checkCompactComposition(plan,doc).aspect_ratio,1.75);
for(const mutate of [d=>d.elements[5].points=[[-1000,0],[0,0]],d=>d.elements[1].x=2000,d=>d.elements[1].height=2000,d=>d.elements[4].fontSize=10,d=>d.elements[0].angle=.4,d=>d.elements[0].width=NaN,d=>d.elements.pop()&&d.elements.pop()&&d.elements.splice(1,1)]){const invalid=structuredClone(doc);mutate(invalid);assert.throws(()=>checkCompactComposition(plan,invalid));}
assert.throws(()=>checkCompactComposition({panels:[{id:'a'}],nodes:[]},doc));assert.throws(()=>checkCompactComposition({...plan,nodes:[{id:'n',panel:'missing'}]},doc));
console.log('compact composition: full-scene bounds including negative arrow points, panel membership, finite geometry, ratio and text-size negatives passed; no scientific validity claim');
