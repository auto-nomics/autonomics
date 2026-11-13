import { useState } from "react";
import type { ScientificBrief } from "./canvas-planning";
import { planDifferences, planDifferenceLabel } from "./canvas-plan-diff";
export function CanvasPlanDraftChanges({baseline,draft,version}:{baseline:string;draft:ScientificBrief;version:number}) {
  const [open,setOpen]=useState(false);
  const diff=open?planDifferences(JSON.parse(baseline),draft):null;
  return <details aria-label="计划草稿修改预览" className="mb-3 rounded border p-3 text-sm" onToggle={event=>setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer">保存前查看草稿修改</summary>
    <p className="mt-2 text-xs text-muted-foreground">相对{version?`已保存计划 v${version}`:"空白计划"}的未保存差异；此预览不保存计划，不验证科学内容。</p>
    {diff&&<><p>共 {diff.total} 处差异{diff.omitted?`，另有 ${diff.omitted} 处未显示`:""}</p>{diff.fields.length>0&&<div className="mt-2 overflow-x-auto"><table className="w-full table-fixed text-left text-xs"><thead><tr><th className="w-1/4 p-2">位置</th><th className="p-2">已保存内容</th><th className="p-2">当前草稿</th></tr></thead><tbody>{diff.fields.map(field=><tr key={field.field} className="border-t"><th className="break-words p-2 align-top">{planDifferenceLabel(field.field)}</th><td className="whitespace-pre-wrap break-words p-2 align-top">{field.before}</td><td className="whitespace-pre-wrap break-words p-2 align-top">{field.after}</td></tr>)}</tbody></table></div>}</>}
  </details>;
}
