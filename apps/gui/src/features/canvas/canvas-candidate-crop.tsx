import {useEffect,useRef,useState} from 'react';
import {gatewayRequest} from '@/gateway';
import {Button} from '@/components/ui/button';
import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
type Point={x:number;y:number};
export function CanvasCandidateCrop(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy,onBusy,isDirty}=props,elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted),selected=elements.find(e=>e.id===focusedId),image=selected?.type==='image'?selected:elements.find(e=>e.id===selected?.customData?.visualAssetElementId&&e.type==='image');
 const draftKey=['image-crop',image?.id??''];
 const crop=image?.type==='image'?image.crop:null;
 const [draft,setDraft,reset]=useCandidateDraft(candidate,draftKey,{x:String(crop?crop.x/crop.naturalWidth*100:0),y:String(crop?crop.y/crop.naturalHeight*100:0),width:String(crop?crop.width/crop.naturalWidth*100:100),height:String(crop?crop.height/crop.naturalHeight*100:100),fit:false});
 const [source,setSource]=useState<{width:number;height:number}|null>(null),[error,setError]=useState('');const start=useRef<Point|null>(null),active=useRef(true);useEffect(()=>{active.current=true;return()=>{active.current=false;};},[]);
 if(!image||image.type!=='image'||!image.fileId||!candidate.document_sha256)return null;
 const owner=elements.find(e=>e.id===image.customData?.semanticAnchorId),single=Boolean(owner&&(!Array.isArray(owner.customData?.visualBadgeIds)||owner.customData.visualBadgeIds.length===0));
 const unsupported=image.locked||image.angle!==0||image.customData?.inset!=null||image.customData?.annotationKind!=null;
 const numbers=[draft.x,draft.y,draft.width,draft.height].map(Number),[x,y,width,height]=numbers.map(n=>n/100);
 const valid=Boolean(source)&&!unsupported&&[draft.x,draft.y,draft.width,draft.height].every(v=>v.trim())&&numbers.every(Number.isFinite)&&x>=0&&y>=0&&width>0&&height>0&&x+width<=1+1e-9&&y+height<=1+1e-9&&(!draft.fit||single);
 const operations:Record<string,unknown>[]=[{op:'crop_image',id:image.id,file_id:image.fileId,natural_width:source?.width,natural_height:source?.height,normalized_crop:[x,y,width,height]}];
 if(draft.fit&&owner)operations.push({op:'fit_illustration_anchor',id:owner.id,file_id:image.fileId,normalized_bounds:[x,y,width,height]});
 const file=candidate.document.files?.[image.fileId];
 const point=(event:React.PointerEvent<HTMLDivElement>)=>{const r=event.currentTarget.getBoundingClientRect();return {x:Math.max(0,Math.min(1,(event.clientX-r.left)/r.width)),y:Math.max(0,Math.min(1,(event.clientY-r.top)/r.height))};};
 const select=(a:Point,b:Point)=>setDraft(current=>({...current,x:String(Math.min(a.x,b.x)*100),y:String(Math.min(a.y,b.y)*100),width:String(Math.abs(a.x-b.x)*100),height:String(Math.abs(a.y-b.y)*100)}));
 return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={()=>{reset();setSource(null);}} label="候选图像裁剪" title="裁剪当前图像" summary="按原图区域裁剪图像" submitLabel="预览图像裁剪" valid={Boolean(valid)} operations={operations}>
 <p>拖选原图区域或填写百分比。保留区域中的像素位置与比例，文件和来源保持；不会把合集自动识别为分子。组件的旧贴合记录会失效。</p>
 <Button type="button" size="sm" variant="outline" disabled={busy||unsupported} onClick={async()=>{onBusy(true);setError('');try{if(isDirty())throw Error('请先保存或重新加载正式图稿。');const proof=await gatewayRequest('GET',`/visualization/projects/${encodeURIComponent(props.projectId)}/figures/${encodeURIComponent(props.canvasId)}/proposals/${encodeURIComponent(candidate.id)}/image-bounds?expected_revision=${candidate.base_revision}&document_sha256=${encodeURIComponent(candidate.document_sha256??'')}`) as {proposal_id:string;document_sha256:string;infos:{element_id:string;file_id:string;natural_width:number;natural_height:number}[]};const info=proof.infos.find(e=>e.element_id===image.id&&e.file_id===image.fileId);if(proof.proposal_id!==candidate.id||proof.document_sha256!==candidate.document_sha256||!info||![info.natural_width,info.natural_height].every(n=>Number.isFinite(n)&&n>0))throw Error('当前文件原图尺寸不可用，请核对候选。');if(active.current)setSource({width:info.natural_width,height:info.natural_height});}catch(cause){if(active.current)setError(String(cause));}finally{onBusy(false);}}}>读取原图尺寸</Button>
 {source&&file&&<div role="application" aria-label="原图裁剪区域" className="relative w-full touch-none overflow-hidden bg-white" style={{aspectRatio:source.width/source.height}} onPointerDown={event=>{if(busy||event.button!==0)return;event.currentTarget.setPointerCapture(event.pointerId);start.current=point(event);}} onPointerMove={event=>{if(start.current&&!busy)select(start.current,point(event));}} onPointerUp={event=>{if(start.current&&!busy)select(start.current,point(event));start.current=null;}} onPointerCancel={()=>{start.current=null;}}><img src={file.dataURL} alt="裁剪用完整原图" draggable={false} className="pointer-events-none absolute inset-0 h-full w-full"/><div className="pointer-events-none absolute border-2 border-red-500 bg-red-500/10" style={{left:`${numbers[0]}%`,top:`${numbers[1]}%`,width:`${numbers[2]}%`,height:`${numbers[3]}%`}}/></div>}
 <div className="grid grid-cols-2 gap-2">{(['x','y','width','height'] as const).map((key,i)=><label key={key}>{['左侧','顶部','宽度','高度'][i]} %<input aria-label={`裁剪${key}百分比`} type="number" min="0" max="100" step="any" className="w-full rounded border bg-background p-1" disabled={busy||unsupported} value={draft[key]} onChange={event=>setDraft(current=>({...current,[key]:event.target.value}))}/></label>)}</div>
 {single&&<label className="block"><input type="checkbox" disabled={busy||unsupported} checked={draft.fit} onChange={event=>setDraft(current=>({...current,fit:event.target.checked}))}/>同时将单主图组件贴合到裁剪矩形</label>}
 <p>矩形贴合包含透明边缘，不是可见像素或科学身份识别。多图元组件需重新核对整个组合贴合。已裁剪图片只能进一步缩小；恢复边缘请返回原候选。</p>
 {unsupported&&<p role="status">锁定、旋转、批注和放大图请使用其专用编辑入口。</p>}{error&&<p role="alert">{error}</p>}
 </CanvasCandidateRevision>;
}
