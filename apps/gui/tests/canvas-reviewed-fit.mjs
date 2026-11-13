import assert from 'node:assert/strict';import {readFileSync} from 'node:fs';
import {reviewedFit} from './helpers/canvas-reviewed-fit.mjs';
const proposals=[{id:'p',document_sha256:'hash',document:{elements:[{id:'anchor',customData:{visualAssetElementId:'image'}},{id:'image',type:'image',fileId:'file',customData:{semanticAnchorId:'anchor'}},{id:'other',type:'image',fileId:'file',customData:{semanticAnchorId:'elsewhere'}}]}}];
const call={name:'visualization_fit',is_error:false,input:{proposal_id:'p',document_sha256:'hash',element_id:'image',file_id:'file',x:.1,y:.1,width:.8,height:.8}};
assert.ok(reviewedFit(call,proposals,'anchor'));assert.ok(reviewedFit({...call,input:{...call.input,element_id:'anchor'}},proposals,'anchor'));
for(const patch of [{document_sha256:'stale'},{proposal_id:'elsewhere'},{element_id:'other'},{file_id:'stale'},{width:800},{x:'0.1'}])assert.equal(reviewedFit({...call,input:{...call.input,...patch}},proposals,'anchor'),false);
assert.equal(reviewedFit({...call,is_error:true},proposals,'anchor'),false);
const invalid=structuredClone(proposals);invalid[0].document.elements[1].customData.semanticAnchorId='other';assert.equal(reviewedFit(call,invalid,'anchor'),false);
if(process.env.CANVAS_FIT_REPLAY){const p=process.env.CANVAS_FIT_REPLAY;const read=n=>JSON.parse(readFileSync(`${p}/${n}`,'utf8'));const calls=read('turn-4.json').calls;const all=read('ifng-art-all-candidates.json');const target=read('workflow-stage-handoff.json').target_id;const fits=calls.filter(c=>reviewedFit(c,all,target));assert.equal(fits.length,1);console.log('actual v18: image-ID fit verified against exact parent hash, reciprocal ownership and source file');}
console.log('reviewed fit: image/anchor equivalence and wrong hash/owner/file/geometry/error negatives passed');
