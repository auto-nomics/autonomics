import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {sampledIllustrationLabelOverlaps} from '../src/features/canvas/canvas-illustration-overlaps.ts';
const svg='<svg/>',hash=createHash('sha256').update(svg).digest('hex');
const doc={elements:[{id:'anchor',customData:{visualAssetElementId:'image',visualLabelIds:['label']}},{id:'image',type:'image',fileId:'f',x:0,y:0,width:100,height:100,customData:{semanticAnchorId:'anchor'}},{id:'label',type:'text',x:70,y:10,width:20,height:20}],files:{f:{dataURL:'data:image/svg+xml;base64,'+Buffer.from(svg).toString('base64')}}};
const info={element_id:'image',file_id:'f',sha256:hash,svg_preview_alpha_bounds:{file_sha256:hash,scope:'uncropped_file_normalized_svg_preview',status:'available',normalized_bounds:[0.1,0.1,0.2,0.2]}};
const snapshot=JSON.stringify(doc),review=await sampledIllustrationLabelOverlaps(doc,[info]);
assert.equal(review.viewport.count,1);assert.equal(review.sampled.count,0);assert.equal(review.complete,true);assert.equal(JSON.stringify(doc),snapshot);
for(const mutate of [i=>i.sha256='bad',i=>i.file_id='wrong',i=>i.svg_preview_alpha_bounds.normalized_bounds=[0,0,2,1]]){const bad=structuredClone(info);mutate(bad);const r=await sampledIllustrationLabelOverlaps(doc,[bad]);assert.equal(r.complete,false);assert.equal(r.sampled.count,1);}
for(const props of [{crop:{}},{angle:1},{scale:[2,1]}]){const copy=structuredClone(doc);Object.assign(copy.elements[1],props);assert.equal((await sampledIllustrationLabelOverlaps(copy,[info])).complete,false);}
const flipped=structuredClone(doc);flipped.elements[1].scale=[-1,1];assert.equal((await sampledIllustrationLabelOverlaps(flipped,[info])).sampled.count,1);
assert.equal((await sampledIllustrationLabelOverlaps(doc,[info,info])).complete,false);
console.log('sampled paint diagnostics: transparent margins, mirrors, source identity, unsupported fallback and immutability passed');

const invalidLabel=structuredClone(doc);invalidLabel.elements[2].width=0;const incomplete=await sampledIllustrationLabelOverlaps(invalidLabel,[info]);assert.equal(incomplete.supported_images,1);assert.equal(incomplete.sampled.skipped,1);assert.equal(incomplete.complete,false);

const unrelated=structuredClone(doc);unrelated.elements.push({id:"unrelated",type:"text",x:10,y:10,width:20,height:20});const scoped=await sampledIllustrationLabelOverlaps(unrelated,[info]);assert.equal(scoped.scope,"illustrations_and_their_owned_unbound_labels_only");assert.equal(scoped.sampled.checked,1);assert.equal(scoped.sampled.count,0);
assert.equal(scoped.all_image_text.sampled.count,1);
assert.equal(scoped.all_image_text.sampled.pairs[0].label_id,'unrelated');
assert.equal(scoped.all_image_text.complete,true);
const many=structuredClone(doc);for(let i=0;i<2001;i++)many.elements.push({id:`caption-${i}`,type:'text',x:200,y:200,width:10,height:10});
const bounded=await sampledIllustrationLabelOverlaps(many,[info]);assert.equal(bounded.all_image_text.sampled.checked,2000);assert.ok(bounded.all_image_text.sampled.omitted>0);assert.equal(bounded.all_image_text.complete,false);
