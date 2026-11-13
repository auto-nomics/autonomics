import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {contextArtworkSlot,checkContextArtwork,checkContextImageMove,contextNote,contextNoteId,previewMetadata} from './helpers/canvas-context-artwork.mjs';
const base={elements:[
 {id:'frame',type:'frame',x:0,y:0,width:320,height:480},
 {id:'origin',type:'rectangle',x:10,y:100,width:300,height:100,frameId:'frame',customData:{scientificId:'cycs',evidence:['source']}},
 {id:'label',type:'text',x:10,y:100,width:300,height:100,containerId:'origin',originalText:'CYCS（线粒体膜间隙）'},
 {id:'other-panel',type:'frame',x:460,y:0,width:320,height:960},
 {id:'scope',type:'text',x:10,y:1000,width:1200,height:120,originalText:'Source-qualified scientific scope'}
],files:{'retained-file':{dataURL:'fixture bytes preserved verbatim'}},scientificPlan:{keep:'unchanged'}};
const slot=contextArtworkSlot(base);assert.equal(slot.placement,'below_origin_panel');
const data='iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+nmX8AAAAASUVORK5CYII=',hash=createHash('sha256').update(Buffer.from(data,'base64')).digest('hex');
const image={id:'context-image',type:'image',x:slot.x+8,y:slot.y+8,width:1,height:1,angle:0,scale:[1,1],crop:null,groupIds:[],frameId:null,fileId:'new-file',customData:{sha256:hash,asset:{id:353,title:'Mitochondria',creators:['fixture author'],licenses:['fixture license'],source_url:'https://bioart.niaid.nih.gov/bioart/353'}}};
const note={id:contextNoteId,type:'text',x:slot.x+8,y:image.y+image.height+12,width:264,height:60,fontSize:16,fontFamily:9,originalText:contextNote};
const after=structuredClone(base);after.elements.push(image,note);after.files['new-file']={mimeType:'image/png',dataURL:`data:image/png;base64,${data}`};
checkContextArtwork(base,after,slot);
const moved=structuredClone(after);moved.elements.find(e=>e.id===image.id).x+=8;checkContextImageMove(after,moved,image.id);checkContextArtwork(base,moved,slot);
function reject(change,pattern){const bad=structuredClone(after);change(bad);assert.throws(()=>checkContextArtwork(base,bad,slot),pattern);}
reject(d=>{d.elements.find(e=>e.id==='origin').customData.evidence=[];},/original origin/);
reject(d=>{d.scientificPlan.keep='changed';});
reject(d=>{d.files['retained-file'].dataURL='changed';});
reject(d=>{d.elements.find(e=>e.id===image.id).customData.semanticAnchorId='origin';},/impersonates/);
reject(d=>{d.elements.find(e=>e.id===image.id).frameId='frame';});
reject(d=>{d.elements.find(e=>e.id===image.id).x=0;},/reviewed free region/);
reject(d=>{d.elements.find(e=>e.id===contextNoteId).originalText='CYCS structure';});
reject(d=>{d.elements.find(e=>e.id===image.id).customData.sha256='incorrect';});
reject(d=>{d.elements.find(e=>e.id===image.id).width=2;},/aspect ratio/);
reject(d=>{d.elements.push({...note,id:'unrequested-note'});},/one contextual image/);
const brokenMove=structuredClone(moved);brokenMove.elements.find(e=>e.id===contextNoteId).x+=8;assert.throws(()=>checkContextImageMove(after,brokenMove,image.id));
const crowded=structuredClone(base);crowded.elements.push({id:'occupied',type:'rectangle',...slot});assert.equal(contextArtworkSlot(crowded).placement,'beside_origin_panel');
const duplicate=structuredClone(base);duplicate.elements.push({...base.elements[1],id:'other-origin'},{...base.elements[2],id:'other-label',containerId:'other-origin'});assert.throws(()=>contextArtworkSlot(duplicate),/one explicit/);
console.log('context artwork: original graph/source preservation, conservative placement, independent identity, PNG/hash/aspect and image-only movement passed; synthetic contract only');

assert.deepEqual(previewMetadata('{"sha256":"kept","description":"[image omitted] inside a JSON value"}[image omitted]'),{sha256:'kept',description:'[image omitted] inside a JSON value'});
assert.throws(()=>previewMetadata('{"sha256":"kept"} unexpected trailer'));
