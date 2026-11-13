import { useEffect, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import type { Planning, ScientificBrief } from "./canvas-planning";
import { planDifferences, planDifferenceLabel } from "./canvas-plan-diff";
export function CanvasPlanChanges({projectId,canvasId,record,dirty,busy,onRestore}:{projectId:string;canvasId:string;record:Planning;dirty:boolean;busy:boolean;onRestore:(document:ScientificBrief)=>void}) {
  const [open,setOpen]=useState(false);
  const [restored,setRestored]=useState(false);
  const [state,setState]=useState<{previous?:Planning;error?:string;loading?:boolean}>({});
  const path=`/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/planning/revisions/${record.version-1}`;
  useEffect(()=>{
    if(!open||record.version<2)return;
    let active=true;setState({loading:true});
    void gatewayRequest("GET",path).then(value=>{
      const previous=value as Planning;
      if(previous.version!==record.version-1)throw new Error("历史计划版本不匹配，请刷新重试。");
      if(active)setState({previous});
    }).catch(error=>{if(active)setState({error:String(error)});});
    return ()=>{active=false;};
  },[open,path,record.version]);
  if(record.version<2)return null;
  const diff=state.previous?planDifferences(state.previous.document,record.document):null;
  return <details aria-label="已保存计划版本差异" className="mb-3 rounded border p-3 text-sm" onToggle={event=>setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer">查看计划 v{record.version-1} → v{record.version} 的修改</summary>
    <p className="mt-2 text-xs text-muted-foreground">比较两个已保存版本，不修改计划或图稿。{dirty?" 当前草稿有未保存修改，不包含在此比较中。":""}</p>
    {state.previous&&<div className="my-2"><Button size="sm" variant="outline" disabled={dirty||busy||!diff?.total} onClick={()=>{if(dirty||busy||!diff?.total||!state.previous)return;onRestore(state.previous.document);setRestored(true);}}>将 v{record.version-1} 载入草稿</Button><p className="mt-1 text-xs text-muted-foreground">先处理当前未保存修改，再载入历史内容。检查草稿并保存后会创建新计划版本。</p></div>}
    {restored&&<p role="status">已将 v{record.version-1} 载入草稿，请检查内容后保存。</p>}
    {state.loading&&<p role="status">读取历史计划…</p>}
    {state.error&&<p role="alert" className="text-destructive">{state.error}</p>}
    {diff&&<><p>共 {diff.total} 处差异{diff.omitted?`，另有 ${diff.omitted} 处未显示`:""} · 画布基线 v{state.previous!.based_on_revision} → v{record.based_on_revision}</p>
      {diff.fields.length>0&&<div className="mt-2 overflow-x-auto"><table className="w-full table-fixed text-left text-xs"><thead><tr><th className="w-1/4 p-2">位置</th><th className="p-2">修改前</th><th className="p-2">修改后</th></tr></thead><tbody>{diff.fields.map(field=><tr key={field.field} className="border-t"><th className="break-words p-2 align-top">{planDifferenceLabel(field.field)}</th><td className="whitespace-pre-wrap break-words p-2 align-top">{field.before}</td><td className="whitespace-pre-wrap break-words p-2 align-top">{field.after}</td></tr>)}</tbody></table></div>}</>}
  </details>;
}
