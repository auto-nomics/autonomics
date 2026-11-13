import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {checkWholeMaterialization,wholeArtQualifier} from './helpers/canvas-whole-materialization.mjs';
const text=(id,label,x,y,containerId=null)=>({id,type:'text',text:label,originalText:label,x,y,width:80,height:20,angle:0,fontFamily:9,fontSize:16,lineHeight:1.25,containerId,frameId:'f'});
const before={elements:[{id:'f',type:'frame',x:0,y:0,width:500,height:300,angle:0},...['a','b'].flatMap((id,i)=>[{id,type:'rectangle',x:20+i*200,y:20,width:100,height:60,frameId:'f',customData:{scientificId:id,evidence:['user-declared']},boundElements:[{id:id+'label',type:'text'},{id:'edge',type:'arrow'}]},text(id+'label',id.toUpperCase(),30+i*200,35,id)]),{id:'edge',type:'arrow',x:120,y:50,width:100,height:0,angle:0,roughness:0,points:[[0,0],[100,0]],startBinding:{elementId:'a'},endBinding:{elementId:'b'},startArrowhead:null,endArrowhead:'arrow',customData:{from:'a',to:'b',evidence:['user-declared']}}],files:{},history:{keep:'exact'}};
const after=structuredClone(before);
for(const [i,id]of ['a','b'].entries()){
 const anchor=after.elements.find(e=>e.id===id),label=after.elements.find(e=>e.id===id+'label'),svg=`<svg xmlns="http://www.w3.org/2000/svg"><rect width="${40+i}" height="60"/></svg>`,hash=createHash('sha256').update(svg).digest('hex'),imageId=id+'image';
 Object.assign(anchor,{strokeColor:'transparent',backgroundColor:'transparent',groupIds:[id+'group'],boundElements:[{id:'edge',type:'arrow'}]});Object.assign(anchor.customData,{visualAssetElementId:imageId,visualLabelIds:[label.id],visualAnchorFit:{fileId:hash}});Object.assign(label,{x:20+i*200,y:100,containerId:null,groupIds:[id+'group']});
 after.files[hash]={dataURL:'data:image/svg+xml;base64,'+Buffer.from(svg).toString('base64')};
 after.elements.push({id:imageId,type:'image',x:20+i*200,y:20,width:40,height:60,angle:0,frameId:'f',fileId:hash,groupIds:[id+'group'],customData:{semanticAnchorId:id,sha256:hash,source_sha256:createHash('sha256').update('original source '+id).digest('hex'),asset:{id:'asset-'+id,name:'generic '+id,author:'fixture',license:'cc-0',source_url:'https://bioicons.com/?query=fixture'}}});
}
after.elements.push({...text('legend',wholeArtQualifier,20,340),width:450,height:80,frameId:null});
const result=checkWholeMaterialization(before,after);assert.equal(result.components,2);assert.equal(result.images,2);
for(const mutate of [d=>d.elements.find(e=>e.id==='aimage').customData.asset.license='',d=>d.elements.find(e=>e.id==='alabel').originalText='Shortened',d=>d.elements.find(e=>e.id==='alabel').fontSize=12,d=>d.elements.find(e=>e.id==='edge').endBinding.elementId='a',d=>d.elements.find(e=>e.id==='edge').customData.evidence=[],d=>d.elements.find(e=>e.id==='aimage').x=-100,d=>d.elements.find(e=>e.id==='aimage').customData.sha256='wrong',d=>d.elements.find(e=>e.id==='alabel').y=35,d=>d.elements.find(e=>e.id==='legend').originalText='Generic art',d=>d.elements.find(e=>e.id==='a').boundElements=[],d=>d.elements.find(e=>e.id==='legend').opacity=0,d=>d.elements.find(e=>e.id==='alabel').opacity=0,d=>d.elements.find(e=>e.id==='aimage').opacity=0,d=>d.elements.find(e=>e.id==='legend').strokeColor='transparent',d=>d.elements.push({...d.elements.find(e=>e.id==='alabel'),isDeleted:true})]){const bad=structuredClone(after);mutate(bad);assert.throws(()=>checkWholeMaterialization(before,bad));}
console.log('whole materialization contract: ownership, full labels, native relation bindings, provenance hashes, frames and overlap bounds passed');

const complexBefore=structuredClone(before),complexAfter=structuredClone(after);
for(const scene of [complexBefore,complexAfter]){const label=scene.elements.find(e=>e.id==='alabel');label.originalText=label.text='SMAD2 complex';label.width=160;}
checkWholeMaterialization(complexBefore,complexAfter);
complexAfter.elements.find(e=>e.id==='aimage').customData.asset.name='inorganic-phosphate';
assert.throws(()=>checkWholeMaterialization(complexBefore,complexAfter),/bare phosphate state marker/);

for(const scene of [complexBefore,complexAfter]){const label=scene.elements.find(e=>e.id==='alabel');label.originalText=label.text='Phosphate';}
checkWholeMaterialization(complexBefore,complexAfter); // The glyph is valid for a phosphate entity, not a protein state.

const unexpected=structuredClone(after);unexpected.elements.push({...text("unrequested-claim","New unsupported mechanism",20,450),frameId:null});assert.throws(()=>checkWholeMaterialization(before,unexpected),/new objects must be/);
