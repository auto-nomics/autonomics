import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {svgPreviewFitBounds} from '../src/features/canvas/canvas-svg-preview-fit.ts';
const bytes=Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><path d="M1 1h8v8H1z"/></svg>'),hash=createHash('sha256').update(bytes).digest('hex');
const candidate={id:'p',document_sha256:'exact-candidate',document:{elements:[{id:'i',type:'image',fileId:'f',angle:0,crop:null}],files:{f:{mimeType:'image/svg+xml',dataURL:'data:image/svg+xml;base64,'+bytes.toString('base64')}}}};
const receipt={proposal_id:'p',document_sha256:'exact-candidate',infos:[{element_id:'i',file_id:'f',sha256:hash,svg_preview_alpha_bounds:{status:'available',method:'nonzero_8bit_alpha_with_one_preview_pixel_padding',scope:'uncropped_file_normalized_svg_preview',file_sha256:hash,normalized_bounds:[0.1,0.1,0.8,0.8]}}]};
assert.deepEqual(await svgPreviewFitBounds(candidate,'i',receipt),[0.1,0.1,0.8,0.8]);
let rejected=0;
for(const mutate of [p=>p.proposal_id='other',p=>p.document_sha256='old',p=>p.infos[0].file_id='other',p=>p.infos[0].sha256='bad',p=>p.infos[0].svg_preview_alpha_bounds.file_sha256='bad',p=>p.infos[0].svg_preview_alpha_bounds.status='empty',p=>p.infos[0].svg_preview_alpha_bounds.normalized_bounds=[0,0,2,1],p=>p.infos.push(structuredClone(p.infos[0]))]){const proof=structuredClone(receipt);mutate(proof);await assert.rejects(()=>svgPreviewFitBounds(candidate,'i',proof));rejected++;}
for(const mutate of [c=>c.document.elements[0].angle=1,c=>c.document.elements[0].crop={},c=>c.document.files.f.mimeType='image/png',c=>c.document.files.f.dataURL+='AA',c=>c.document.elements.push(structuredClone(c.document.elements[0]))]){const changed=structuredClone(candidate);mutate(changed);await assert.rejects(()=>svgPreviewFitBounds(changed,'i',receipt));rejected++;}
console.log(`SVG preview fit: exact candidate/file bytes verified; ${rejected} stale/unsupported/ambiguous cases rejected`);
