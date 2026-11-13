// Whole-scene preservation oracle; generic art never verifies molecular structure.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {sampledIllustrationLabelOverlaps,illustrationLabelOverlaps} from '../../src/features/canvas/canvas-illustration-overlaps.ts';
import {illustrationConnectorLabelOverlaps} from '../../src/features/canvas/canvas-connector-label-overlaps.ts';
import {textOverlapChanges} from '../../src/features/canvas/canvas-text-overlaps.ts';
const backendPaintProof=Symbol('backend SVG paint review');
const omit=(value,keys)=>Object.fromEntries(Object.entries(value).filter(([key])=>!keys.includes(key)));
const normal=text=>String(text??'').replace(/\s/gu,'');
const optionalMetadata=value=>value==null?{}:value;

// Native connector references are an unordered collection; preserve every field and multiplicity.
const connectorReferences=items=>[...(items??[])].sort((a,b)=>String(a.id).localeCompare(String(b.id))||String(a.type).localeCompare(String(b.type)));

export const wholeArtQualifier='图中图标均为通用示意，不代表 TGFB1、TGFBR1/2 或 SMAD2/3/4 的特异结构、复合物化学计量或磷酸化位点；具体状态与区室以完整文字标签及所列来源为准。';
function contained(element,frame){
 const angle=element.angle??0,c=Math.abs(Math.cos(angle)),s=Math.abs(Math.sin(angle)),w=element.width*c+element.height*s,h=element.width*s+element.height*c;
 const x=element.x+element.width/2-w/2,y=element.y+element.height/2-h/2;
 return x>=frame.x-1e-6&&y>=frame.y-1e-6&&x+w<=frame.x+frame.width+1e-6&&y+h<=frame.y+frame.height+1e-6;
}
export function checkWholeMaterialization(before,after,options={}){
 const {qualifier=wholeArtQualifier,minDistinctAssets=2}=options,paint=options[backendPaintProof];
 const old=before.elements.filter(e=>!e.isDeleted),next=after.elements.filter(e=>!e.isDeleted),index=new Map(next.map(e=>[e.id,e]));
 assert.equal(index.size,next.length);assert.equal(new Set(after.elements.map(e=>e.id)).size,after.elements.length,'deleted objects cannot duplicate live native IDs');assert.deepEqual(omit(after,['elements','files']),omit(before,['elements','files']));
 for(const [id,file]of Object.entries(before.files??{}))assert.deepEqual(after.files[id],file);
 const anchors=old.filter(e=>e.customData?.scientificId),ids=new Set(anchors.map(e=>e.id)),images=next.filter(e=>e.type==='image');
 const primary=images.filter(e=>typeof e.customData?.semanticAnchorId==='string'),badges=images.filter(e=>typeof e.customData?.semanticBadgeAnchorId==='string');
 assert.ok(anchors.length>1);assert.equal(primary.length,anchors.length);assert.equal(new Set(primary.map(e=>e.customData.semanticAnchorId)).size,anchors.length);assert.equal(primary.length+badges.length,images.length,'each image must be exactly one primary illustration or explicitly owned badge');
 for(const image of images){assert.equal(image.opacity??100,100);const asset=image.customData?.asset;assert.ok(asset?.id&&asset.name&&asset.author&&asset.license);assert.ok(asset.source_url?.startsWith('https://bioicons.com/'));assert.match(image.customData.source_sha256??'',/^[a-f0-9]{64}$/);const file=after.files[image.fileId];assert.ok(file?.dataURL?.startsWith('data:image/svg+xml;base64,'));assert.equal(createHash('sha256').update(Buffer.from(file.dataURL.split(',')[1],'base64')).digest('hex'),image.customData.sha256);}
 for(const anchor of next.filter(e=>ids.has(e.id))){const declared=anchor.customData?.visualBadgeIds??[];assert.ok(Array.isArray(declared)&&declared.length<=32&&declared.every(id=>typeof id==='string'));assert.equal(new Set(declared).size,declared.length);assert.deepEqual([...declared].sort(),badges.filter(e=>e.customData.semanticBadgeAnchorId===anchor.id).map(e=>e.id).sort());}
 for(const badge of badges){const owner=index.get(badge.customData.semanticBadgeAnchorId);assert.ok(owner&&ids.has(owner.id),'badge owner is an original scientific component');assert.ok(badge.customData.semanticAnchorId==null);assert.ok(typeof badge.customData.badgeMeaning==='string'&&badge.customData.badgeMeaning.trim()&&Buffer.byteLength(badge.customData.badgeMeaning)<=512&&!/^\s*data:/i.test(badge.customData.badgeMeaning));const group=owner.customData.visualAssetGroupId;assert.ok(typeof group==='string'&&group);assert.equal(badge.customData.visualBadgeGroupId,group);assert.ok([badge,owner,index.get(owner.customData.visualAssetElementId)].every(e=>Array.isArray(e?.groupIds)&&e.groupIds.includes(group)),'badge shares its component native group');assert.equal(badge.frameId,owner.frameId);const frame=index.get(owner.frameId);if(frame)assert.equal(contained(badge,frame),true,'complete badge remains in component frame');}

 assert.equal(new Set(primary.map(e=>e.customData?.asset?.id)).size>=minDistinctAssets,true,'different component classes use more than one glyph');
 assert.deepEqual(next.filter(e=>e.customData?.scientificId).map(e=>e.id).sort(),[...ids].sort());
 assert.deepEqual(next.filter(e=>e.type==='arrow').map(e=>e.id).sort(),old.filter(e=>e.type==='arrow').map(e=>e.id).sort());
 assert.deepEqual(next.filter(e=>e.type==='frame').map(e=>e.id).sort(),old.filter(e=>e.type==='frame').map(e=>e.id).sort());
 const labels=new Set();
 for(const original of anchors){
  const anchor=index.get(original.id),image=primary.find(e=>e.customData?.semanticAnchorId===original.id);assert.ok(anchor&&image);assert.equal(anchor.type,original.type);assert.equal(anchor.frameId,original.frameId);
  for(const [key,value]of Object.entries(original.customData??{}))assert.deepEqual(anchor.customData[key],value,`preserve scientific metadata ${original.id}.${key}`);
  assert.equal(anchor.customData.visualAssetElementId,image.id);assert.equal(anchor.customData.visualAnchorFit?.fileId,image.fileId,'fit belongs to the current SVG file');assert.deepEqual(connectorReferences(anchor.boundElements),connectorReferences((original.boundElements??[]).filter(b=>b.type!=='text')),'retain exact reciprocal connector reference identities, types and multiplicity');assert.equal(image.frameId,anchor.frameId);assert.equal(anchor.strokeColor,'transparent');assert.equal(anchor.backgroundColor,'transparent');
  const owned=old.filter(e=>e.type==='text'&&e.containerId===original.id);assert.ok(owned.length);assert.deepEqual([...anchor.customData.visualLabelIds].sort(),owned.map(e=>e.id).sort());
  const frame=index.get(anchor.frameId);if(frame){assert.equal(contained(image,frame),true,'full image viewport stays in its semantic frame');}
  for(const source of owned){labels.add(source.id);const label=index.get(source.id);assert.ok(label);assert.equal(label.containerId,null);assert.equal(label.frameId,source.frameId);if(frame)assert.equal(contained(label,frame),true,'complete component label remains inside frame');assert.ok(anchor.groupIds?.some(id=>image.groupIds?.includes(id)&&label.groupIds?.includes(id)));}
  assert.equal(image.opacity??100,100,'SVG instance cannot be hidden through native opacity');assert.match(image.customData.source_sha256??'',/^[a-f0-9]{64}$/,'original download hash is retained separately');const asset=image.customData.asset;assert.ok(asset?.id&&asset.name&&asset.author&&asset.license);assert.ok(asset.source_url?.startsWith('https://bioicons.com/'));
  if(/TGFBR|SMAD|protein|蛋白/i.test(owned.map(e=>e.originalText??e.text).join(' ')))assert.doesNotMatch(asset.name,/^(?:inorganic[-_ ]?)?phosphate(?:[-_ ](?:ion|group|molecule))?$/i,'a bare phosphate state marker cannot replace an entire protein or receptor complex');
  if(/TGFBR[12]/i.test(owned.map(e=>e.originalText??e.text).join(' ')))assert.doesNotMatch(asset.name,/7helix|3helix|calciumsensing|g.?protein|gpcr|channel/i,'do not use named wrong membrane topology/receptor family');
  const file=after.files[image.fileId];assert.ok(file?.dataURL?.startsWith('data:image/svg+xml;base64,'));assert.equal(createHash('sha256').update(Buffer.from(file.dataURL.split(',')[1],'base64')).digest('hex'),image.customData.sha256);
 }
 for(const original of old){const actual=index.get(original.id);assert.ok(actual,`preserve ${original.id}`);assert.equal(actual.type,original.type,'retain native element types');
  if(original.type==='text'){
   assert.equal(actual.originalText??actual.text,original.originalText??original.text);assert.equal(normal(actual.text),normal(original.originalText??original.text));
   assert.equal(actual.opacity??100,original.opacity??100,'preserve text visibility');assert.deepEqual(actual.strokeColor,original.strokeColor);
   for(const key of ['fontFamily','fontSize','lineHeight','frameId'])assert.deepEqual(actual[key],original[key]);assert.deepEqual(optionalMetadata(actual.customData),optionalMetadata(original.customData),'preserve every scientific/custom metadata field; null/absent metadata has no fields');
   assert.ok(actual.width>0&&actual.height>0);if(!labels.has(original.id))assert.equal(actual.containerId??null,original.containerId??null);
  }else if(original.type==='arrow'){
   assert.deepEqual(actual.customData,original.customData);for(const key of ['startBinding','endBinding'])assert.equal(actual[key]?.elementId,original[key]?.elementId);
   for(const key of ['strokeColor','strokeWidth','strokeStyle','roughness','startArrowhead','endArrowhead','opacity','roundness'])assert.deepEqual(actual[key],original[key]);
  }else if(original.type==='frame'){
   assert.deepEqual(omit(actual,['x','y','width','height','version','versionNonce','updated']),omit(original,['x','y','width','height','version','versionNonce','updated']));
  }else if(!ids.has(original.id))assert.deepEqual(actual,original);
 }
 const legend=next.find(e=>!old.some(p=>p.id===e.id)&&e.type==='text'&&normal(e.originalText??e.text)===normal(qualifier));assert.ok(legend,'new separate visible generic-art qualification');assert.ok(legend.fontSize>=16);assert.equal(legend.opacity??100,100,'qualification must remain visible');assert.notEqual(legend.strokeColor,'transparent');
 const originalIds=new Set(old.map(e=>e.id));for(const element of next)if(!originalIds.has(element.id))assert.ok(element.type==='image'||element.id===legend.id,'new objects must be the requested artwork instances or generic qualification, not unrequested claims');
 const ownOverlap=illustrationLabelOverlaps(after),connectorOverlap=illustrationConnectorLabelOverlaps(after),textChanges=textOverlapChanges(before,after);
 if(paint){
  assert.equal(paint.supported_images,images.length);assert.equal(paint.complete,true,'all owned label checks covered');assert.equal(paint.all_image_text.complete,true,'all image/text pairs covered');assert.deepEqual(paint.viewport,ownOverlap);
  assert.equal(paint.sampled.count,0,'padded sampled SVG bounds do not overlap owned labels');assert.equal(paint.all_image_text.sampled.count,0,'padded sampled SVG bounds do not overlap any text');
 }else assert.equal(ownOverlap.count,0,'image viewports do not overlap owned labels');assert.equal(ownOverlap.skipped+ownOverlap.omitted,0);assert.equal(ownOverlap.checked,labels.size);
 assert.equal(connectorOverlap.count,0,'supported connector paths do not cross illustration labels');assert.equal(connectorOverlap.skipped+connectorOverlap.omitted,0);
 assert.equal(textChanges.counts.introduced,0,'no introduced supported text rectangle overlaps');assert.equal(textChanges.counts.unknown,0);
 return {components:anchors.length,images:images.length,primary_images:primary.length,badges:badges.length,unique_assets:new Set(images.map(e=>e.customData.asset.id)).size,owned_label_overlap:ownOverlap,sampled_paint_review:paint??null,connector_label_overlap:connectorOverlap,text_overlap_changes:textChanges};
}

/** Only feed the real backend receipt for this exact pending candidate, never model-authored bounds. */
export async function checkWholeMaterializationWithImageInfo(before,candidate,receipt,options={}){
 assert.ok(candidate.id&&candidate.document_sha256);
 assert.equal(receipt.proposal_id,candidate.id);assert.equal(receipt.document_sha256,candidate.document_sha256);assert.ok(Array.isArray(receipt.infos));
 const paint=await sampledIllustrationLabelOverlaps(candidate.document,receipt.infos);
 return checkWholeMaterialization(before,candidate.document,{...options,[backendPaintProof]:paint});
}
