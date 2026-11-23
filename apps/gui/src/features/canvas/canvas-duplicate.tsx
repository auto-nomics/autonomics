import {useState} from "react";
import type {ComponentProps} from "react";
import {useTranslation} from "react-i18next";
import {serializeAsJSON} from "@excalidraw/excalidraw";
import type {CanvasInset} from "./canvas-inset";
import {gatewayRequest} from "@/gateway";
import {Button} from "@/components/ui/button";
import {Dialog,DialogContent,DialogTitle} from "@/components/ui/dialog";
export function CanvasDuplicate({projectId,canvasId,disabled,getApi,getRevision,ensureSaved,isDirty,onApplied}:ComponentProps<typeof CanvasInset>){
  const {t} = useTranslation("canvas");
  const [open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const [review,setReview]=useState<{ids:string[];revision:number;snapshot:string;prefix:string}|null>(null),[dx,setDx]=useState("240"),[dy,setDy]=useState("0");
  const snapshot=()=>{const api=getApi();return api?serializeAsJSON(api.getSceneElements(),api.getAppState(),api.getFiles(),"local"):"";};
  const validOffset=(value:string)=>value.trim()!==""&&Number.isFinite(Number(value))&&Math.abs(Number(value))<=1e6;
  return <Dialog open={open} onOpenChange={value=>{if(!busy)setOpen(value);}}>
    <Button size="sm" variant="outline" disabled={disabled||busy} onClick={async()=>{
      setOpen(true);setBusy(true);setError("");setReview(null);
      try{
        if(!await ensureSaved())throw new Error(t("common.resolveSaveError"));
        const api=getApi(),revision=getRevision();if(!api||revision===undefined||isDirty())throw new Error(t("common.canvasChangedRetry"));
        const selection=api.getAppState().selectedElementIds,ids=api.getSceneElements().filter(e=>!e.isDeleted&&selection[e.id]===true).map(e=>e.id);
        if(!ids.length||ids.length>100)throw new Error(t("duplicate.selectRange"));
        setReview({ids,revision,snapshot:snapshot(),prefix:`copy_${crypto.randomUUID().replaceAll('-','')}`});
      }catch(cause){setError(String(cause));}finally{setBusy(false);}
    }}>{t("duplicate.button")}</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-xl"><DialogTitle>{t("duplicate.title")}</DialogTitle>
      {error&&<p role="alert" className="text-sm text-destructive">{error}</p>}
      {review&&<div className="space-y-3 text-sm">
        <p>{t("duplicate.summary", { count: review.ids.length, revision: review.revision })}</p>
        <label className="block">{t("duplicate.offsetX")}<input aria-label={t("duplicate.offsetXLabel")} className="ml-2 w-28 rounded border bg-background p-1" type="number" min="-1000000" max="1000000" step="any" value={dx} disabled={busy} onChange={event=>setDx(event.target.value)}/></label>
        <label className="block">{t("duplicate.offsetY")}<input aria-label={t("duplicate.offsetYLabel")} className="ml-2 w-28 rounded border bg-background p-1" type="number" min="-1000000" max="1000000" step="any" value={dy} disabled={busy} onChange={event=>setDy(event.target.value)}/></label>
        <p>{t("duplicate.note")}</p>
        <Button disabled={busy||!validOffset(dx)||!validOffset(dy)} onClick={async()=>{
          if(busy)return;setBusy(true);setError("");
          try{
            if(isDirty()||getRevision()!==review.revision||snapshot()!==review.snapshot)throw new Error(t("duplicate.canvasChangedReselect"));
            const saved=await gatewayRequest("POST",`/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`,{expected_revision:review.revision,summary:t("duplicate.summaryOperation"),operations:[{op:"duplicate",element_ids:review.ids,id_prefix:review.prefix,dx:Number(dx),dy:Number(dy)}]}) as {id:string};
            if(isDirty()||getRevision()!==review.revision||snapshot()!==review.snapshot)throw new Error(t("duplicate.canvasChangedDuringGeneration"));
            setOpen(false);onApplied(saved.id);
          }catch(cause){setError(String(cause));}finally{setBusy(false);}
        }}>{t("duplicate.apply")}</Button>
      </div>}
    </DialogContent>
  </Dialog>;
}
