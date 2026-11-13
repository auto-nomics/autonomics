// Structural preservation for two explicitly requested semantic replacements.
// This does not validate experimental protocols or icon biological specificity.
import assert from 'node:assert/strict';
const omit=(element,keys)=>Object.fromEntries(Object.entries(element).filter(([key])=>!keys.includes(key)));
const bookkeeping=['version','versionNonce','updated'];
const normalize=text=>text.replace(/\s/g,'');
export function checkLocalComponentLabelMove(previous,next,labelId,dy=8){
 const label=previous.elements.find(e=>e.id===labelId);assert.ok(label?.type==='text'&&!label.containerId&&previous.elements.some(e=>e.customData?.visualLabelIds?.includes(labelId)),'local target is an owned illustration label');
 assert.deepEqual(omit(next,['elements']),omit(previous,['elements']));assert.equal(next.elements.length,previous.elements.length);
 for(const old of previous.elements){const actual=next.elements.find(e=>e.id===old.id);assert.ok(actual);
  if(old.id!==labelId)assert.deepEqual(actual,old,`local label edit preserves ${old.id}`);
  else{assert.equal(actual.y,old.y+dy);assert.deepEqual(omit(actual,[...bookkeeping,'y']),omit(old,[...bookkeeping,'y']));}
 }
}
export function checkMaterializedComponents(parent,child,assetIds){
 const before=parent.document,after=child.document,index=new Map(after.elements.map(e=>[e.id,e]));
 const anchors=before.elements.filter(e=>e.customData?.scientificId);assert.equal(anchors.length,2);
 const images=after.elements.filter(e=>e.type==='image'&&!e.isDeleted);assert.equal(images.length,2);
 assert.equal(new Set(after.elements.map(e=>e.id)).size,after.elements.length,'native IDs remain unique');
 assert.equal(new Set(images.map(e=>e.customData.semanticAnchorId)).size,2);
 assert.equal(new Set(images.map(e=>e.customData.asset.id)).size,2);
 assert.equal(after.elements.length,before.elements.length+2);
 assert.deepEqual(omit(after,['elements','files']),omit(before,['elements','files']));
 for(const [id,file]of Object.entries(before.files??{}))assert.deepEqual(after.files[id],file);
 const changed=new Set();
 for(const old of anchors){
  const actual=index.get(old.id),image=images.find(e=>e.customData.semanticAnchorId===old.id);assert.ok(actual&&image);
  assert.equal(actual.customData.visualAssetElementId,image.id);assert.ok(assetIds.includes(image.customData.asset.id));
  assert.equal(actual.type,'rectangle');assert.equal(actual.strokeColor,'transparent');assert.equal(actual.backgroundColor,'transparent');
  assert.deepEqual(actual.boundElements,(old.boundElements??[]).filter(binding=>binding.type!=='text'),'non-text reciprocal bindings survive');
  assert.ok(image.customData.asset.author&&image.customData.asset.license&&image.customData.source_sha256);
  assert.ok(after.files[image.fileId].dataURL.startsWith('data:image/svg+xml;base64,'));
  assert.ok([image.x,image.y,image.width,image.height].every(Number.isFinite)&&image.width>0&&image.height>0);
  assert.ok(image.x>=old.x-1e-6&&image.y>=old.y-1e-6&&image.x+image.width<=old.x+old.width+1e-6&&image.y+image.height<=old.y+old.height+1e-6,'image stays inside its declared component slot');
  assert.deepEqual(omit(actual,[...bookkeeping,'type','strokeColor','backgroundColor','strokeWidth','groupIds','boundElements','customData']),omit(old,[...bookkeeping,'type','strokeColor','backgroundColor','strokeWidth','groupIds','boundElements','customData']));
  assert.deepEqual(omit(actual.customData,['visualAssetElementId','visualAssetGroupId','visualLabelIds']),old.customData);
  const oldLabels=before.elements.filter(e=>e.type==='text'&&e.containerId===old.id);assert.equal(oldLabels.length,1);
  assert.deepEqual(actual.customData.visualLabelIds,oldLabels.map(e=>e.id));
  const group=actual.customData.visualAssetGroupId;assert.equal(typeof group,'string');assert.ok(image.groupIds.includes(group));
  for(const oldLabel of oldLabels){const label=index.get(oldLabel.id);assert.ok(label);assert.equal(label.containerId,null);assert.ok(label.groupIds.includes(group));
   assert.equal(normalize(label.text),normalize(oldLabel.text));
   assert.deepEqual(omit(label,[...bookkeeping,'containerId','groupIds','x','y','width','height','text','autoResize']),omit(oldLabel,[...bookkeeping,'containerId','groupIds','x','y','width','height','text','autoResize']));
   changed.add(label.id);
  }
  const assetName=image.customData.asset.name;assert.equal(typeof assetName,'string','catalogue name is distinct from opaque asset identity');
  assert.match(assetName,old.customData.scientificId==='culture'?/petri|flask|cultur|dish/i:/microscop/i,'the two selected catalogue identities represent different requested objects');
  changed.add(old.id);
 }
 for(const old of before.elements)if(!changed.has(old.id))assert.deepEqual(index.get(old.id),old,`unrelated ${old.id} unchanged`);
 assert.deepEqual(child.review_markups,parent.review_markups);
 return images;
}
