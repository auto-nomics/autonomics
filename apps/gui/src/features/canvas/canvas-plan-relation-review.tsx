import { useMemo } from "react";
import { Button } from "@/components/ui/button";
import { planRelationReview, type RelationState } from "./canvas-plan-review";
const directions: Record<string, string> = { forward: "单向", bidirectional: "双向", undirected: "无方向" }, styles: Record<string, string> = { solid: "实线", dashed: "虚线", dotted: "点线" };
const describe = (state: RelationState | null) => state ? `${directions[state.direction ?? ""] ?? "端标记无法判定"} · ${state.inhibitory === null ? "抑制无法判定" : state.inhibitory ? "抑制" : "无抑制标记"} · ${styles[state.line_style ?? ""] ?? "线型无法判定"}` : "无法确定";
export function CanvasPlanRelationReview({ document, onFocus }: { document: unknown; onFocus: (id: string) => void }) {
  const review = useMemo(() => planRelationReview(document), [document]), differences = review.items.filter(item => item.status !== "same");
  const names = { changed: "方向、抑制或线型变化", missing: "未找到唯一关系", ambiguous: "对应关系不唯一", unsupported: "当前端标记或线型无法判定", same: "一致" };
  return <details aria-label="确认计划关系核对" className="shrink-0 border-b text-xs"><summary className="cursor-pointer p-2">确认计划关系核对{review.available ? ` · ${differences.length} 项待核对` : " · 无法对照"}</summary><div className="max-h-48 space-y-2 overflow-auto p-2">
    {review.available ? <><p>对照候选附带的历史计划 v{review.version}，已检查 {review.items.length} 条关系，包含无标签关系。比较实际端标记、抑制和线型；不验证科学依据，也不自动恢复旧计划或阻止采用。线型差异不等于科学含义改变。</p>{differences.map(item => <article key={item.key} className="rounded border p-2"><p>{item.key} · {names[item.status]}</p><p>计划：{describe(item.expected)}</p><p>候选：{describe(item.actual)}</p>{item.metadata_mismatch && <p>候选记录的方向或抑制状态与画面端标记不一致。</p>}{item.id && <Button size="sm" variant="outline" onClick={() => onFocus(item.id!)}>定位{item.key}</Button>}</article>)}{review.omitted > 0 && <p>另有 {review.omitted} 条关系超过检查上限。</p>}</> : <p>{review.reason}</p>}
  </div></details>;
}
