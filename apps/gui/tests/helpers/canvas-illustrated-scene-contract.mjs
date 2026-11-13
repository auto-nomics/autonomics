import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
const byId=(doc,id)=>doc.elements.find(e=>e.id===id);
const intersects=(a,b)=>a.x<b.x+b.width&&b.x<a.x+a.width&&a.y<b.y+b.height&&b.y<a.y+a.height;
export function checkIllustratedScene({child,parent,target,notes,assetProvider,assetId}){
 const nih=assetProvider==='bioart';
 assert.deepEqual(child.review_markups,parent.review_markups,'no review links added or discarded');
 const images=child.document.elements.filter(e=>e.type==='image'&&!e.isDeleted);assert.equal(images.length,1);const image=images[0];assert.equal(image.customData.semanticAnchorId,target.id);assert.equal(image.customData.asset.id,assetId);assert.ok((nih?image.customData.asset.creators?.length:image.customData.asset.author)&&(nih?image.customData.asset.licenses?.length:image.customData.asset.license)&&(nih?image.customData.sha256:image.customData.source_sha256));assert.ok(child.document.files[image.fileId].dataURL.startsWith(nih?'data:image/png;base64,':'data:image/svg+xml;base64,'));
 if(nih)assert.equal(createHash('sha256').update(Buffer.from(child.document.files[image.fileId].dataURL.split(',')[1],'base64')).digest('hex'),image.customData.sha256,'NIH provenance hash equals exact PNG bytes');
 const anchor=byId(child.document,target.id),oldLabel=parent.document.elements.find(e=>e.type==='text'&&e.containerId===target.id),label=byId(child.document,oldLabel.id);assert.equal(anchor.customData.visualAssetElementId,image.id);for(const field of ['scientificId','evidence'])assert.deepEqual(anchor.customData[field],target.customData[field]);assert.equal(label.containerId,null);for(const field of ['originalText','fontFamily','fontSize','lineHeight'])assert.deepEqual(label[field],oldLabel[field]);assert.equal(label.text.replace(/\s/g,''),oldLabel.text.replace(/\s/g,''));
 const arrows=parent.document.elements.filter(e=>e.type==='arrow'&&(e.startBinding?.elementId===target.id||e.endBinding?.elementId===target.id));const relatedText=parent.document.elements.filter(e=>e.type==='text'&&arrows.some(a=>e.containerId===a.id));
 for(const old of arrows){const actual=byId(child.document,old.id);assert.deepEqual(actual.customData,old.customData);for(const field of ['startArrowhead','endArrowhead'])assert.deepEqual(actual[field],old[field]);for(const field of ['startBinding','endBinding'])assert.equal(actual[field]?.elementId,old[field]?.elementId);}
 for(const old of relatedText){const actual=byId(child.document,old.id);assert.equal(actual.originalText,old.originalText);assert.equal(actual.text.replace(/\s/g,''),old.text.replace(/\s/g,''));}
 const delta=byId(child.document,notes[0]).y-byId(parent.document,notes[0]).y;assert.ok(delta>=24,'explicit caption clearance request requires at least 24 units of uniform movement');
 for(const id of notes){const old=byId(parent.document,id),actual=byId(child.document,id);assert.equal(actual.y-old.y,delta);for(const field of ['x','width','height','text','originalText','fontSize','fontFamily','lineHeight'])assert.deepEqual(actual[field],old[field]);assert.ok(!intersects(actual,label)&&!intersects(actual,image),'captions clear the detached label and illustration');}
 const changed=new Set([target.id,oldLabel.id,...arrows.map(e=>e.id),...relatedText.map(e=>e.id),...notes]);for(const old of parent.document.elements)if(!changed.has(old.id))assert.deepEqual(byId(child.document,old.id),old,`unrelated ${old.id} unchanged`);
 assert.equal(child.document.elements.length,parent.document.elements.length+1);
 return image;
}
