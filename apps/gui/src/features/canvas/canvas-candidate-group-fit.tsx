import {useEffect,useRef,useState} from 'react';
import {gatewayRequest} from '@/gateway';
import {Button} from '@/components/ui/button';
import {svgPreviewFitBounds} from './canvas-svg-preview-fit';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
type Sample={id:string;file_id:string;normalized_bounds:[number,number,number,number]};
export function CanvasCandidateGroupFit(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy,onBusy,isDirty}=props,elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted),selected=elements.find(e=>e.id===focusedId),anchorId=selected?.type==='image'?(selected.customData?.semanticAnchorId??selected.customData?.semanticBadgeAnchorId):selected?.id,anchor=elements.find(e=>e.id===anchorId),primary=anchor?.customData?.visualAssetElementId,badges=anchor?.customData?.visualBadgeIds;
 const [samples,setSamples]=useState<Sample[]|null>(null),[error,setError]=useState(''),active=useRef(true);useEffect(()=>{active.current=true;return()=>{active.current=false;};},[]);
 if(!anchor||typeof primary!=='string'||!Array.isArray(badges)||!badges.length||badges.length>32||!badges.every(id=>typeof id==='string')||!candidate.document_sha256)return null;
 return <CanvasCandidateRevision {...props} draftKey={['group-fit',anchor.id]} onCommitted={()=>setSamples(null)} label="候选组合SVG采样贴合" title="按整个组合的可见范围贴合" summary="按所有绑定SVG可见范围贴合组件" submitLabel="预览组合重新贴合" valid={samples!==null} operations={samples?[{op:'fit_illustration_group',anchor_id:anchor.id,samples}]:[]}>
 <p>采样主图与全部附属图元，使用可见外框的并集更新连接锚点；保留每张图片、标签和来源。背景及标记也会计入，并非分子结构识别。旧连线端口或几何不受支持时，本次修改会被拒绝。</p>
 <Button type="button" size="sm" variant="outline" disabled={busy} onClick={async()=>{onBusy(true);setError('');setSamples(null);try{if(isDirty())throw Error('请先保存或重新加载正式图稿。');const path=`/visualization/projects/${encodeURIComponent(props.projectId)}/figures/${encodeURIComponent(props.canvasId)}/proposals/${encodeURIComponent(candidate.id)}/image-bounds?expected_revision=${candidate.base_revision}&document_sha256=${encodeURIComponent(candidate.document_sha256??'')}`,proof=await gatewayRequest('GET',path),next:Sample[]=[];for(const id of [primary,...badges]){const image=elements.find(e=>e.id===id);if(!image||image.type!=='image'||typeof image.fileId!=='string')throw Error('组合图片缺失，请核对归属。');next.push({id,file_id:image.fileId,normalized_bounds:await svgPreviewFitBounds(candidate,id,proof)});}if(active.current&&!isDirty())setSamples(next);}catch(cause){if(active.current)setError(String(cause));}finally{onBusy(false);}}}>读取组合内全部 SVG 可见范围</Button>
 {samples&&<p role="status">已核对 {samples.length} 张当前 SVG 文件；提交后请检查完整候选。</p>}{error&&<p role="alert">{error}</p>}
 </CanvasCandidateRevision>;
}
