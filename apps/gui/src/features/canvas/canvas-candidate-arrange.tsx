import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
const modes=[['left','左对齐'],['center','水平居中'],['right','右对齐'],['top','顶部对齐'],['middle','垂直居中'],['bottom','底部对齐'],['distribute_horizontal','水平等间距'],['distribute_vertical','垂直等间距'],['fit_contents','按内容收紧边界'],['pack_horizontal','横向紧凑排列'],['pack_vertical','纵向紧凑排列']] as const;
export function CanvasCandidateArrange(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,busy}=props,draftKey=['frame-arrangement'];
 const frames=(candidate.document.elements??[]).filter(e=>!e.isDeleted&&e.type==='frame'&&!e.frameId);
 const [draft,setDraft,reset]=useCandidateDraft(candidate,draftKey,{ids:[] as string[],mode:'top',padding:'24',gap:'144',wrapCaptions:false});
 if(frames.length<2||!candidate.document_sha256)return null;
 const fitting=draft.mode==='fit_contents',distributing=draft.mode==='distribute_horizontal'||draft.mode==='distribute_vertical',padding=Number(draft.padding);
 const packing=draft.mode==='pack_horizontal'||draft.mode==='pack_vertical',gap=Number(draft.gap);
 const elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted);
 const selectedCrossings=elements.filter(e=>e.type==='arrow'&&e.customData?.crossPanel===true&&[e.startBinding?.elementId,e.endBinding?.elementId].every(id=>{const node=elements.find(n=>n.id===id);return node?.frameId&&draft.ids.includes(node.frameId);}));
 const captions=elements.flatMap(e=>e.type==='text'&&selectedCrossings.some(arrow=>arrow.id===e.containerId)?[e]:[]);
 const wrap=packing&&draft.mode==='pack_horizontal'&&draft.wrapCaptions;
 const wrapSupported=captions.every(e=>!e.locked&&e.angle===0&&e.fontFamily===9)&&selectedCrossings.every(e=>!e.locked&&e.angle===0);
 const captionOperations=wrap?captions.map(e=>({op:'set_wrapped_text',id:e.id,text:e.originalText??e.text,font_size:e.fontSize,max_width:gap-24})):[];
 const edit=packing?{operations:[...captionOperations,{op:'pack_frames',element_ids:draft.ids,vertical:draft.mode==='pack_vertical',gap}]}:fitting?{operations:draft.ids.map(id=>({op:'fit_frame',id,padding}))}:{arrangement:{element_ids:draft.ids,mode:draft.mode}};
 const valid=(!wrap||(wrapSupported&&gap>=104&&captionOperations.length<100))&&(!packing||(draft.gap.trim().length>0&&Number.isFinite(gap)&&gap>=8&&gap<=512))&&(!fitting||(draft.padding.trim().length>0&&Number.isFinite(padding)&&padding>=8&&padding<=128))&&draft.ids.length>=(fitting?1:distributing?3:2)&&draft.ids.length<=100&&draft.ids.every(id=>frames.some(e=>e.id===id&&!e.locked&&e.angle===0))&&modes.some(([mode])=>mode===draft.mode);
 return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={reset} label="候选分面对齐分布" title="批量调整分面布局" summary={fitting?"按统一边距收紧所选分面":"手动对齐或分布候选分面"} submitLabel="预览分面布局" valid={valid} {...edit}>
  <p>对齐至少选择两个分面，等间距分布至少选择三个分面，连同内容整体移动；收紧边界不移动内容，仅减小分面右侧和底部留白。保留文字、素材和科学关系。此操作不推断生物学顺序；请在子候选中核对交叠与跨分面连线，再决定是否采用。</p>
  {distributing&&draft.ids.length<3&&<p role="status">等间距分布需要至少三个分面。</p>}
  <fieldset disabled={busy}>{frames.map(frame=><label key={frame.id} className="block"><input type="checkbox" aria-label={`布局分面 ${frame.id}`} disabled={Boolean(frame.locked)||frame.angle!==0} checked={draft.ids.includes(frame.id)} onChange={event=>setDraft(current=>({...current,ids:event.target.checked?[...current.ids,frame.id]:current.ids.filter(id=>id!==frame.id)}))}/>{('name' in frame&&typeof frame.name==='string'?frame.name:'')||frame.id}</label>)}</fieldset>
  <label>布局方式<select aria-label="候选分面布局方式" value={draft.mode} disabled={busy} onChange={event=>setDraft(current=>({...current,mode:event.target.value}))} className="ml-2 rounded border bg-background p-1">{modes.map(([mode,label])=><option key={mode} value={mode}>{label}</option>)}</select></label>
  {packing&&<><p>按当前空间顺序排列，第一处分面固定，另一方向坐标保持。完整组件一起移动，支持的跨分面连线重新路由；不会自动避开未选分面，请核对子候选。</p><label>分面间距<input aria-label="紧凑排列分面间距" type="number" min="8" max="512" step="any" disabled={busy} value={draft.gap} onChange={event=>setDraft(current=>({...current,gap:event.target.value}))} className="ml-2 w-24 rounded border bg-background p-1"/></label></>}
  {packing&&draft.mode==='pack_horizontal'&&<><label className="block"><input type="checkbox" aria-label="按间距换行跨分面图注" disabled={busy||!wrapSupported} checked={draft.wrapCaptions} onChange={event=>setDraft(current=>({...current,wrapCaptions:event.target.checked}))}/>按间距换行跨分面图注（{captions.length} 条）</label><p>可选：保留完整原文、字体和字号，将所选分面之间的关系图注按间距减去24单位换行；间距至少104（图注宽度至少80）。文字框会变高，请核对遮挡。其他图注保持。</p>{!wrapSupported&&<p role="status">部分关系或图注锁定、旋转或使用不支持的字体，无法批量换行。</p>}</>}
  {fitting&&<label>统一内容边距<input aria-label="所选分面统一内容边距" type="number" min="8" max="128" step="any" disabled={busy} value={draft.padding} onChange={event=>setDraft(current=>({...current,padding:event.target.value}))} className="ml-2 w-24 rounded border bg-background p-1"/></label>}
 </CanvasCandidateRevision>;
}
