import assert from 'node:assert/strict';
import {hasProtectedCopyTarget,prepareNativePaste,stampNativeIdentities,repairNativeDuplicate} from '../src/features/canvas/canvas-native-duplicate.ts';
const original=[{id:'anchor',groupIds:['g'],customData:{visualAssetElementId:'image',visualAssetGroupId:'g',visualLabelIds:['label'],note:'preserve'}},{id:'image',groupIds:['g'],customData:{semanticAnchorId:'anchor',asset:{author:'Artist'}}},{id:'label',groupIds:['g'],text:'Caption'}];
const serialized=JSON.stringify(original);stampNativeIdentities(original);assert.equal(JSON.stringify(original),serialized,'runtime markers never enter JSON');
const copies=original.map(e=>({...e,id:`new-${e.id}`,groupIds:['new-g'],customData:e.customData?structuredClone(e.customData):undefined}));
const repaired=repairNativeDuplicate([...original,...copies],original);
assert.equal(repaired[3].customData.visualAssetElementId,'new-image');assert.equal(repaired[4].customData.semanticAnchorId,'new-anchor');assert.deepEqual(repaired[3].customData.visualLabelIds,['new-label']);assert.equal(repaired[3].customData.visualAssetGroupId,'new-g');assert.equal(JSON.stringify(original),serialized);
assert.ok(!JSON.stringify(repaired).includes('__autonomics_copy_'));
const second=repaired.slice(3).map(e=>({...e,id:`second-${e.id}`,groupIds:['second-g']}));const twice=repairNativeDuplicate([...repaired,...second],repaired);assert.equal(twice.at(-2).customData.semanticAnchorId,'second-new-anchor');
const standalone=repairNativeDuplicate([...original,{...copies[1]}],original);assert.equal(standalone.at(-1).customData.semanticAnchorId,undefined,'uncopied owners detach');
assert.throws(()=>repairNativeDuplicate([...original,{id:'unverified',groupIds:['g'],customData:{semanticAnchorId:'anchor'}}],original));
assert.throws(()=>repairNativeDuplicate([...original,{...copies[1],customData:{inset:{}}}],original));
assert.throws(()=>repairNativeDuplicate([...original,{...copies[0],groupIds:[]}],original));
console.log('canvas-native-duplicate: exact runtime identities, JSON omission, repeated copies, ownership/group remap, external detach and unverifiable/protected copy rejection passed');
assert.throws(()=>repairNativeDuplicate([...original,{...copies[1],customData:{semanticAnchorId:{invalid:true}}}],original));
assert.throws(()=>repairNativeDuplicate([...original,{...copies[0],customData:{visualLabelIds:[null]}}],original));
assert.throws(()=>repairNativeDuplicate([...original,{...copies[0],customData:{visualAssetGroupId:{invalid:true}}}],original));

const clipboard=JSON.parse(serialized);const paste=prepareNativePaste(clipboard,undefined,{});
const pasted=paste.elements.map(e=>({...e,id:`paste-${e.id}`,groupIds:['paste-g']}));
const pastedScene=repairNativeDuplicate(pasted,[]);assert.equal(pastedScene[1].customData.semanticAnchorId,'paste-anchor');assert.equal(pastedScene[0].customData.visualAssetGroupId,'paste-g');assert.ok(!JSON.stringify(pastedScene).includes('__autonomics_copy_'));assert.equal(JSON.stringify(clipboard),serialized);
const conflicting={shared:{id:'shared',dataURL:'data:image/png;base64,OLD'}};
const imported=prepareNativePaste([{id:'img',groupIds:[],fileId:'shared'}],{shared:{id:'shared',dataURL:'data:image/png;base64,NEW'}},conflicting);assert.notEqual(imported.elements[0].fileId,'shared');assert.equal(imported.files[imported.elements[0].fileId].dataURL,'data:image/png;base64,NEW');assert.equal(conflicting.shared.dataURL,'data:image/png;base64,OLD');
assert.throws(()=>prepareNativePaste([{id:'a',groupIds:[],customData:{inset:{}}}],undefined,{}));
console.log('canvas-native-duplicate: cross-scene paste identity bridge, JSON omission, source isolation and conflicting image file remap passed');

const skeletons=[{type:'rectangle',x:0,y:0,width:10,height:10},{type:'ellipse',x:20,y:0,width:10,height:10}];assert.deepEqual(prepareNativePaste(skeletons,undefined,{}).elements,skeletons,'ordinary programmatic paste keeps native ID generation');
const reused=prepareNativePaste([{id:'same',groupIds:[],fileId:'shared'}],conflicting,conflicting);assert.equal(reused.elements[0].fileId,'shared');assert.deepEqual(reused.files,conflicting);

assert.throws(()=>prepareNativePaste([{id:'missing-image',type:'image',groupIds:[],fileId:'shared'}],undefined,conflicting));

assert.equal(repaired[3],copies[0],"native post-callback binding repair keeps the new object reference");

const protectedGroup=[{id:'frame',groupIds:[],type:'frame'},{id:'annotation',groupIds:['detail'],frameId:'frame',customData:{inset:{}}},{id:'title',groupIds:['detail'],frameId:'frame'},{id:'normal',groupIds:[]}];
assert.equal(hasProtectedCopyTarget(protectedGroup,{title:true}),true);assert.equal(hasProtectedCopyTarget(protectedGroup,{frame:true}),true);assert.equal(hasProtectedCopyTarget(protectedGroup,{annotation:false},'title'),true);assert.equal(hasProtectedCopyTarget(protectedGroup,{normal:true}),false);assert.equal(hasProtectedCopyTarget(protectedGroup,{annotation:false}),false);
console.log('canvas-native-duplicate: protected group/frame selection and pointer-hit expansion without blocking ordinary targets passed');
