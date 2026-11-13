import assert from 'node:assert/strict';
import {checkIllustrationResize,reviewedResizeCall} from './helpers/canvas-illustration-resize-contract.mjs';
const previous={elements:[{id:'a',type:'rectangle',x:5,y:0,width:10,height:40,customData:{scientificId:'receptor',evidence:['read source'],visualAssetElementId:'art',visualAnchorFit:{normalizedBounds:[.25,0,.5,1]}}},{id:'art',type:'image',x:0,y:0,width:20,height:40,fileId:'file',customData:{semanticAnchorId:'a',asset:{license:'cc0'}}},{id:'label',type:'text',text:'Full scientific label',originalText:'Full scientific label',x:0,y:60}],files:{file:{dataURL:'exact source pixels'}}};
const next=structuredClone(previous);
Object.assign(next.elements[1],{x:-1,y:-2,width:22,height:44});
Object.assign(next.elements[0],{x:4.5,y:-2,width:11,height:44});next.elements[0].customData.visualAnchorFit.imageBounds=[-1,-2,22,44];
checkIllustrationResize(previous,next,'a');
for(const mutate of [v=>v.elements[1].customData.asset.license='changed',v=>v.elements[2].text='Short label',v=>v.elements[0].width=12,v=>v.elements[0].customData.evidence=[],v=>v.files.file.dataURL='different pixels',v=>v.elements.push({id:'extra'}),v=>v.appState={viewBackgroundColor:'#000000'},v=>v.elements[0].opacity=0]){const invalid=structuredClone(next);mutate(invalid);assert.throws(()=>checkIllustrationResize(previous,invalid,'a'));}
console.log('illustration resize contract: requested center/scale, anchor geometry, label/source/file/identity preservation and negative cases passed');

const parent={id:'parent',document_sha256:'hash',base_revision:2,document:previous},scope={projectId:'p',figureId:'f'};
const scalar={name:'visualization_scale_illustration',input:{project_id:'p',figure_id:'f',proposal_id:'parent',document_sha256:'hash',expected_revision:2,element_id:'art',file_id:'file',factor:1.1}};
assert.ok(reviewedResizeCall([scalar],parent,'a',scope));
for(const field of ['project_id','figure_id','proposal_id','document_sha256','file_id','element_id']){const bad=structuredClone(scalar);bad.input[field]='wrong';assert.equal(reviewedResizeCall([bad],parent,'a',scope),false);}
for(const mutate of [v=>v.is_error=true,v=>v.input.factor='1.1',v=>v.input.factor=1.2,v=>v.input.expected_revision=3]){const bad=structuredClone(scalar);mutate(bad);assert.equal(reviewedResizeCall([bad],parent,'a',scope),false);}
const legacy={name:'visualization',input:{...scalar.input,action:'revise_proposal',operations:[{op:'set_illustration_geometry',id:'a',file_id:'file',normalized_bounds:[.25,0,.5,1]}]}};assert.ok(reviewedResizeCall([legacy],parent,'a',scope));legacy.input.operations[0].normalized_bounds=[0,0,1,1];assert.equal(reviewedResizeCall([legacy],parent,'a',scope),false);
console.log('resize call provenance: scalar and explicit geometry entries retain exact parent/hash/scope/owner/file; wrong identity, factor, error and reviewed bounds rejected');
