import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
export function CanvasCandidateLabelOwner(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy}=props,elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted),label=elements.find(e=>e.id===focusedId&&e.type==='text'),key=['label-owner',label?.id??''];
 const [anchorId,setAnchorId,reset]=useCandidateDraft(candidate,key,'');
 if(!label||label.type!=='text'||!candidate.document_sha256||label.customData?.annotationKind!=null||label.customData?.inset!=null||elements.some(e=>Array.isArray(e.customData?.visualLabelIds)&&e.customData.visualLabelIds.includes(label.id)))return null;
 const container=elements.find(e=>e.id===label.containerId),anchors=elements.filter(e=>['rectangle','ellipse','diamond'].includes(e.type)&&e.frameId===label.frameId&&!e.locked&&e.angle===0&&typeof e.customData?.visualAssetElementId==='string'&&(!container||container.id===e.id)&&elements.some(image=>image.id===e.customData?.visualAssetElementId&&image.customData?.semanticAnchorId===e.id));
 if(!anchors.length)return null;
 const valid=!label.locked&&label.angle===0&&anchors.some(e=>e.id===anchorId);
 return <CanvasCandidateRevision {...props} draftKey={key} onCommitted={reset} label="候选文字归属组件" title="关联文字到素材组件" summary="将已有标签加入素材组件" submitLabel="预览标签归属" valid={valid} operations={[{op:'attach_illustration_label',id:label.id,anchor_id:anchorId}]}>
 <p>明确选择标签所说明的组件。保留原文、字体、位置和元数据，将标签加入组件组并随整体移动；不从其他存活容器或分组中自动移走标签。</p>
 <label>归属组件<select aria-label="标签归属组件" value={anchorId} disabled={busy||label.locked} onChange={event=>setAnchorId(event.target.value)} className="ml-2 w-full rounded border bg-background"><option value="">选择组件</option>{anchors.map(e=><option key={e.id} value={e.id}>{String(e.customData?.scientificId??e.id)} · {e.id}</option>)}</select></label>
 </CanvasCandidateRevision>;
}
