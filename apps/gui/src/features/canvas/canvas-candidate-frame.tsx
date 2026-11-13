import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
export function CanvasCandidateFrame(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy}=props;
 const matches=(candidate.document.elements??[]).filter(e=>!e.isDeleted&&e.id===focusedId&&e.type==='frame');
 const frame=matches.length===1?matches[0]:undefined,draftKey=['frame-fit',frame?.id??''];
 const [padding,setPadding,reset]=useCandidateDraft(candidate,draftKey,'24');
 if(!frame||!candidate.document_sha256)return null;
 const value=Number(padding),unsupported=frame.locked||frame.angle!==0||Boolean(frame.frameId);
 const valid=!unsupported&&padding.trim().length>0&&Number.isFinite(value)&&value>=8&&value<=128;
 return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={reset} label="收紧候选分面" title="按现有内容收紧分面" summary="按现有内容收紧分面边界" submitLabel="预览收紧分面" valid={valid} operations={[{op:'fit_frame',id:frame.id,padding:value}]}>
  <p>仅收紧右侧和底部留白，保留分面原点、所有内容、标签和连线；不重新排列组件。检查子候选预览后再采用。</p>
  {unsupported&&<p role="status">请先解锁并取消旋转；嵌套分面暂不支持此操作。</p>}
  <label>内容边距<input aria-label="候选分面内容边距" type="number" min="8" max="128" step="any" value={padding} disabled={busy||unsupported} onChange={event=>setPadding(event.target.value)} className="ml-2 w-24 rounded border bg-background p-1"/></label>
 </CanvasCandidateRevision>;
}
