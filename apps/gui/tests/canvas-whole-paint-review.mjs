import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {checkWholeMaterialization,checkWholeMaterializationWithImageInfo} from './helpers/canvas-whole-materialization.mjs';
const root=new URL('../../../target/visualization-evaluations/',import.meta.url),read=async name=>JSON.parse(await readFile(new URL(name,root),'utf8'));
const before=(await read('tgfb-whole-art-model-v213/whole-art-parent.json')).document,candidate=await read('whole-paint-review-v234/candidate.json'),receipt=await read('whole-paint-review-v234/backend-image-info.json');
const good=await checkWholeMaterializationWithImageInfo(before,candidate,receipt);assert.equal(good.images,8);assert.equal(good.owned_label_overlap.count,3);assert.equal(good.sampled_paint_review.all_image_text.sampled.count,0);
let rejected=0;
for(const mutate of [r=>r.proposal_id='wrong',r=>r.document_sha256='wrong',r=>r.infos=[],r=>r.infos[0].sha256='wrong',r=>r.infos[0].svg_preview_alpha_bounds.normalized_bounds=[0,0,1,1],r=>r.infos.push(r.infos[0])]){const bad=structuredClone(receipt);mutate(bad);await assert.rejects(()=>checkWholeMaterializationWithImageInfo(before,candidate,bad));rejected++;}
const semantic=structuredClone(candidate);semantic.document.elements.find(e=>e.type==='image'&&e.customData.asset.name==='receptor').customData.asset.name='inorganic-phosphate';await assert.rejects(()=>checkWholeMaterializationWithImageInfo(before,semantic,receipt),/bare phosphate state marker/);rejected++;
assert.throws(()=>checkWholeMaterialization(before,candidate.document,{sampled_paint_review:{complete:true,count:0}}));rejected++;
await writeFile(new URL('whole-paint-review-v234/negative-review.json',root),JSON.stringify({passed:true,rejected_cases:rejected,scope:'saved real-model candidate and exact native backend image-info receipt; not scientific or publication-quality validation'},null,2));
console.log(`whole paint review: real 8-component preservation passed; ${rejected} identity/coverage/collision/semantic/forged-statistic negatives rejected`);
