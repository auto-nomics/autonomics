import {Button} from "@/components/ui/button";
import {useEffect,useState} from 'react';
import {gatewayRequest} from '@/gateway';
import {sampledIllustrationLabelOverlaps} from './canvas-illustration-overlaps';
type Review=Awaited<ReturnType<typeof sampledIllustrationLabelOverlaps>>;
export function CanvasPaintedReview({path,id,hash,revision,document,onFocus}:{path:string;id:string;hash?:string;revision:number;document:unknown;onFocus:(id:string)=>void}){
 const [state,setState]=useState<{key:string;review?:Review;failed?:boolean}>();
 const key=`${id}:${hash}:${revision}`;
 useEffect(()=>{
  if(!hash)return;
  let active=true;const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),15000);
  void gatewayRequest('GET',`${path}/${encodeURIComponent(id)}/image-bounds?expected_revision=${revision}&document_sha256=${encodeURIComponent(hash)}`,undefined,{signal:controller.signal}).then(async value=>{
   if(value?.proposal_id!==id||value?.document_sha256!==hash)throw new Error('candidate changed');
   const review=await sampledIllustrationLabelOverlaps(document,value.infos??[]);
   if(active)setState({key,review});
  }).catch(()=>{if(active)setState({key,failed:true});}).finally(()=>clearTimeout(timeout));
  return()=>{active=false;clearTimeout(timeout);controller.abort();};
 },[path,id,hash,revision,document,key]);
 const current=state?.key===key?state:undefined;
 return <section aria-label="图形透明边界检查" className="shrink-0 border-b p-2 text-xs">
 {!hash||current?.failed?<p>图形边界检查不可用，请结合图片视口检查与预览核对。</p>:!current?.review?<p>正在检查图形透明边界…</p>:<><p>采样图形与标签重叠 {current.review.sampled.count} 对 · 图片视口重叠 {current.review.viewport.count} 对</p><p>全图插图与文字：采样重叠 {current.review.all_image_text.sampled.count} 对 · 视口重叠 {current.review.all_image_text.viewport.count} 对；检查 {current.review.all_image_text.sampled.checked} 对，跳过 {current.review.all_image_text.sampled.skipped} 对，超限 {current.review.all_image_text.sampled.omitted} 对。</p><p>已识别 {current.review.supported_images} 个素材；{current.review.unsupported_images.length} 个素材使用完整视口回退。已检查 {current.review.sampled.checked} 对，跳过 {current.review.sampled.skipped} 对，超限 {current.review.sampled.omitted} 对。首项仅检查所属独立标签；全图项包含其他文字与连线说明，不检查插图之间或连线路径的碰撞。边界包含背景和装饰，属于预览采样估计；不代表精确字形碰撞或科学含义验证。</p><details aria-label="全图插图文字重叠对象"><summary className="cursor-pointer">定位重叠对象</summary><div className="max-h-40 overflow-auto">{current.review.all_image_text.viewport.pairs.slice(0,50).map(pair=>{
 const painted=current.review!.all_image_text.sampled.pairs.some(p=>p.image_id===pair.image_id&&p.label_id===pair.label_id);
 return <p key={`${pair.image_id}:${pair.label_id}`}>{painted?'采样范围相交，需核对预览':'仅视口相交，需核对透明留白'} <Button size="sm" variant="outline" aria-label={`全图定位插图 ${pair.image_id}`} onClick={()=>onFocus(pair.image_id)}>定位插图</Button> <Button size="sm" variant="outline" aria-label={`全图定位文字 ${pair.label_id}`} onClick={()=>onFocus(pair.label_id)}>定位文字</Button></p>;
 })}{current.review.all_image_text.viewport.pairs.length>50&&<p>仅列出前50对，请结合预览继续核对。</p>}</div></details></>}
 </section>;
}
