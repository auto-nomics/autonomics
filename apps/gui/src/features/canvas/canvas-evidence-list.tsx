import { useState } from "react";
import type { ComponentProps } from "react";
import { useTranslation } from "react-i18next";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { relationSources } from "./canvas-source-manifest";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Report=ReturnType<typeof relationSources>;
export function CanvasEvidenceList({disabled,getApi,getRevision,ensureSaved,isDirty}:ComponentProps<typeof CanvasInset>) {
  const {t} = useTranslation("canvas");
  const directions:Record<string,string>={forward:t("relation.directionForward"),bidirectional:t("relation.directionBidirectional"),undirected:t("relation.directionUndirected")};
  const head=(value:string|null)=>value===null?t("relation.headNone"):value==="arrow"?t("relation.headArrow"):value==="bar"?t("relation.headBar"):t("evidence.headOther");
  const [open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const [review,setReview]=useState<{report:Report;revision:number;snapshot:string}|null>(null),[page,setPage]=useState(0);
  const [filter,setFilter]=useState("all");
  const filtered=(review?.report.relations??[]).filter(relation=>filter==="missing" ? relation.evidence_status==="not_recorded" || (relation.evidence_status==="recorded" && !relation.reference_count) : filter==="limited" ? relation.evidence_status==="unsupported" || relation.references_truncated || relation.references.some(reference=>reference.kind==="unsupported" || reference.truncated || reference.omitted_fields) : true);
  const snapshot=()=>{const api=getApi();return api?serializeAsJSON(api.getSceneElements(),api.getAppState(),api.getFiles(),"local"):"";};
  return <Dialog open={open} onOpenChange={value=>{if(!busy)setOpen(value);}}>
    <Button size="sm" variant="outline" disabled={disabled||busy} onClick={async()=>{
      setOpen(true);setBusy(true);setError("");setReview(null);setPage(0);setFilter("all");
      try {
        if(!await ensureSaved())throw new Error(t("common.resolveSaveError"));
        const revision=getRevision(),source=snapshot();
        if(revision===undefined||!source||isDirty())throw new Error(t("common.canvasChangedRetry"));
        setReview({report:relationSources(JSON.parse(source)),revision,snapshot:source});
      }catch(cause){setError(String(cause));}finally{setBusy(false);}
    }}>{t("evidence.button")}</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-2xl"><DialogTitle>{t("evidence.title")}</DialogTitle>
      {error&&<p role="alert" className="text-sm text-destructive">{error}</p>}
      {review&&<div className="space-y-3 text-sm">
        <p>{t("evidence.summary",{revision:review.revision,total:review.report.total})}</p>
        <p>{t("evidence.description")}</p>
        <label>{t("evidence.filterLabel")}<select aria-label={t("evidence.filterLabel")} className="ml-2 rounded border bg-background p-1" value={filter} onChange={event=>{setFilter(event.target.value);setPage(0);}}><option value="all">{t("evidence.filterAll")}</option><option value="missing">{t("evidence.filterMissing")}</option><option value="limited">{t("evidence.filterLimited")}</option></select></label>
        <p aria-label={t("evidence.filterCountLabel")}>{t("evidence.filterCount",{count:filtered.length})}</p>
        {review.report.total>0&&!filtered.length&&<p>{t("evidence.noMatch")}</p>}
        {!review.report.total&&<p>{t("evidence.noRelations")}</p>}
        {filtered.slice(page*20,(page+1)*20).map((relation,index)=><article key={`${relation.element_id}:${index}`} className="space-y-1 rounded border p-2" aria-label={t("evidence.relationLabel",{id:relation.element_id ?? t("evidence.unknownObject")})}>
          <p className="break-all font-medium">{relation.element_id??t("evidence.unknownObjectId")}</p>
          <p className="break-all">{t("evidence.endpoints",{start:relation.start_element_id??t("evidence.unbound"),end:relation.end_element_id??t("evidence.unbound")})}</p>
          <p>{t("evidence.declaredLine",{start:head(relation.start_arrowhead),end:head(relation.end_arrowhead),direction:relation.declared_direction?directions[relation.declared_direction]:t("relation.notRecorded"),inhibition:relation.declared_inhibitory===null?t("relation.notRecorded"):relation.declared_inhibitory?t("relation.yes"):t("relation.no")})}</p>
          {relation.evidence_status==='not_recorded'?<p>{t("evidence.notRecorded")}</p>:relation.evidence_status==='unsupported'?<p>{t("evidence.unsupported")}</p>:!relation.reference_count?<p>{t("evidence.emptyRecorded")}</p>:null}
          {relation.references.map((reference,i)=><div key={i} className="whitespace-pre-wrap break-all border-l pl-2">{reference.kind==='text'?reference.text:reference.kind==='structured'?Object.entries(reference.fields??{}).map(([key,value])=><p key={key}>{key}: {value}</p>):t("evidence.referenceUnsupported")}{reference.truncated&&<p>{t("evidence.referenceTruncated")}</p>}{reference.omitted_fields&&<p>{t("evidence.referenceOmitted")}</p>}</div>)}
          {relation.references_truncated&&<p>{t("evidence.referencesTruncated",{count:relation.reference_count})}</p>}
          <Button size="sm" variant="outline" disabled={!relation.element_id} aria-label={t("evidence.locateLabel",{id:relation.element_id})} onClick={()=>{
            try{
              if(isDirty()||getRevision()!==review.revision||snapshot()!==review.snapshot)throw new Error(t("evidence.canvasChanged"));
              const api=getApi(),element=api?.getSceneElements().find(e=>e.id===relation.element_id&&!e.isDeleted);
              if(!api||!element)throw new Error(t("evidence.objectUnavailable"));
              api.updateScene({appState:{selectedElementIds:{[element.id]:true}}});api.scrollToContent(element,{fitToContent:true,maxZoom:1.5});setOpen(false);
            }catch(cause){setError(String(cause));}
          }}>{t("evidence.locate")}</Button>
        </article>)}
        {filtered.length>20&&<div className="flex items-center gap-2"><Button size="sm" disabled={page===0} onClick={()=>setPage(value=>value-1)}>{t("evidence.prevPage")}</Button><span>{page+1}/{Math.ceil(filtered.length/20)}</span><Button size="sm" disabled={(page+1)*20>=filtered.length} onClick={()=>setPage(value=>value+1)}>{t("evidence.nextPage")}</Button></div>}
        {review.report.truncated&&<p>{t("evidence.truncated")}</p>}
      </div>}
    </DialogContent>
  </Dialog>;
}
