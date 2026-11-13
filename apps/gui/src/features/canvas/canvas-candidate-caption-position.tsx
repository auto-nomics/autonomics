import {useCandidateDraft} from './canvas-candidate-drafts';
import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
export function CanvasCandidateCaptionPosition(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId,busy}=props,elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted),selected=elements.find(e=>e.id===focusedId),arrowId=selected?.type==='text'?selected.containerId:selected?.id,arrow=elements.find(e=>e.id===arrowId&&e.type==='arrow'),labels=elements.filter(e=>e.type==='text'&&e.containerId===arrow?.id),caption=labels.length===1?labels[0]:undefined,key=['arrow-caption-position',caption?.id??''];
 const [draft,setDraft,reset]=useCandidateDraft(candidate,key,{position:'50',rewrap:false,width:String(caption?.width??'')});
 const {position,rewrap,width}=draft;
 if(!arrow||!caption||caption.type!=='text'||!candidate.document_sha256)return null;
 const number=Number(position),unsupported=arrow.locked||caption.locked||arrow.angle!==0||caption.angle!==0||arrow.roughness!==0||Boolean(arrow.roundness)||('elbowed' in arrow&&arrow.elbowed);
 const widthNumber=Number(width),canRewrap=caption.fontFamily===9&&(caption.originalText??caption.text).length<=2048;
 const valid=!unsupported&&position.trim().length>0&&Number.isFinite(number)&&number>0&&number<100&&(!rewrap||(canRewrap&&width.trim().length>0&&Number.isFinite(widthNumber)&&widthNumber>=1&&widthNumber<=20000));
 const operations:Record<string,unknown>[]=[...(rewrap?[{op:'set_wrapped_text',id:caption.id,text:caption.originalText??caption.text,font_size:caption.fontSize,max_width:widthNumber}]:[]),{op:'position_arrow_caption',id:caption.id,position:number/100}];
 return <CanvasCandidateRevision {...props} draftKey={key} onCommitted={reset} label="候选箭头说明位置" title="沿连线移动说明" summary="沿现有连线调整说明位置" submitLabel="预览说明位置" valid={valid} operations={operations}>
 <p>从连线起点沿路径长度定位，50% 为中点。保留完整文字、字体、端点、方向与实际路径；请核对预览中的间距后再采用。</p>
 {unsupported&&<p role="status">此入口要求未锁定、无旋转的说明，以及无粗糙或曲线效果的原生连线。</p>}
 <label>沿连线位置<input aria-label="候选箭头说明路径百分比" type="number" min="0" max="100" step="any" value={position} disabled={busy||unsupported} onChange={event=>setDraft(current=>({...current,position:event.target.value}))} className="ml-2 w-24 rounded border bg-background p-1"/>%</label>
 <label className="block"><input aria-label="同时调整说明换行宽度" type="checkbox" checked={rewrap} disabled={busy||unsupported||!canRewrap} onChange={event=>setDraft(current=>({...current,rewrap:event.target.checked}))}/> 同时调整换行宽度，保留原文和字号</label>
 {rewrap&&<label className="block">换行宽度<input aria-label="沿线说明换行宽度" type="number" min="1" max="20000" step="any" value={width} disabled={busy||unsupported} onChange={event=>setDraft(current=>({...current,width:event.target.value}))} className="ml-2 w-24 rounded border bg-background p-1"/><span className="ml-2">换行与位置会一起生成一个候选；请核对与其他文字的间距。</span></label>}
 </CanvasCandidateRevision>;
}
