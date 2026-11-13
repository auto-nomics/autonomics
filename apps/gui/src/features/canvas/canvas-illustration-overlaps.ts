type Row=Record<string,unknown>;
const row=(v:unknown):Row=>v&&typeof v==='object'&&!Array.isArray(v)?v as Row:{};
type Point=[number,number];
function corners(e:Row):Point[]|null{
 const {x,y,width,height}=e,angle=e.angle??0;
 if(![x,y,width,height,angle].every(v=>typeof v==='number'&&Number.isFinite(v))||Number(width)<=0||Number(height)<=0||e.opacity===0)return null;
 const cx=Number(x)+Number(width)/2,cy=Number(y)+Number(height)/2,s=Math.sin(Number(angle)),c=Math.cos(Number(angle));
 return [[-1,-1],[1,-1],[1,1],[-1,1]].map(([sx,sy])=>{const dx=sx*Number(width)/2,dy=sy*Number(height)/2;return [cx+c*dx-s*dy,cy+s*dx+c*dy] as Point;});
}
function intersects(a:Point[],b:Point[]){
 for(const polygon of [a,b])for(let i=0;i<2;i++){
  const p=polygon[i],q=polygon[i+1],axis=[-(q[1]-p[1]),q[0]-p[0]],length=Math.hypot(...axis);
  const project=(points:Point[])=>points.map(([x,y])=>(x*axis[0]+y*axis[1])/length);
  const first=project(a),second=project(b);
  if(Math.min(Math.max(...first),Math.max(...second))-Math.max(Math.min(...first),Math.min(...second))<=1e-6)return false;
 }
 return true;
}
/** Conservative viewport/label geometry only; transparent image margins may intersect. */
export function illustrationLabelOverlaps(document:unknown){
 const elements=(Array.isArray(row(document).elements)?row(document).elements as unknown[]:[]).map(row).filter(e=>e.isDeleted!==true);
 const index=new Map(elements.map(e=>[e.id,e]));let checked=0,skipped=0,omitted=0;
 const pairs:{anchor_id:string;image_id:string;label_id:string}[]=[];
 for(const anchor of elements){
  const data=row(anchor.customData),image=index.get(data.visualAssetElementId);
  if(typeof anchor.id!=='string'||!image||image.type!=='image'||row(image.customData).semanticAnchorId!==anchor.id||!Array.isArray(data.visualLabelIds))continue;
  for(const labelId of new Set(data.visualLabelIds)){
   const label=index.get(labelId);if(!label||label.type!=='text'||label.containerId)continue;
   if(checked+skipped>=200){omitted++;continue;}
   const a=corners(image),b=corners(label);if(!a||!b){skipped++;continue;}
   checked++;if(intersects(a,b))pairs.push({anchor_id:anchor.id,image_id:String(image.id),label_id:String(labelId)});
  }
 }
 return {method:'rotated_label_and_image_viewport_rectangles',checked,skipped,omitted,count:pairs.length,pairs};
}

/** All live image/text rectangles, including unowned and connector captions; bounded pair coverage. */
export function allIllustrationTextOverlaps(document:unknown){
 const elements=(Array.isArray(row(document).elements)?row(document).elements as unknown[]:[]).map(row).filter(e=>e.isDeleted!==true&&e.opacity!==0);
 const images=elements.filter(e=>e.type==='image'),texts=elements.filter(e=>e.type==='text');
 let checked=0,skipped=0,omitted=0;const pairs:{image_id:string;label_id:string}[]=[];
 for(const image of images)for(const label of texts){
  if(checked+skipped>=2000){omitted++;continue;}
  const a=corners(image),b=corners(label);if(!a||!b){skipped++;continue;}
  checked++;if(intersects(a,b))pairs.push({image_id:String(image.id),label_id:String(label.id)});
 }
 return {scope:'all_live_images_and_text',method:'rotated_image_and_text_rectangles',checked,skipped,omitted,count:pairs.length,pairs};
}

