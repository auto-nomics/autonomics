import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
export function CanvasCandidateBinding(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy}=props,elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted),anchor=elements.find(e=>e.id===focusedId&&['rectangle','ellipse','diamond'].includes(e.type)),key=['illustration-binding',anchor?.id??''];
 const [draft,setDraft,reset]=useCandidateDraft(candidate,key,{primary:'',labels:[] as string[],members:{} as Record<string,string>});
 const current=elements.find(e=>e.id===anchor?.customData?.visualAssetElementId);
 if(!anchor||!candidate.document_sha256||current?.customData?.semanticAnchorId===anchor.id||anchor.customData?.inset!=null||anchor.customData?.annotationKind!=null)return null;
 const images=elements.filter(e=>e.type==='image'&&e.frameId===anchor.frameId&&!e.locked&&e.angle===0&&e.customData?.semanticAnchorId==null&&e.customData?.semanticBadgeAnchorId==null&&e.customData?.inset==null&&e.customData?.annotationKind==null);
 const labels=elements.filter(e=>e.type==='text'&&e.frameId===anchor.frameId&&!e.locked&&e.angle===0&&(!e.containerId||e.containerId===anchor.id||!elements.some(v=>v.id===e.containerId)));
 const members=Object.entries(draft.members).map(([id,meaning])=>({id,meaning}));
 const valid=images.length<=200&&labels.length<=200&&!anchor.locked&&anchor.angle===0&&images.some(e=>e.id===draft.primary)&&draft.labels.length<=32&&draft.labels.every(id=>labels.some(e=>e.id===id))&&members.length<=32&&members.every(m=>m.id!==draft.primary&&images.some(e=>e.id===m.id)&&m.meaning.trim().length>0&&new TextEncoder().encode(m.meaning).length<=512);
 const describe=(e:typeof elements[number])=>e.type==='text'?String(('originalText' in e?e.originalText:'')||('text' in e?e.text:'')||e.id).slice(0,160):`${String(e.customData?.asset?.name??'图片')} · ${e.id}`;
 return <CanvasCandidateRevision {...props} draftKey={key} onCommitted={reset} label="候选素材组件绑定" title="绑定已有素材与标签" summary="绑定已放置的素材组件" submitLabel="预览组件绑定" valid={valid} operations={[{op:'bind_illustration',anchor_id:anchor.id,primary_id:draft.primary,label_ids:draft.labels,members}]}>
 <p>明确选择主图、完整标签和附属图元，并填写各附属图元的含义。保留位置、尺寸和来源；已有分组须完整包含在本次选择中。此操作不验证生物身份或化学计量，会清除旧贴合记录，之后须按当前 SVG 重新贴合。</p>
 {images.length>200||labels.length>200?<p>对象过多，请缩小图稿范围或通过 Agent 指定完整对象 ID。</p>:<>
 <label>主图<select aria-label="绑定组件主图" value={draft.primary} disabled={busy} onChange={event=>{const primary=event.target.value;setDraft(old=>{const members={...old.members};delete members[primary];return {...old,primary,members};});}} className="w-full rounded border bg-background"><option value="">选择已有图片</option>{images.map(e=><option key={e.id} value={e.id}>{describe(e)}</option>)}</select></label>
 <fieldset disabled={busy}><legend>组件完整标签</legend>{labels.map(e=><label key={e.id} className="block"><input type="checkbox" aria-label={`绑定标签 ${e.id}`} checked={draft.labels.includes(e.id)} onChange={event=>setDraft(old=>({...old,labels:event.target.checked?[...old.labels,e.id]:old.labels.filter(id=>id!==e.id)}))}/>{describe(e)}</label>)}</fieldset>
 <fieldset disabled={busy}><legend>附属图元及明确含义</legend>{images.filter(e=>e.id!==draft.primary).map(e=><div key={e.id}><label><input type="checkbox" aria-label={`绑定附属图元 ${e.id}`} checked={Object.hasOwn(draft.members,e.id)} onChange={event=>setDraft(old=>{const members={...old.members};if(event.target.checked)members[e.id]='';else delete members[e.id];return {...old,members};})}/>{describe(e)}</label>{Object.hasOwn(draft.members,e.id)&&<input aria-label={`附属图元含义 ${e.id}`} value={draft.members[e.id]} maxLength={160} disabled={busy} onChange={event=>setDraft(old=>({...old,members:{...old.members,[e.id]:event.target.value}}))} className="w-full rounded border bg-background"/>}</div>)}</fieldset>
 </>}
 </CanvasCandidateRevision>;
}
