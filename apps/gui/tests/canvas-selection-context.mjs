import assert from 'node:assert/strict';
import {canvasSelectionContext} from '../src/features/canvas/canvas-selection-context.ts';
const reference={kind:'image_inset_snapshot',anchor_id:'panel',image_id:'detail',region_id:'region',callout_id:'leader',source_element_id:'source',source_revision:3,region:{x:.1,y:.2,width:.4,height:.5},private_note:'PRIVATE-REFERENCE'};
const elements=[
 {id:'source',type:'image',x:1,y:2,width:200,height:100,angle:.7,scale:[-1,-1],crop:{x:100,y:20,width:200,height:80,naturalWidth:400,naturalHeight:200},fileId:'source-file',customData:{user_note:'PRIVATE-NOTE',dataURL:'PRIVATE-BYTES'}},
 {id:'panel',type:'rectangle',customData:{inset:reference,visualAssetElementId:'detail',visualLabelIds:['title'],visualAssetGroupId:'detail-group'}},
 {id:'detail',type:'image',fileId:'detail-file',customData:{inset:reference,semanticAnchorId:'panel'}},
 {id:'region',type:'rectangle',customData:{inset:reference,annotationKind:'inset_region'}},
 {id:'leader',type:'arrow',startBinding:{elementId:'region'},endBinding:{elementId:'panel'},customData:{inset:reference,annotationKind:'inset_callout'}},
 {id:'title',type:'text',text:'Detail',groupIds:['detail-group']},
 {id:'deleted',type:'rectangle',isDeleted:true},
];
const selected=Object.fromEntries(elements.map(e=>[e.id,true]));
const snapshot=canvasSelectionContext(elements,selected);
assert.equal(snapshot.total,6);assert.equal(snapshot.truncated,false);
assert.deepEqual(snapshot.objects.map(e=>e.role),['canvas_object','inset_panel','inset_image','inset_region','inset_callout','inset_title']);
const source=snapshot.objects[0];assert.equal(source.angle,.7);assert.deepEqual(source.scale,[-1,-1]);assert.deepEqual(source.crop,elements[0].crop);
assert.equal(snapshot.objects[2].semantic_anchor_verified,true);
assert.equal(snapshot.objects[4].start_binding_id,'region');assert.equal(snapshot.objects[4].end_binding_id,'panel');
assert.ok(!JSON.stringify(snapshot).includes('PRIVATE-'));
assert.equal(canvasSelectionContext(elements,{source:false,detail:true}).total,1,'false selection map entries are not selected');
const moved=structuredClone(elements);moved[0].x=45;
assert.notDeepEqual(canvasSelectionContext(moved,{source:true}),canvasSelectionContext(elements,{source:true}),'geometry changes update context even with unchanged IDs');
const missingSource=canvasSelectionContext(elements.filter(e=>e.id!=='source'),{detail:true});
assert.equal(missingSource.objects[0].inset.source_available,false);assert.equal(missingSource.objects[0].inset.identity_verified,true,'frozen snapshot survives source removal');
const copied={...elements[2],id:'copied-image'};
const copy=canvasSelectionContext([...elements,copied],{'copied-image':true}).objects[0];
assert.equal(copy.role,'unverified_inset_reference');assert.equal(copy.inset.identity_verified,false);assert.equal(copy.semantic_anchor_verified,false);
const malformed=structuredClone(elements);malformed[3].customData.inset={...reference,source_revision:4};
assert.equal(canvasSelectionContext(malformed,{region:true}).objects[0].role,'unverified_inset_reference');
const detached=structuredClone(elements);detached[5].groupIds=[];
assert.equal(canvasSelectionContext(detached,{title:true}).objects[0].role,'canvas_object','do not infer title ownership after ungrouping');
const many=Array.from({length:150},(_,n)=>({id:`element-${n}`,type:'text',text:'x'.repeat(1200)}));
const large=canvasSelectionContext(many,Object.fromEntries(many.map(e=>[e.id,true])));
assert.equal(large.total,150);assert.equal(large.truncated,true);assert.ok(large.objects.length<100);assert.ok(JSON.stringify(large.objects).length<=16000);assert.equal(large.objects[0].text_truncated,true);
const oversized=canvasSelectionContext([{id:'z'.repeat(513),type:'rectangle'}],{['z'.repeat(513)]:true});
assert.equal(oversized.total,1);assert.equal(oversized.truncated,true);assert.deepEqual(oversized.objects,[],'never silently truncate opaque IDs');
console.log('canvas-selection-context: transform/crop, verified roles, false/deleted selections, copied/ambiguous ownership, privacy and bounded completeness passed');

const grouped=canvasSelectionContext([{id:'group-member',type:'rectangle',groupIds:Array.from({length:40},(_,n)=>`group-${n}`)}],{'group-member':true}).objects[0];
assert.equal(grouped.group_ids.length,32);assert.equal(grouped.group_ids_truncated,true);assert.equal(grouped.group_ids[0],'group-0');
