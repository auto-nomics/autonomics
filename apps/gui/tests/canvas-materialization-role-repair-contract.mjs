import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {checkMaterializationRoleRepair} from './helpers/canvas-materialization-role-repair.mjs';
const vector=fill=>`<svg xmlns="http://www.w3.org/2000/svg"><rect width="20" height="40" fill="${fill}"/></svg>`;
const source={elements:[{id:'f',type:'frame',x:0,y:0,width:1000,height:400}],files:{},history:{keep:'declared user scope'}};
for(let i=0;i<8;i++){
 const id='a'+i,imageId='image'+i,labelId='label'+i,svg=vector(i===0?'yellow':'pink'),hash=createHash('sha256').update(svg).digest('hex');source.files[hash]={dataURL:'data:image/svg+xml;base64,'+Buffer.from(svg).toString('base64')};
 source.elements.push({id,type:'rectangle',x:20+i*100,y:20,width:60,height:60,frameId:'f',boundElements:[],customData:{scientificId:i===0?'endosome_psmad':i===1?'endosome_complex':'other'+i,evidence:['declared scope'],visualAssetElementId:imageId,visualLabelIds:[labelId],visualAssetGroupId:'g'+i,visualAnchorFit:{fileId:hash,imageBounds:[20+i*100,20,20,40]},visualAssetSlot:[0,0,1,1]}},{id:imageId,type:'image',x:20+i*100,y:20,width:20,height:40,frameId:'f',fileId:hash,customData:{semanticAnchorId:id,sha256:hash,asset:{id:i===0?'phosphate':'receptor',name:i===0?'inorganic-phosphate':'receptor'}}},{id:labelId,type:'text',x:20+i*100,y:100,width:80,height:20,angle:0,frameId:'f',text:'Component '+i,originalText:'Component '+i,fontFamily:9,fontSize:16,lineHeight:1.25,opacity:100,strokeColor:'black',containerId:null});
}
const next=structuredClone(source),target=next.elements.find(e=>e.id==='a0'),image=next.elements.find(e=>e.id==='image0'),donor=next.elements.find(e=>e.id==='image1');image.fileId=donor.fileId;image.customData.sha256=donor.customData.sha256;image.customData.asset=structuredClone(donor.customData.asset);target.customData.visualAnchorFit.fileId=image.fileId;
// A fresh fit legitimately changes geometry bookkeeping while retaining science.
target.customData.visualAssetSlot=[-1.2,-0.5,3.2,1.5];
assert.equal(checkMaterializationRoleRepair(source,next).preserved_other_svg_instances,7);
for(const mutate of [d=>d.elements.find(e=>e.id==='label0').fontSize=12,d=>d.elements.find(e=>e.id==='image3').x+=8,d=>d.elements.find(e=>e.id==='a0').customData.evidence=[],d=>d.elements.find(e=>e.id==='image0').customData.sha256='wrong',d=>d.elements.find(e=>e.id==='a0').customData.visualAssetSlot=[0,0,0,1],d=>d.elements.push({id:'extra',type:'rectangle'}),d=>d.history.keep='different']){const bad=structuredClone(next);mutate(bad);assert.throws(()=>checkMaterializationRoleRepair(source,bad));}
console.log('role repair contract: derived layout slot may change; scientific metadata, other seven instances, hashes and original labels remain exact');
