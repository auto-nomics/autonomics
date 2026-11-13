import { useState } from "react";
import type { ComponentProps } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { relationSources } from "./canvas-source-manifest";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Report=ReturnType<typeof relationSources>;
const directions:Record<string,string>={forward:"单向",bidirectional:"双向",undirected:"无向"};
const head=(value:string|null)=>value===null?"无":value==="arrow"?"箭头":value==="bar"?"抑制横杠":"其他端标记";
export function CanvasEvidenceList({disabled,getApi,getRevision,ensureSaved,isDirty}:ComponentProps<typeof CanvasInset>) {
  const [open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const [review,setReview]=useState<{report:Report;revision:number;snapshot:string}|null>(null),[page,setPage]=useState(0);
  const [filter,setFilter]=useState("all");
  const filtered=(review?.report.relations??[]).filter(relation=>filter==="missing" ? relation.evidence_status==="not_recorded" || (relation.evidence_status==="recorded" && !relation.reference_count) : filter==="limited" ? relation.evidence_status==="unsupported" || relation.references_truncated || relation.references.some(reference=>reference.kind==="unsupported" || reference.truncated || reference.omitted_fields) : true);
  const snapshot=()=>{const api=getApi();return api?serializeAsJSON(api.getSceneElements(),api.getAppState(),api.getFiles(),"local"):"";};
  return <Dialog open={open} onOpenChange={value=>{if(!busy)setOpen(value);}}>
    <Button size="sm" variant="outline" disabled={disabled||busy} onClick={async()=>{
      setOpen(true);setBusy(true);setError("");setReview(null);setPage(0);setFilter("all");
      try {
        if(!await ensureSaved())throw new Error("请先解决画布保存错误。");
        const revision=getRevision(),source=snapshot();
        if(revision===undefined||!source||isDirty())throw new Error("画布发生变化，请保存后重试。");
        setReview({report:relationSources(JSON.parse(source)),revision,snapshot:source});
      }catch(cause){setError(String(cause));}finally{setBusy(false);}
    }}>关系依据清单</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-2xl"><DialogTitle>整图关系依据</DialogTitle>
      {error&&<p role="alert" className="text-sm text-destructive">{error}</p>}
      {review&&<div className="space-y-3 text-sm">
        <p>当前图稿 v{review.revision} · 普通连线 {review.report.total} 条 · 本清单未核验引用真实性或科学支持。</p>
        <p>实际端标记与已记录方向分别列出。缺少引用不自动表示关系错误；请逐条结合图意核对。来源导出包含同一格式的引用记录。</p>
        <label>依据记录筛选<select aria-label="依据记录筛选" className="ml-2 rounded border bg-background p-1" value={filter} onChange={event=>{setFilter(event.target.value);setPage(0);}}><option value="all">全部关系</option><option value="missing">未记录或空列表</option><option value="limited">格式或展示不完整</option></select></label>
        <p aria-label="依据筛选数量">匹配 {filtered.length} 条（当前清单范围内）</p>
        {review.report.total>0&&!filtered.length&&<p>此筛选下没有匹配关系。</p>}
        {!review.report.total&&<p>当前没有普通关系连线。</p>}
        {filtered.slice(page*20,(page+1)*20).map((relation,index)=><article key={`${relation.element_id}:${index}`} className="space-y-1 rounded border p-2" aria-label={`关系依据 ${relation.element_id ?? '未知对象'}`}>
          <p className="break-all font-medium">{relation.element_id??"未知对象 ID"}</p>
          <p className="break-all">端点：{relation.start_element_id??"未绑定"} → {relation.end_element_id??"未绑定"}（仅为布局端点）</p>
          <p>实际端标记：{head(relation.start_arrowhead)} / {head(relation.end_arrowhead)}；声明方向：{relation.declared_direction?directions[relation.declared_direction]:"未记录"}；抑制声明：{relation.declared_inhibitory===null?"未记录":relation.declared_inhibitory?"是":"否"}</p>
          {relation.evidence_status==='not_recorded'?<p>未记录依据。</p>:relation.evidence_status==='unsupported'?<p>依据格式未支持，请核对完整源文件。</p>:!relation.reference_count?<p>已记录空依据列表。</p>:null}
          {relation.references.map((reference,i)=><div key={i} className="whitespace-pre-wrap break-all border-l pl-2">{reference.kind==='text'?reference.text:reference.kind==='structured'?Object.entries(reference.fields??{}).map(([key,value])=><p key={key}>{key}: {value}</p>):"此项格式未支持，请核对源文件。"}{reference.truncated&&<p>引用已截断。</p>}{reference.omitted_fields&&<p>部分结构化字段未展示，请核对源文件。</p>}</div>)}
          {relation.references_truncated&&<p>共 {relation.reference_count} 项，仅展示前 100 项。</p>}
          <Button size="sm" variant="outline" disabled={!relation.element_id} aria-label={`定位关系 ${relation.element_id}`} onClick={()=>{
            try{
              if(isDirty()||getRevision()!==review.revision||snapshot()!==review.snapshot)throw new Error("画布发生变化，请重新打开清单。");
              const api=getApi(),element=api?.getSceneElements().find(e=>e.id===relation.element_id&&!e.isDeleted);
              if(!api||!element)throw new Error("对象已不可用，请重新打开清单。");
              api.updateScene({appState:{selectedElementIds:{[element.id]:true}}});api.scrollToContent(element,{fitToContent:true,maxZoom:1.5});setOpen(false);
            }catch(cause){setError(String(cause));}
          }}>定位关系</Button>
        </article>)}
        {filtered.length>20&&<div className="flex items-center gap-2"><Button size="sm" disabled={page===0} onClick={()=>setPage(value=>value-1)}>上一页关系</Button><span>{page+1}/{Math.ceil(filtered.length/20)}</span><Button size="sm" disabled={(page+1)*20>=filtered.length} onClick={()=>setPage(value=>value+1)}>下一页关系</Button></div>}
        {review.report.truncated&&<p>仅列出前 1000 条关系，请结合完整源文件核对。</p>}
      </div>}
    </DialogContent>
  </Dialog>;
}
