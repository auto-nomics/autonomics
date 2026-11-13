import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
const sides=[['left','左'],['right','右'],['top','上'],['bottom','下']] as const;
export function CanvasCandidatePorts(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy}=props,elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted),selected=elements.find(e=>e.id===focusedId),arrow=elements.find(e=>e.id===(selected?.type==='text'?selected.containerId:selected?.id)),key=['connector-ports',arrow?.id??''];
 const [draft,setDraft,reset]=useCandidateDraft(candidate,key,{start:'',end:''});
 if(!arrow||arrow.type!=='arrow'||!arrow.frameId||!candidate.document_sha256)return null;
 const supported=!arrow.locked&&arrow.angle===0&&arrow.roughness===0&&!arrow.roundness&&!('elbowed' in arrow&&arrow.elbowed),valid=supported&&sides.some(([s])=>s===draft.start)&&sides.some(([s])=>s===draft.end);
 return <CanvasCandidateRevision {...props} draftKey={key} onCommitted={reset} label="候选连线端口重绑定" title="选择端口并重建直角路径" summary="按指定端口重建局部直角连线" submitLabel="预览连线端口修复" valid={valid} operations={[{op:'rebind_connector_ports',id:arrow.id,start_side:draft.start,end_side:draft.end}]}>
 <p>明确选择两端的居中端口，修复偏移端点或斜线。保留绑定对象、间距、方向、说明位置与原文，在原分面中避让当前对象。路径会改变，请检查子候选后再采用。</p>
 {!supported&&<p role="status">仅支持未锁定、无旋转、无圆角或粗糙效果的原生局部连线。</p>}
 {(['start','end'] as const).map(field=><label key={field} className="block">{field==='start'?'起点端口':'终点端口'}<select aria-label={field==='start'?'连线起点端口':'连线终点端口'} value={draft[field]} disabled={busy||!supported} onChange={event=>setDraft(old=>({...old,[field]:event.target.value}))} className="ml-2 rounded border bg-background"><option value="">选择端口</option>{sides.map(([value,label])=><option key={value} value={value}>{label}</option>)}</select></label>)}
 </CanvasCandidateRevision>;
}
