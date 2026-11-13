import assert from 'node:assert/strict';
const without=(element,fields)=>Object.fromEntries(Object.entries(element).filter(([key])=>!fields.includes(key)));
const bookkeeping=['version','versionNonce','updated'];
export function checkIllustrationResize(previous,next,targetId,factor=1.1){
 const old=previous.elements.find(e=>e.id===targetId),anchor=next.elements.find(e=>e.id===targetId);
 assert.ok(old&&anchor);
 const visualId=old.customData.visualAssetElementId;
 const image=previous.elements.find(e=>e.id===visualId),actual=next.elements.find(e=>e.id===visualId);
 assert.ok(image&&actual);assert.deepEqual(without(next,["elements"]),without(previous,["elements"]),"scene settings, plan and files survive");
 assert.equal(next.elements.length,previous.elements.length);
 for(const [key,value]of Object.entries({width:image.width*factor,height:image.height*factor,x:image.x+(image.width-image.width*factor)/2,y:image.y+(image.height-image.height*factor)/2}))assert.ok(Math.abs(actual[key]-value)<1e-6,`requested illustration ${key}`);
 assert.deepEqual(without(actual,[...bookkeeping,'x','y','width','height']),without(image,[...bookkeeping,'x','y','width','height']));
 for(const [key,value]of Object.entries({width:old.width*factor,height:old.height*factor,x:actual.x+(old.x-image.x)*factor,y:actual.y+(old.y-image.y)*factor}))assert.ok(Math.abs(anchor[key]-value)<1e-6,`fitted anchor ${key}`);
 assert.deepEqual(anchor.customData.visualAnchorFit.imageBounds,[actual.x,actual.y,actual.width,actual.height]);
 assert.deepEqual(without(anchor,[...bookkeeping,"x","y","width","height","customData"]),without(old,[...bookkeeping,"x","y","width","height","customData"]));
 assert.deepEqual(anchor.customData.visualAnchorFit.normalizedBounds,old.customData.visualAnchorFit.normalizedBounds);
 assert.deepEqual(without(anchor.customData,['visualAnchorFit','visualAssetSlot']),without(old.customData,['visualAnchorFit','visualAssetSlot']));
 const arrows=previous.elements.filter(e=>e.type==='arrow'&&(e.startBinding?.elementId===targetId||e.endBinding?.elementId===targetId));
 const labels=previous.elements.filter(e=>e.type==='text'&&arrows.some(a=>a.id===e.containerId));
 const changed=new Set([targetId,visualId,...arrows.map(e=>e.id),...labels.map(e=>e.id)]);
 for(const element of previous.elements){const current=next.elements.find(e=>e.id===element.id);assert.ok(current);if(!changed.has(element.id))assert.deepEqual(current,element,`unrelated ${element.id}`);}
 for(const element of arrows){const current=next.elements.find(e=>e.id===element.id);assert.deepEqual(without(current,[...bookkeeping,'x','y','width','height','points']),without(element,[...bookkeeping,'x','y','width','height','points']));}
 for(const element of labels){const current=next.elements.find(e=>e.id===element.id);assert.deepEqual(without(current,[...bookkeeping,'x','y']),without(element,[...bookkeeping,'x','y']));}
 return {imageId:visualId,factor};
}

// Match the actual successful call to its immutable reviewed parent and owned file.
export function reviewedResizeCall(calls,previous,targetId,scope,factor=1.1){
 const anchor=previous.document.elements.find(e=>e.id===targetId),image=previous.document.elements.find(e=>e.id===anchor?.customData?.visualAssetElementId);
 if(!anchor||!image||image.customData?.semanticAnchorId!==targetId)return false;
 const owned=id=>id===targetId||id===image.id;
 return calls.some(call=>{
  const input=call.input;if(call.is_error||!input||input.project_id!==scope.projectId||input.figure_id!==scope.figureId||input.expected_revision!==previous.base_revision||input.proposal_id!==previous.id||input.document_sha256!==previous.document_sha256)return false;
  if(call.name==='visualization_scale_illustration')return owned(input.element_id)&&input.file_id===image.fileId&&input.factor===factor;
  if(call.name!=='visualization'||input.action!=='revise_proposal')return false;
  return input.operations?.some(op=>op.op==='set_illustration_geometry'&&owned(op.id)&&op.file_id===image.fileId&&JSON.stringify(op.normalized_bounds)===JSON.stringify(anchor.customData.visualAnchorFit?.normalizedBounds))??false;
 });
}
