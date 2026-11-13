import { useCandidateDraft } from "./canvas-candidate-drafts";
import { CanvasCandidateRevision, type CandidateRevisionProps } from "./canvas-candidate-revision";
export function CanvasCandidateRelation(props: CandidateRevisionProps & { focusedId?: string }) {
  const { candidate, focusedId, busy } = props;
  const elements = (candidate.document.elements ?? []).filter(element => !element.isDeleted), focused = elements.filter(element => element.id === focusedId);
  const selected = focused.length === 1 ? focused[0] : undefined;
  const matches = selected ? elements.filter(element => element.id === (selected.type === "text" ? selected.containerId : selected.id)) : [];
  const arrow = matches.length === 1 && matches[0].type === "arrow" ? matches[0] : undefined;
  const start = arrow?.startArrowhead ?? null, end = arrow?.endArrowhead ?? null;
  const direction = start === null && (end === "arrow" || end === "bar") ? "forward" : start === "arrow" && end === "arrow" ? "bidirectional" : start === null && end === null ? "undirected" : "";
  const original = { direction, inhibitory: end === "bar", style: arrow && ["solid", "dashed", "dotted"].includes(arrow.strokeStyle) ? arrow.strokeStyle : "" };
  const draftKey = ["relation", arrow?.id ?? ""];
  const [draft, setDraft, resetDraft] = useCandidateDraft(candidate, draftKey, original);
  if (!arrow || !candidate.document_sha256) return null;
  const metadata = arrow.customData;
  if ((metadata != null && (typeof metadata !== "object" || Array.isArray(metadata))) || metadata?.annotationKind != null || metadata?.inset != null) return <section aria-label="候选关系方向与线型" className="shrink-0 border-t p-2 text-xs">注释与放大图引线使用专用编辑入口。</section>;
  const directionChanged = draft.direction !== original.direction || draft.inhibitory !== original.inhibitory, styleChanged = draft.style !== original.style;
  const invalidInhibition = draft.inhibitory && draft.direction !== "forward";
  const valid = !arrow.locked && !invalidInhibition && (!directionChanged || ["forward", "bidirectional", "undirected"].includes(draft.direction)) && (!styleChanged || ["solid", "dashed", "dotted"].includes(draft.style)) && (directionChanged || styleChanged);
  const operations: Record<string, unknown>[] = [];
  if (directionChanged) operations.push({ op: "set_relation_direction", id: arrow.id, direction: draft.direction, inhibitory: draft.inhibitory });
  if (styleChanged) operations.push({ op: "update", id: arrow.id, changes: { strokeStyle: draft.style } });
  const mismatch = direction && ((typeof metadata?.direction === "string" && metadata.direction !== direction) || (typeof metadata?.inhibitory === "boolean" && metadata.inhibitory !== original.inhibitory));
  return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={resetDraft} label="候选关系方向与线型" title="调整关系方向与线型" summary="手动调整候选关系方向与线型" submitLabel="预览候选关系调整" valid={valid} operations={operations}>
    <p>方向与抑制标记会改变图中表达的关系，请核对科学依据。线型只改变外观，不自动表示不确定性或可选机制。保留连接对象、文字、几何和已记录的来源。</p>
    {arrow.locked && <p role="status">请先解锁连线，再调整关系。</p>}
    {mismatch && <p role="status">当前端标记与记录的方向不一致；此处按画面端标记显示，请核对关系含义。</p>}
    {direction ? <>
      <label className="block">方向<select aria-label="候选关系方向" className="ml-2 rounded border bg-background p-1" value={draft.direction} disabled={busy || arrow.locked} onChange={event => setDraft(current => ({ ...current, direction: event.target.value }))}><option value="forward">单向（起点 → 终点）</option><option value="bidirectional">双向</option><option value="undirected">无方向</option></select></label>
      <label className="block"><input type="checkbox" aria-label="候选关系抑制" checked={draft.inhibitory} disabled={busy || arrow.locked} onChange={event => setDraft(current => ({ ...current, inhibitory: event.target.checked }))} />使用抑制端标记（仅支持单向）</label>
      {invalidInhibition && <p role="status">双向或无方向关系不支持抑制端标记；请明确取消抑制或恢复单向。</p>}
    </> : <p role="status">当前使用自定义端标记，可在此保留端标记并只修改线型。</p>}
    <label className="block">线型<select aria-label="候选关系线型" className="ml-2 rounded border bg-background p-1" value={draft.style} disabled={busy || arrow.locked} onChange={event => setDraft(current => ({ ...current, style: event.target.value }))}>{!original.style && <option value="">选择线型</option>}<option value="solid">实线</option><option value="dashed">虚线</option><option value="dotted">点线</option></select></label>
  </CanvasCandidateRevision>;
}
