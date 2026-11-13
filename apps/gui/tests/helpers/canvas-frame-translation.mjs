import assert from 'node:assert/strict';
const omit=(v,keys)=>Object.fromEntries(Object.entries(v).filter(([k])=>!keys.includes(k)));
const bookkeeping=['version','versionNonce','updated'];
export function checkFrameTranslation(before,after,id,dx,dy){
 assert.deepEqual(omit(after,['elements']),omit(before,['elements']));assert.equal(after.elements.length,before.elements.length);
 const frame=before.elements.find(e=>e.id===id);assert.equal(frame?.type,'frame');
 const cross=before.elements.filter(e=>e.type==='arrow'&&e.customData?.crossPanel===true&&[e.startBinding?.elementId,e.endBinding?.elementId].some(endpoint=>before.elements.find(n=>n.id===endpoint)?.frameId===id)).map(e=>e.id);
 for(const old of before.elements){const actual=after.elements.find(e=>e.id===old.id);assert.ok(actual);
  if(cross.includes(old.id)){assert.deepEqual(omit(actual,[...bookkeeping,'x','y','width','height','points','startBinding','endBinding']),omit(old,[...bookkeeping,'x','y','width','height','points','startBinding','endBinding']));assert.equal(actual.startBinding.elementId,old.startBinding.elementId);assert.equal(actual.endBinding.elementId,old.endBinding.elementId);}
  else if(cross.includes(old.containerId)){assert.deepEqual(omit(actual,[...bookkeeping,'x','y']),omit(old,[...bookkeeping,'x','y']));}
  else if(old.id===id||old.frameId===id){const expected=structuredClone(old);expected.x+=dx;expected.y+=dy;if(expected.customData?.visualAnchorFit?.imageBounds){expected.customData.visualAnchorFit.imageBounds[0]+=dx;expected.customData.visualAnchorFit.imageBounds[1]+=dy;}assert.deepEqual(omit(actual,bookkeeping),omit(expected,bookkeeping));}
  else assert.deepEqual(actual,old);
 }
 return {target_frame:id,dx,dy,cross_panel_relations:cross.length,all_unrelated_objects_preserved:true};
}
