import { useState } from "react";
import type { ComponentProps } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Direction = "forward" | "bidirectional" | "undirected";
const names: Record<Direction, string> = { forward: "单向", bidirectional: "双向", undirected: "无向" };
const directionValue = (value: unknown): Direction | "" => value === "forward" || value === "bidirectional" || value === "undirected" ? value : "";
function observed(start: unknown, end: unknown): { direction: Direction | ""; inhibitory: boolean } {
  if (start == null && end === "arrow") return { direction: "forward", inhibitory: false };
  if (start == null && end === "bar") return { direction: "forward", inhibitory: true };
  if (start === "arrow" && end === "arrow") return { direction: "bidirectional", inhibitory: false };
  if (start == null && end == null) return { direction: "undirected", inhibitory: false };
  return { direction: "", inhibitory: false };
}
const headName = (head: unknown) => head == null ? "无" : head === "arrow" ? "箭头" : head === "bar" ? "抑制横杠" : "其他样式";
type Review = { id: string; revision: number; snapshot: string; start: string; end: string; declared: Direction | ""; inhibition?: boolean; mismatch: boolean; evidence: string[]; evidenceCount: number; evidenceUnknown: boolean };
export function CanvasRelation({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onProposed }: ComponentProps<typeof CanvasInset>) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [review, setReview] = useState<Review | null>(null), [direction, setDirection] = useState<Direction | "">(""), [inhibitory, setInhibitory] = useState(false);
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
      setOpen(true); setBusy(true); setReview(null); setError("");
      try {
        if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
        const api = getApi(), revision = getRevision();
        if (!api || revision === undefined || isDirty()) throw new Error("画布发生变化，请保存后重试。");
        const selected = api.getAppState().selectedElementIds;
        const chosen = api.getSceneElements().filter(element => !element.isDeleted && selected[element.id] === true);
        const arrows = chosen.filter(element => element.type === "arrow");
        if (arrows.length !== 1 || chosen.some(element => element.id !== arrows[0].id && !(element.type === "text" && element.containerId === arrows[0].id))) throw new Error("请只选中一条关系连线（可包含它的绑定文字）。");
        const arrow = arrows[0];
        if (arrow.type !== "arrow") throw new Error("请选中关系连线。");
        const data = arrow.customData;
        if (arrow.locked || (data != null && (typeof data !== "object" || Array.isArray(data))) || data?.annotationKind != null || data?.inset != null) throw new Error("锁定、批注或放大图引导连线不能在这里修改。");
        const actual = observed(arrow.startArrowhead, arrow.endArrowhead), declared = directionValue(data?.direction);
        const inhibition = typeof data?.inhibitory === "boolean" ? data.inhibitory : undefined;
        const evidence = Array.isArray(data?.evidence) ? data.evidence.filter((value: unknown): value is string => typeof value === "string") : [];
        setDirection(actual.direction); setInhibitory(actual.inhibitory);
        setReview({ id: arrow.id, revision, snapshot: snapshot(), start: headName(arrow.startArrowhead), end: headName(arrow.endArrowhead), declared, inhibition, mismatch: Boolean(actual.direction && ((declared && declared !== actual.direction) || (inhibition !== undefined && inhibition !== actual.inhibitory))), evidence: evidence.slice(0, 20).map((value: string) => value.length > 600 ? `${value.slice(0, 600)}…（已截断）` : value), evidenceCount: evidence.length, evidenceUnknown: data?.evidence != null && (!Array.isArray(data.evidence) || data.evidence.length !== evidence.length) });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>检查并修改关系</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-xl"><DialogTitle>关系方向与依据</DialogTitle>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {review && <div className="space-y-3 text-sm">
        <p className="break-all">连线：{review.id} · v{review.revision}</p>
        <p aria-label="实际关系端标记">当前端标记 · 起点：{review.start} · 终点：{review.end}</p>
        <p aria-label="已记录关系声明">已记录声明 · 方向：{review.declared ? names[review.declared] : "未记录"} · 抑制：{review.inhibition === undefined ? "未记录" : review.inhibition ? "是" : "否"}</p>
        {review.mismatch && <p role="status">已记录声明与当前画法不同，请核对后选择新设置。</p>}
        <section aria-label="关系依据"><p className="font-medium">已记录依据</p>{review.evidence.length ? review.evidence.map((entry, index) => <p key={index} className="whitespace-pre-wrap break-all">{entry}</p>) : <p>{review.evidenceUnknown ? "请结合图稿来源核对依据。" : "未记录依据。"}</p>}{review.evidenceCount > review.evidence.length && <p>共 {review.evidenceCount} 条，仅显示前 {review.evidence.length} 条。</p>}{review.evidenceUnknown && <p>包含当前面板未展示的结构化依据，请核对源文件。</p>}</section>
        <label className="block">新关系方向<select aria-label="新关系方向" className="ml-2 rounded border bg-background p-1" value={direction} disabled={busy || inhibitory} onChange={event => setDirection(directionValue(event.target.value))}><option value="">请选择方向</option>{Object.entries(names).map(([value, name]) => <option key={value} value={value}>{name}</option>)}</select></label>
        <label className="flex items-center gap-2"><input aria-label="新关系抑制标记" type="checkbox" checked={inhibitory} disabled={busy || (direction !== "forward" && !inhibitory)} onChange={event => setInhibitory(event.target.checked)} />单向抑制端标记</label>
        <p className="text-xs text-muted-foreground">初始设置按当前画法显示。提交后先预览提案，核对方向、文字与依据，再采用。</p>
        <Button disabled={busy || !direction} onClick={async () => {
          if (!direction || busy) return; setBusy(true); setError("");
          try {
            if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error("画布发生变化，请重新检查连线。");
            const candidate = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/proposals`, { expected_revision: review.revision, summary: "修改选中关系的方向与抑制标记", operations: [{ op: "set_relation_direction", id: review.id, direction, inhibitory }] }) as { id: string };
            if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error("生成期间画布发生变化；提案已保留，请从列表重新核对。");
            setOpen(false); onProposed(candidate.id);
          } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>预览关系修改提案</Button>
      </div>}
    </DialogContent>
  </Dialog>;
}
