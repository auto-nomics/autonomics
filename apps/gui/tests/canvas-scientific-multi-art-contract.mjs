// Synthetic preservation contracts; catalogue discovery is tested by the live workflow.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {statDimerTarget,checkSecondMaterialization} from './helpers/canvas-scientific-multi-art.mjs';
const nodes=[{id:'gaf',label:'STAT1 homodimer (GAF, cytoplasm)'},{id:'nucleus',label:'Nuclear STAT1 homodimer'}];
assert.equal(statDimerTarget({nodes}).id,'gaf');
for(const nodes of [[{id:'x',label:'STAT3 homodimer (cytoplasm)'}],[{id:'x',label:'Nuclear STAT1 homodimer'}],[{id:'x',label:'Nuclear STAT1 homodimer from cytoplasm'}],[{id:'x',label:'GAF'}],[{id:'x',label:'STAT1 homodimer (cytoplasm)'},{id:'y',label:'STAT1 homodimer (胞质)'}]])assert.throws(()=>statDimerTarget({nodes}));
const box=(id,type,x,y,width=100,height=60)=>({id,type,x,y,width,height});
const text=(id,source,containerId=null)=>({...box(id,'text',400,280,200,40),text:source,originalText:source,containerId,fontFamily:9,fontSize:20,lineHeight:1.25});
const before={elements:[
 {...box('rec','rectangle',0,0),customData:{scientificId:'receptor',evidence:['retained receptor source']}},
 {...box('rec-image','image',0,0),fileId:'rec-file',customData:{semanticAnchorId:'rec',asset:{id:1,creators:['Fixture creator'],licenses:['Fixture license']}}},
 {...text('rec-label','Receptor'),x:0,y:100},
 {...box('gaf','rectangle',400,200,220,100),customData:{scientificId:'gaf',evidence:['retained dimer source']}},
 text('gaf-label',nodes[0].label,'gaf'),
 {...box('edge','arrow',400,0),startBinding:{elementId:'upstream'},endBinding:{elementId:'gaf'},startArrowhead:null,endArrowhead:'arrow',customData:{evidence:['retained transition']}},
 text('edge-label','Forms a dimer','edge')
],files:{'rec-file':{dataURL:'fixture data'}},scientificPlan:{message:'Fixture only, not scientific validation'}};
const after=structuredClone(before),target=before.elements.find(e=>e.id==='gaf');
const anchor=after.elements.find(e=>e.id==='gaf');Object.assign(anchor.customData,{visualAssetElementId:'stat-image',visualLabelIds:['gaf-label']});after.elements.find(e=>e.id==='gaf-label').containerId=null;
const png=Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j8R8AAAAASUVORK5CYII=','base64');
after.files['stat-file']={dataURL:`data:image/png;base64,${png.toString('base64')}`};
after.elements.push({...box('stat-image','image',400,200),fileId:'stat-file',customData:{semanticAnchorId:'gaf',sha256:createHash('sha256').update(png).digest('hex'),asset:{id:2,title:'STAT activated dimer fixture',source_url:'https://bioart.niaid.nih.gov/bioart/2',creators:['Fixture creator'],licenses:['Fixture license']}}},
 {...text('stat-note','STAT 素材为家族级示意，不验证 STAT1 特异结构或磷酸化位点。'),x:0,y:1000,fontSize:16,frameId:null});
checkSecondMaterialization(before,after,target,'stat-note');
for(const mutate of [
 d=>d.elements.find(e=>e.id==='rec').x++,
 d=>d.elements.find(e=>e.id==='rec-image').customData.asset.creators=['invented'],
 d=>d.files['rec-file'].dataURL='replaced',
 d=>d.elements.find(e=>e.id==='gaf-label').originalText='shortened',
 d=>d.elements.find(e=>e.id==='edge').endArrowhead=null,
 d=>d.elements.find(e=>e.id==='edge').startArrowhead='arrow',
 d=>d.elements.find(e=>e.id==='edge').customData.evidence=['invented'],
 d=>d.elements.find(e=>e.id==='edge-label').text='shortened',
 d=>d.elements.find(e=>e.id==='stat-image').fileId='rec-file',
 d=>d.elements.find(e=>e.id==='stat-image').customData.asset.title='Unrelated protein',
 d=>d.files['stat-file'].dataURL='data:image/png;base64,dGFtcGVyZWQ=',
 d=>d.elements.find(e=>e.id==='stat-note').originalText='STAT1 specific structure is verified',
 d=>d.elements.find(e=>e.id==='stat-note').y=0,
 d=>d.scientificPlan.message='rewritten',
 d=>d.elements.push(structuredClone(d.elements[0]))
]){const invalid=structuredClone(after);mutate(invalid);assert.throws(()=>checkSecondMaterialization(before,invalid,target,'stat-note'));}
console.log('scientific multi-art: explicit STAT1 dimer target, original component/files, labels/evidence/arrowheads, distinct hashed catalogue image and non-specificity note preserved; synthetic negative edits rejected');
