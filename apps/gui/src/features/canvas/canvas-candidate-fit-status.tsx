import {illustrationFitStatus} from './canvas-selection-context';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
import {useState,useRef,useEffect} from 'react';
import {gatewayRequest} from '@/gateway';
import {svgPreviewFitBounds} from './canvas-svg-preview-fit';
import {Button} from '@/components/ui/button';
export function CanvasCandidateFitStatus(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy,onBusy,isDirty}=props;
 const [bounds,setBounds]=useState<[number,number,number,number]|null>(null),[error,setError]=useState(''),active=useRef(true);
 useEffect(()=>{active.current=true;return()=>{active.current=false;};},[]);
 const status=focusedId?illustrationFitStatus(candidate.document.elements??[],focusedId):null;
 if(!status)return null;
 const messages={not_recorded:'此插图尚无贴合记录，请核对可见范围与连线端点。',stale_file:'插图文件已改变，原贴合记录已过期。请按当前 SVG 重新贴合，并核对标签与连线间距。',stale_geometry:'插图位置、尺寸或变换已改变，原贴合记录已过期。请重新核对贴合与间距。',invalid_record:'贴合记录不完整，请重新贴合并检查预览。',current_snapshot:'贴合记录对应当前插图文件与视口；仍须检查预览中的遮挡和科学表达。'};
 return <><p role="status" aria-label="候选插图贴合状态" className="shrink-0 border-t p-2 text-xs text-muted-foreground">{messages[status.status]}</p>
 <CanvasCandidateRevision {...props} draftKey={['svg-preview-fit',status.image_id]} onCommitted={()=>setBounds(null)} label="候选SVG采样贴合" title="按当前 SVG 可见范围重新贴合" summary="按当前SVG预览采样范围贴合连接锚点" submitLabel="预览重新贴合" valid={bounds!==null} operations={bounds?[{op:'fit_illustration_anchor',id:status.image_id,file_id:status.current_file_id,normalized_bounds:bounds}]:[]}>
 <p>读取后端非零透明度范围（含一个预览像素边距），再生成待审候选。背景与装饰也会计入；这不是精确轮廓或科学组件识别。请检查连线、标签和完整图稿。</p>
 <Button type="button" size="sm" variant="outline" disabled={busy} onClick={async()=>{onBusy(true);setError('');setBounds(null);try{if(isDirty())throw Error('请先保存或重新加载正式图稿。');const path=`/visualization/projects/${encodeURIComponent(props.projectId)}/figures/${encodeURIComponent(props.canvasId)}/proposals/${encodeURIComponent(candidate.id)}/image-bounds?expected_revision=${candidate.base_revision}&document_sha256=${encodeURIComponent(candidate.document_sha256??'')}`;const proof=await gatewayRequest('GET',path),measured=await svgPreviewFitBounds(candidate,status.image_id,proof);if(active.current&&!isDirty())setBounds(measured);}catch(cause){if(active.current)setError(String(cause));}finally{onBusy(false);}}}>读取当前 SVG 可见范围</Button>
 {bounds&&<p role="status">已读取当前文件范围，提交后请检查候选预览。</p>}{error&&<p role="alert">{error}</p>}
 </CanvasCandidateRevision></>;
}