/** Read-only sampled SVG paint rectangles. Receipts must come from image_info for this snapshot. */
export async function sampledIllustrationLabelOverlaps(document:unknown,receipts:unknown[]){
 const doc=row(document),files=row(doc.files),elements=(Array.isArray(doc.elements)?doc.elements:[]).map(row);
 const infos=new Map<string,Row>(),duplicates=new Set<string>();
 for(const value of receipts){const info=row(value);if(typeof info.element_id==='string'){if(infos.has(info.element_id))duplicates.add(info.element_id);infos.set(info.element_id,info);}}
 const supported=new Set<string>(),unsupported:string[]=[];
 const projected=[];
 for(const e of elements){
  if(e.type!=='image'||e.isDeleted===true){projected.push(e);continue;}
  const id=String(e.id),info=infos.get(id),bounds=row(info?.svg_preview_alpha_bounds),b=bounds.normalized_bounds;
  const url=row(files[String(e.fileId)]).dataURL,scale=e.scale??[1,1];
  const nativeCrop=row(e.crop),hasCrop=e.crop!=null,receiptCrop=info?.visible_crop;
  const cropKeys=['x','y','width','height','naturalWidth','naturalHeight'];
  const cropValid=!hasCrop||(cropKeys.every(k=>typeof nativeCrop[k]==='number'&&Number.isFinite(nativeCrop[k]))&&Number(nativeCrop.x)>=0&&Number(nativeCrop.y)>=0&&Number(nativeCrop.width)>0&&Number(nativeCrop.height)>0&&Number(nativeCrop.naturalWidth)>0&&Number(nativeCrop.naturalHeight)>0&&nativeCrop.naturalWidth===info?.natural_width&&nativeCrop.naturalHeight===info?.natural_height&&Number(nativeCrop.x)+Number(nativeCrop.width)<=Number(nativeCrop.naturalWidth)+1e-6&&Number(nativeCrop.y)+Number(nativeCrop.height)<=Number(nativeCrop.naturalHeight)+1e-6&&Array.isArray(receiptCrop)&&receiptCrop.length===4&&['x','y','width','height'].every((key,i)=>typeof receiptCrop[i]==='number'&&Math.abs(Number(receiptCrop[i])-Number(nativeCrop[key]))<=1e-6));
  const valid=info&&info.file_id===e.fileId&&info.sha256===bounds.file_sha256&&!duplicates.has(id)&&bounds.scope==='uncropped_file_normalized_svg_preview'&&bounds.status==='available'&&typeof url==='string'&&url.startsWith('data:image/svg+xml;base64,')&&url.length<48*1024*1024&&cropValid&&Number(e.angle??0)===0&&Array.isArray(scale)&&scale.length===2&&scale.every(v=>v===1||v===-1)&&[e.x,e.y,e.width,e.height].every(v=>typeof v==='number'&&Number.isFinite(v))&&Number(e.width)>0&&Number(e.height)>0&&Array.isArray(b)&&b.length===4&&b.every(v=>typeof v==='number'&&Number.isFinite(v))&&b[0]>=0&&b[1]>=0&&b[2]>0&&b[3]>0&&b[0]+b[2]<=1+1e-9&&b[1]+b[3]<=1+1e-9;
  let matches=false;
  if(valid){try{const bytes=Uint8Array.from(atob((url as string).split(',')[1]),c=>c.charCodeAt(0));const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),v=>v.toString(16).padStart(2,'0')).join('');matches=hash===info.sha256;}catch{/* Unsupported encoding: retain viewport diagnostic. */}}
  if(!valid||!matches){unsupported.push(id);projected.push(e);continue;}
  const [bx,by,bw,bh]=b as number[],[sx,sy]=scale as number[];
  const crop=hasCrop?[Number(nativeCrop.x)/Number(nativeCrop.naturalWidth),Number(nativeCrop.y)/Number(nativeCrop.naturalHeight),Number(nativeCrop.width)/Number(nativeCrop.naturalWidth),Number(nativeCrop.height)/Number(nativeCrop.naturalHeight)]:[0,0,1,1];
  const left=Math.max(bx,crop[0]),top=Math.max(by,crop[1]),right=Math.min(bx+bw,crop[0]+crop[2]),bottom=Math.min(by+bh,crop[1]+crop[3]);
  if(right<=left||bottom<=top){unsupported.push(id);projected.push(e);continue;}
  const nx=(left-crop[0])/crop[2],ny=(top-crop[1])/crop[3],nw=(right-left)/crop[2],nh=(bottom-top)/crop[3];
  supported.add(id);projected.push({...e,x:Number(e.x)+(sx<0?1-nx-nw:nx)*Number(e.width),y:Number(e.y)+(sy<0?1-ny-nh:ny)*Number(e.height),width:nw*Number(e.width),height:nh*Number(e.height)});
 }
 const viewport=illustrationLabelOverlaps(document),sampled=illustrationLabelOverlaps({...doc,elements:projected});
 const globalViewport=allIllustrationTextOverlaps(document),globalSampled=allIllustrationTextOverlaps({...doc,elements:projected});
 return {method:'sampled_svg_alpha_rectangles_with_viewport_fallback',scope:'illustrations_and_their_owned_unbound_labels_only',supported_images:supported.size,unsupported_images:unsupported,complete:unsupported.length===0&&sampled.skipped===0&&sampled.omitted===0,viewport,sampled,all_image_text:{viewport:globalViewport,sampled:globalSampled,complete:unsupported.length===0&&globalSampled.skipped===0&&globalSampled.omitted===0},limitations:'Primary viewport/sampled fields check owned unbound labels. all_image_text checks every live image/text pair, including unrelated labels and connector captions, with bounded coverage; image/image and connector paths are not checked. Sampled padded alpha rectangles include backgrounds and decorations. Valid crops clip the full-source alpha rectangle and map it through verified native crop/flip geometry; this is conservative rectangle intersection, not a cropped pixel mask. Stale/malformed or alpha-disjoint crops use unsupported full-viewport fallback. This does not assess scientific correctness or exact glyph intersections.'};
}

/** Receipt identity must match the exact candidate reviewed by the user. */
export async function paintedCandidateFeedback(document:unknown,receipt:unknown,id:string,hash:string){
 const value=receipt&&typeof receipt==='object'?receipt as Record<string,unknown>:null;
 if(!value||value.proposal_id!==id||value.document_sha256!==hash||!Array.isArray(value.infos))return {available:false as const};
 const review=await sampledIllustrationLabelOverlaps(document,value.infos);
 return {available:true as const,proposal_id:id,document_sha256:hash,...review};
}
