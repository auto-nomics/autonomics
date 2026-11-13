import assert from 'node:assert/strict';
import {checkMaterializedComponents,checkLocalComponentLabelMove} from './helpers/canvas-materialized-components.mjs';
const parent={review_markups:[],document:{files:{},elements:[]}};
for(const [i,role]of ['culture','imaging'].entries()){
 parent.document.elements.push({id:role,type:'rectangle',x:i*400,y:0,width:220,height:100,angle:0,strokeColor:'black',backgroundColor:'blue',strokeWidth:2,groupIds:[],boundElements:[{id:role+'-label',type:'text'}],customData:{scientificId:role,planningSource:{version:7}}});
 parent.document.elements.push({id:role+'-label',type:'text',x:i*400+10,y:30,width:200,height:25,fontFamily:9,fontSize:20,lineHeight:1.25,originalText:role,text:role,containerId:role,groupIds:[],autoResize:true});
}
parent.document.elements.push({id:'relation',type:'arrow',customData:{evidence:['declared transition']}});
const child=structuredClone(parent),ids=['petri-dish','microscope'];
for(const [i,role]of ['culture','imaging'].entries()){
 const anchor=child.document.elements.find(e=>e.id===role),label=child.document.elements.find(e=>e.id===role+'-label'),imageId=role+'-image',group=role+'-group',file=role+'-file';
 Object.assign(anchor,{strokeColor:'transparent',backgroundColor:'transparent',groupIds:[group],boundElements:[]});
 Object.assign(anchor.customData,{visualAssetElementId:imageId,visualAssetGroupId:group,visualLabelIds:[label.id]});
 Object.assign(label,{containerId:null,groupIds:[group],y:112,autoResize:false});
 child.document.elements.push({id:imageId,type:'image',x:i*400+60,y:0,width:100,height:100,fileId:file,groupIds:[group],customData:{semanticAnchorId:role,source_sha256:'fixture',asset:{id:ids[i],name:ids[i],author:'Fixture author',license:'Fixture license'}}});
 child.document.files[file]={dataURL:'data:image/svg+xml;base64,fixture'};
}
checkMaterializedComponents(parent,child,ids);
const roundoff=structuredClone(child);roundoff.document.elements.find(e=>e.id==='culture-image').y=-7.105427357601002e-15;roundoff.document.elements.find(e=>e.id==='culture-image').height=100.00000000000001;checkMaterializedComponents(parent,roundoff,ids);
for(const [key,value]of [['x',-0.001],['y',-0.001],['width',221],['height',101]]){const invalid=structuredClone(child);invalid.document.elements.find(e=>e.id==='culture-image')[key]=value;assert.throws(()=>checkMaterializedComponents(parent,invalid,ids),/declared component slot/);}
const opaque=structuredClone(child);const opaqueIds=['opaque-hash-a','opaque-hash-b'];opaque.document.elements.filter(e=>e.type==='image').forEach((e,i)=>e.customData.asset.id=opaqueIds[i]);checkMaterializedComponents(parent,opaque,opaqueIds);
const wrongName=structuredClone(opaque);wrongName.document.elements.find(e=>e.id==='culture-image').customData.asset.name='unrelated icon';assert.throws(()=>checkMaterializedComponents(parent,wrongName,opaqueIds));
const moved=structuredClone(child.document);moved.elements.find(e=>e.id==='culture-label').y+=8;
checkLocalComponentLabelMove(child.document,moved,'culture-label');
assert.throws(()=>checkLocalComponentLabelMove(child.document,child.document,'missing-label'),/owned illustration label/);
const unrelated=structuredClone(moved);unrelated.elements.find(e=>e.id==='imaging-image').x+=1;assert.throws(()=>checkLocalComponentLabelMove(child.document,unrelated,'culture-label'));
for(const mutate of [v=>v.document.elements.find(e=>e.id==='culture-image').customData.semanticAnchorId='imaging',v=>v.document.elements.find(e=>e.id==='culture').customData.planningSource.version=8,v=>v.document.elements.find(e=>e.id==='culture-label').originalText='removed word',v=>v.document.elements.find(e=>e.id==='imaging-label').fontSize=16,v=>v.document.elements.find(e=>e.id==='relation').customData.evidence=['invented'],v=>v.document.elements.find(e=>e.id==='culture-image').x=900]){
 const invalid=structuredClone(child);mutate(invalid);assert.throws(()=>checkMaterializedComponents(parent,invalid,ids));
}
console.log('semantic materialization: two owned illustrations accepted; identity, history, complete labels, font, relation evidence and slot violations rejected');
