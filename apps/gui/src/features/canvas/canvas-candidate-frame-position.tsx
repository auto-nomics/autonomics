import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
export function CanvasCandidateFramePosition(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy}=props,frames=(candidate.document.elements??[]).filter(e=>!e.isDeleted&&e.id===focusedId&&e.type==='frame');
 const frame=frames.length===1?frames[0]:undefined,key=['frame-position',frame?.id??''];
 const [draft,setDraft,reset]=useCandidateDraft(candidate,key,{x:String(frame?.x??0),y:String(frame?.y??0)});
 if(!frame||!candidate.document_sha256)return null;
 const x=Number(draft.x),y=Number(draft.y),dx=x-frame.x,dy=y-frame.y,unsupported=frame.locked||frame.angle!==0||Boolean(frame.frameId);
 const valid=!unsupported&&[draft.x,draft.y].every(v=>v.trim().length>0)&&[x,y,dx,dy].every(v=>Number.isFinite(v)&&Math.abs(v)<=1e6)&&(dx!==0||dy!==0);
 return <CanvasCandidateRevision {...props} draftKey={key} onCommitted={reset} label="候选分面位置" title="整体移动分面" summary="手动整体移动候选分面" submitLabel="预览分面移动" valid={valid} operations={[{op:'translate',element_ids:[frame.id],dx,dy}]}>
  <p>按画布坐标整体移动分面及内容，保留文字、素材和科学关系。支持的跨分面原生连线会重新规划；请检查子候选的路径和说明位置后再采用。</p>
  {unsupported&&<p role="status">请先解锁并取消旋转；此入口暂不支持嵌套分面。</p>}
  {(['x','y'] as const).map(axis=><label key={axis} className="mr-2">{axis.toUpperCase()}<input aria-label={`候选分面${axis.toUpperCase()}`} type="number" min="-1000000" max="1000000" step="any" disabled={busy||unsupported} value={draft[axis]} onChange={event=>setDraft(current=>({...current,[axis]:event.target.value}))} className="ml-2 w-28 rounded border bg-background p-1"/></label>)}
 </CanvasCandidateRevision>;
}
