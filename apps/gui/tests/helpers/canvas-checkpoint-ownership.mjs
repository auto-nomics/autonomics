import assert from 'node:assert/strict';
import {canvasMaterializationProgress} from '../../src/features/canvas/canvas-selection-context.ts';
export function checkCheckpointIllustrationOwnership(before,after,activeIds){
 assert.equal(canvasMaterializationProgress(after.elements).unassigned_images.total,0,'all artwork has explicit reciprocal ownership');
 const existing=new Set(before.elements.filter(e=>e.type==='image'&&!e.isDeleted).map(e=>e.id));
 for(const image of after.elements.filter(e=>e.type==='image'&&!e.isDeleted&&!existing.has(e.id))){
  const data=image.customData??{},ownerId=data.semanticAnchorId??data.semanticBadgeAnchorId;
  assert.notEqual(Boolean(data.semanticAnchorId),Boolean(data.semanticBadgeAnchorId),'artwork has exactly one ownership role');
  assert.ok(activeIds.includes(ownerId),'new artwork belongs to an active original component');
  const owner=after.elements.find(e=>e.id===ownerId);assert.ok(owner);assert.equal(image.frameId,owner.frameId);
  const group=owner.customData?.visualAssetGroupId;assert.ok(group&&image.groupIds?.includes(group));
  if(data.semanticBadgeAnchorId){assert.ok(owner.customData.visualBadgeIds?.includes(image.id));assert.equal(data.visualBadgeGroupId,group);assert.ok(data.badgeMeaning?.trim());}
  else{assert.equal(owner.customData.visualAssetElementId,image.id);assert.doesNotMatch(data.asset?.name??'',/^(?:inorganic[-_ ]?)?phosphate(?:[-_ ](?:ion|group|molecule))?$/i,'phosphate cannot replace a primary complex');}
 }
}
