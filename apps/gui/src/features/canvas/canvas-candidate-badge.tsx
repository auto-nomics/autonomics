import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
export function CanvasCandidateBadge(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy}=props,elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted),matches=elements.filter(e=>e.id===focusedId&&e.type==='image'),image=matches.length===1?matches[0]:undefined;
 const owner=typeof image?.customData?.semanticBadgeAnchorId==='string'?image.customData.semanticBadgeAnchorId:null,key=['badge-ownership',image?.id??''];
 const [draft,setDraft,reset]=useCandidateDraft(candidate,key,{anchor:'',meaning:typeof image?.customData?.badgeMeaning==='string'?image.customData.badgeMeaning:''});
 if(!image||!candidate.document_sha256||image.customData?.semanticAnchorId!=null||image.customData?.inset!=null)return null;
 const anchors=elements.filter(e=>['rectangle','ellipse','diamond'].includes(e.type)&&e.frameId===image.frameId&&typeof e.customData?.visualAssetElementId==='string'&&!e.locked&&e.angle===0);
 const labelFor=(anchor:typeof anchors[number])=>{const ids=anchor.customData?.visualLabelIds,label=Array.isArray(ids)?elements.find(e=>e.type==='text'&&ids.includes(e.id)):undefined;const text=String(label&&'originalText' in label?label.originalText:anchor.customData?.scientificId??anchor.id).replace(/\s+/g,' ');return text.length>200?text.slice(0,200)+'…':text;};
 const currentOwner=elements.find(e=>e.id===owner);
 const valid=!image.locked&&(owner?elements.some(e=>e.id===owner&&!e.locked):image.angle===0&&!(image.groupIds?.length)&&anchors.some(e=>e.id===draft.anchor)&&draft.meaning.trim().length>0&&new TextEncoder().encode(draft.meaning).length<=512);
 const operations=owner?[{op:'detach_illustration_badge',id:image.id,anchor_id:owner}]:[{op:'attach_illustration_badge',id:image.id,anchor_id:draft.anchor,meaning:draft.meaning}];
 return <CanvasCandidateRevision {...props} draftKey={key} onCommitted={reset} label="候选附属标记归属" title={owner?'解除附属标记归属':'关联附属标记'} summary={owner?'解除候选附属标记归属':'关联候选附属标记'} submitLabel={owner?'预览解除标记归属':'预览标记关联'} valid={valid} operations={operations}>
 {owner?<><p>当前归属：{currentOwner?labelFor(currentOwner):owner}</p><p>已记录含义：{String(image.customData?.badgeMeaning??'未记录')}</p><p>解除后保留图形、位置、来源和已记录含义。</p></>:<>
 <p>明确选择此图形所说明的组件，保留主插图与来源。标记加入组件组并随整体移动；关联记录不验证生物状态，须核对当前图意。</p>
 <label>归属组件<select aria-label="附属标记归属组件" value={draft.anchor} disabled={busy||image.locked} onChange={event=>setDraft(current=>({...current,anchor:event.target.value}))} className="ml-2 w-full rounded border bg-background p-1"><option value="">选择组件</option>{anchors.map(anchor=><option key={anchor.id} value={anchor.id}>{labelFor(anchor)}</option>)}</select></label>
 <label>简短标记含义<input aria-label="附属标记含义" maxLength={160} value={draft.meaning} disabled={busy||image.locked} onChange={event=>setDraft(current=>({...current,meaning:event.target.value}))} className="mt-1 w-full rounded border bg-background p-1"/></label>
 </>}
 </CanvasCandidateRevision>;
}
