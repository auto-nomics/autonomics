import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {textOverlapChanges} from '../../src/features/canvas/canvas-text-overlaps.ts';
const omit=(value,keys)=>Object.fromEntries(Object.entries(value).filter(([key])=>!keys.includes(key)));
export function checkMaterializationRoleRepair(document,after){
 const child={document:after},find=(doc,id)=>doc.elements.find(e=>e.id===id),target=document.elements.find(e=>e.customData?.scientificId==='endosome_psmad'),donor=document.elements.find(e=>e.customData?.scientificId==='endosome_complex');assert.ok(target&&donor);
 const oldImage=find(document,target.customData.visualAssetElementId),donorImage=find(document,donor.customData.visualAssetElementId),label=find(document,target.customData.visualLabelIds[0]);
 const anchor=find(child.document,target.id),image=find(child.document,anchor.customData.visualAssetElementId),actualLabel=find(child.document,label.id);assert.equal(image.customData.semanticAnchorId,target.id);assert.equal(anchor.frameId,target.frameId);assert.equal(image.frameId,target.frameId);assert.deepEqual(anchor.customData.visualLabelIds,target.customData.visualLabelIds);assert.deepEqual(anchor.boundElements,target.boundElements);assert.equal(image.customData.asset.id,donorImage.customData.asset.id);assert.equal(image.customData.asset.name,'receptor');assert.equal(anchor.customData.visualAnchorFit.fileId,image.fileId);assert.equal(child.document.elements.filter(e=>e.type==='image'&&!e.isDeleted).length,8);
 const file=child.document.files[image.fileId];assert.equal(createHash('sha256').update(Buffer.from(file.dataURL.split(',')[1],'base64')).digest('hex'),image.customData.sha256);
 for(const [key,value]of Object.entries(target.customData))if(!['visualAssetElementId','visualAssetGroupId','visualLabelIds','visualAnchorFit','visualAssetSlot'].includes(key))assert.deepEqual(anchor.customData[key],value);
 assert.equal(actualLabel.originalText,label.originalText);assert.equal(actualLabel.text.replace(/\s/gu,''),label.originalText.replace(/\s/gu,''));for(const key of ['fontFamily','fontSize','lineHeight','frameId','opacity','strokeColor'])assert.deepEqual(actualLabel[key],label[key]);
 const related=document.elements.filter(e=>e.type==='arrow'&&(e.startBinding?.elementId===target.id||e.endBinding?.elementId===target.id)),captions=document.elements.filter(e=>e.type==='text'&&related.some(a=>e.containerId===a.id)),allowed=new Set([target.id,oldImage.id,label.id,...related.map(e=>e.id),...captions.map(e=>e.id)]);
 for(const element of child.document.elements)if(!document.elements.some(old=>old.id===element.id))assert.equal(element.id,image.id,'no unrelated new objects');
 for(const old of document.elements)if(!allowed.has(old.id))assert.deepEqual(find(child.document,old.id),old,`preserve ${old.id}`);
 for(const old of related){const actual=find(child.document,old.id);for(const key of ['customData','startArrowhead','endArrowhead','strokeColor','strokeWidth','strokeStyle','roughness','opacity'])assert.deepEqual(actual[key],old[key]);for(const key of ['startBinding','endBinding'])assert.equal(actual[key].elementId,old[key].elementId);}
 for(const old of captions)assert.deepEqual(omit(find(child.document,old.id),['x','y','version','versionNonce','updated']),omit(old,['x','y','version','versionNonce','updated']));
 for(const [id,file]of Object.entries(document.files))assert.deepEqual(child.document.files[id],file);assert.deepEqual(omit(child.document,['elements','files']),omit(document,['elements','files']));assert.equal(textOverlapChanges(document,child.document).counts.introduced,0);

 const slot=anchor.customData.visualAssetSlot;assert.ok(Array.isArray(slot)&&slot.length===4&&slot.every(Number.isFinite)&&slot[2]>0&&slot[3]>0,'layout slot is geometric bookkeeping, not a normalized file rectangle');
 for(const [i,key]of ['x','y','width','height'].entries())assert.ok(Math.abs(anchor.customData.visualAnchorFit.imageBounds[i]-image[key])<1e-6,'fit records the current native image bounds');
 return {target_id:target.id,previous_asset:oldImage.customData.asset.name,replacement_asset:image.customData.asset.name,preserved_other_svg_instances:7};
}
