import {useState} from "react";
import type {ComponentProps} from "react";
import {serializeAsJSON} from "@excalidraw/excalidraw";
import type {CanvasInset} from "./canvas-inset";
import {gatewayRequest} from "@/gateway";
import {Button} from "@/components/ui/button";
import {Dialog,DialogContent,DialogTitle} from "@/components/ui/dialog";
export function CanvasDuplicate({projectId,canvasId,disabled,getApi,getRevision,ensureSaved,isDirty,onApplied}:ComponentProps<typeof CanvasInset>){
  const [open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const [review,setReview]=useState<{ids:string[];revision:number;snapshot:string;prefix:string}|null>(null),[dx,setDx]=useState("240"),[dy,setDy]=useState("0");
  const snapshot=()=>{const api=getApi();return api?serializeAsJSON(api.getSceneElements(),api.getAppState(),api.getFiles(),"local"):"";};
  const validOffset=(value:string)=>value.trim()!==""&&Number.isFinite(Number(value))&&Math.abs(Number(value))<=1e6;
  return <Dialog open={open} onOpenChange={value=>{if(!busy)setOpen(value);}}>
    <Button size="sm" variant="outline" disabled={disabled||busy} onClick={async()=>{
      setOpen(true);setBusy(true);setError("");setReview(null);
      try{
        if(!await ensureSaved())throw new Error("请先解决画布保存错误。");
        const api=getApi(),revision=getRevision();if(!api||revision===undefined||isDirty())throw new Error("画布发生变化，请保存后重试。");
        const selection=api.getAppState().selectedElementIds,ids=api.getSceneElements().filter(e=>!e.isDeleted&&selection[e.id]===true).map(e=>e.id);
        if(!ids.length||ids.length>100)throw new Error("请选中 1 至 100 个对象；组合和附属对象会自动展开。");
        setReview({ids,revision,snapshot:snapshot(),prefix:`copy_${crypto.randomUUID().replaceAll('-','')}`});
      }catch(cause){setError(String(cause));}finally{setBusy(false);}
    }}>复制组件修改</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-xl"><DialogTitle>复制选中组件</DialogTitle>
      {error&&<p role="alert" className="text-sm text-destructive">{error}</p>}
      {review&&<div className="space-y-3 text-sm">
        <p>已选 {review.ids.length} 个对象 · v{review.revision}。复制会包含所属组合、帧内容、标签和图片，最多展开 1000 个对象。</p>
        <label className="block">水平偏移<input aria-label="复制水平偏移" className="ml-2 w-28 rounded border bg-background p-1" type="number" min="-1000000" max="1000000" step="any" value={dx} disabled={busy} onChange={event=>setDx(event.target.value)}/></label>
        <label className="block">垂直偏移<input aria-label="复制垂直偏移" className="ml-2 w-28 rounded border bg-background p-1" type="number" min="-1000000" max="1000000" step="any" value={dy} disabled={busy} onChange={event=>setDy(event.target.value)}/></label>
        <p>内部连接和图片归属指向副本，未复制的外部连接会断开。原组件保持不变。现有声明与引用会保留，请核对是否适用于副本；批注、放大图和锁定成员不能在此复制。</p>
        <Button disabled={busy||!validOffset(dx)||!validOffset(dy)} onClick={async()=>{
          if(busy)return;setBusy(true);setError("");
          try{
            if(isDirty()||getRevision()!==review.revision||snapshot()!==review.snapshot)throw new Error("画布发生变化，请重新选择复制组件。");
            const saved=await gatewayRequest("POST",`/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`,{expected_revision:review.revision,summary:"复制选中组件",operations:[{op:"duplicate",element_ids:review.ids,id_prefix:review.prefix,dx:Number(dx),dy:Number(dy)}]}) as {id:string};
            if(isDirty()||getRevision()!==review.revision||snapshot()!==review.snapshot)throw new Error("生成期间画布发生变化；修改已保存，请核对当前画布。");
            setOpen(false);onApplied(saved.id);
          }catch(cause){setError(String(cause));}finally{setBusy(false);}
        }}>应用复制修改</Button>
      </div>}
    </DialogContent>
  </Dialog>;
}
