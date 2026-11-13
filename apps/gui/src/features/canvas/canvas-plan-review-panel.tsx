import { useMemo } from "react";
import { Button } from "@/components/ui/button";
import { planLabelReview } from "./canvas-plan-review";
export function CanvasPlanReview({ document, onFocus }: { document: unknown; onFocus: (id: string) => void }) {
  const review = useMemo(() => planLabelReview(document), [document]);
  const differences = review.items.filter(item => item.status !== "same");
  const names = { changed: "文字变化", missing: "未找到唯一标签", ambiguous: "对应关系不唯一", same: "一致" };
  return <details aria-label="确认计划标签核对" className="shrink-0 border-b text-xs"><summary className="cursor-pointer p-2">确认计划标签核对{review.available ? ` · ${differences.length} 项待核对` : " · 无法对照"}</summary><div className="max-h-48 space-y-2 overflow-auto p-2">
    {review.available ? <><p>对照候选附带的历史计划 v{review.version}，已检查 {review.items.length} 项。仅忽略空白差异；本检查不验证科学正确性，也不阻止采用。文字过长时仅显示前 2000 字。</p>{differences.map(item => <article key={item.key} className="rounded border p-2"><p>{item.key} · {names[item.status]}</p><p className="whitespace-pre-wrap break-all">计划：{item.expected}</p><p className="whitespace-pre-wrap break-all">候选显示文字：{item.actual || "（无法确定）"}</p>{item.original !== undefined && <p className="whitespace-pre-wrap break-all">候选原始文字：{item.original}</p>}{item.id && <Button size="sm" variant="outline" onClick={() => onFocus(item.id!)}>定位{item.key}</Button>}</article>)}{review.omitted > 0 && <p>另有 {review.omitted} 项超过检查上限。</p>}</> : <p>{review.reason}</p>}
  </div></details>;
}
