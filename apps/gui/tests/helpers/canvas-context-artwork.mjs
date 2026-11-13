// Context artwork remains separate from the scientific molecular graph.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
export const contextNoteId='mitochondria-context-note';
export const contextNote='线粒体素材为通用区室示意，不代表CYCS结构或本图具体膜状态。';
// SDK text_content appends this marker for image blocks; preserve all JSON data.
export function previewMetadata(content){
 assert.equal(typeof content,'string');
 return JSON.parse(content.replace(/(?:\s*\[image omitted\])+\s*$/,''));
}
const live=doc=>doc.elements.filter(e=>!e.isDeleted);
const intersects=(a,b)=>a.x<b.x+b.width&&b.x<a.x+a.width&&a.y<b.y+b.height&&b.y<a.y+a.height;
const inside=(a,b)=>a.x>=b.x&&a.y>=b.y&&a.x+a.width<=b.x+b.width&&a.y+a.height<=b.y+b.height;
function geometry(element){
 assert.ok(['x','y','width','height'].every(k=>Number.isFinite(element[k]))&&element.width>=0&&element.height>=0,'context placement requires finite native extents');
 assert.equal(element.angle??0,0,'rotated objects require explicit reviewed placement');
 return element;
}
export function contextArtworkSlot(document){
 const elements=live(document);elements.forEach(geometry);
 const origins=elements.filter(e=>e.customData?.scientificId&&elements.some(label=>label.type==='text'&&label.containerId===e.id&&/CYCS|细胞色素\s*c/i.test(label.originalText??label.text)&&/膜间隙|intermembrane/i.test(label.originalText??label.text)));
 assert.equal(origins.length,1,'one explicit mitochondrial CYCS origin is required');
 const frame=elements.find(e=>e.id===origins[0].frameId&&e.type==='frame');assert.ok(frame,'origin belongs to an explicit visual panel');
 const choices=[{x:frame.x+20,y:frame.y+frame.height+24,width:280,height:360,placement:'below_origin_panel'},
  {x:Math.min(...elements.map(e=>e.x))-304,y:frame.y+24,width:280,height:360,placement:'beside_origin_panel'}];
 const slot=choices.find(candidate=>elements.every(e=>!intersects(candidate,e)));assert.ok(slot,'no conservative artwork slot; request a reviewed layout change');
 return {...slot,origin_id:origins[0].id,frame_id:frame.id};
}
export function checkContextArtwork(before,after,slot){
 const existing=new Map(before.elements.map(e=>[e.id,e]));assert.equal(new Set(after.elements.map(e=>e.id)).size,after.elements.length);
 for(const [id,prior] of existing)assert.deepEqual(after.elements.find(e=>e.id===id),prior,`original ${id} stays unchanged`);
 assert.deepEqual(Object.fromEntries(Object.entries(after).filter(([k])=>!['elements','files'].includes(k))),Object.fromEntries(Object.entries(before).filter(([k])=>!['elements','files'].includes(k))));
 for(const [id,file] of Object.entries(before.files??{}))assert.deepEqual(after.files[id],file);
 const added=after.elements.filter(e=>!existing.has(e.id));assert.equal(added.length,2,'one contextual image and one explicit qualification only');
 const image=added.find(e=>e.type==='image'),note=added.find(e=>e.id===contextNoteId&&e.type==='text');assert.ok(image&&note);
 for(const element of added){geometry(element);assert.ok(inside(element,slot),'new art and note stay in the reviewed free region');for(const prior of live(before))assert.ok(!intersects(element,prior),`new artwork avoids original ${prior.id}`);}
 assert.ok(!intersects(image,note));assert.equal(note.originalText??note.text,contextNote);assert.equal(note.fontSize,16);assert.ok(note.width<=264);assert.ok(note.y>=image.y+image.height+12);assert.equal(note.fontFamily,9);assert.equal(note.containerId??null,null);
 assert.equal(image.frameId??null,null);assert.equal(image.customData.semanticAnchorId??null,null,'organelle image never impersonates a CYCS semantic replacement');
 assert.equal(image.customData.scientificId??null,null);assert.deepEqual(image.groupIds,[]);assert.deepEqual(image.scale,[1,1]);assert.equal(image.crop??null,null);
 const asset=image.customData.asset;assert.match(asset.title??asset.name,/mitochondri/i);assert.doesNotMatch(asset.title??asset.name,/damaged/i);
 assert.ok(asset.creators?.length&&asset.licenses?.length);assert.equal(asset.source_url,`https://bioart.niaid.nih.gov/bioart/${asset.id}`);
 assert.ok(Object.keys(after.files??{}).filter(id=>!Object.hasOwn(before.files??{},id)).every(id=>id===image.fileId),'no unrelated new files');
 const file=after.files[image.fileId];assert.equal(file.mimeType,'image/png');assert.ok(file.dataURL.startsWith('data:image/png;base64,'));
 const bytes=Buffer.from(file.dataURL.split(',')[1],'base64');assert.ok(bytes.length>=24&&bytes.subarray(0,8).equals(Buffer.from([137,80,78,71,13,10,26,10])),'actual embedded PNG signature');
 const width=bytes.readUInt32BE(16),height=bytes.readUInt32BE(20);assert.ok(width>0&&height>0);const scale=Math.min(240/Math.max(width,height),1);assert.ok(Math.abs(image.width-width*scale)<1e-6&&Math.abs(image.height-height*scale)<1e-6,'original default image size and aspect ratio');
 const hash=createHash('sha256').update(bytes).digest('hex');assert.equal(hash,image.customData.sha256);
 return {image,note};
}
export function checkContextImageMove(before,after,imageId){
 assert.equal(after.elements.length,before.elements.length);assert.deepEqual(after.files,before.files);
 assert.deepEqual(Object.fromEntries(Object.entries(after).filter(([k])=>k!=='elements')),Object.fromEntries(Object.entries(before).filter(([k])=>k!=='elements')));
 for(const prior of before.elements){const actual=after.elements.find(e=>e.id===prior.id);assert.ok(actual);if(prior.id!==imageId)assert.deepEqual(actual,prior);else{
  assert.equal(actual.x,prior.x+8);const omit=e=>Object.fromEntries(Object.entries(e).filter(([k])=>!['x','version','versionNonce','updated'].includes(k)));assert.deepEqual(omit(actual),omit(prior));
 }}
}
