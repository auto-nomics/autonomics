import { useCandidateDraft } from "./canvas-candidate-drafts";
import { CanvasCandidateRevision, type CandidateRevisionProps } from "./canvas-candidate-revision";
export function CanvasCandidateText(props: CandidateRevisionProps & { focusedId?: string }) {
  const { candidate, focusedId, busy } = props;
  const elements = (candidate.document.elements ?? []).filter(element => !element.isDeleted);
  const focused = elements.filter(element => element.id === focusedId);
  const targets = focused.length === 1 ? elements.filter(element => element.type === "text" && (element.id === focusedId || element.containerId === focusedId)) : [];
  const target = targets.length === 1 && targets[0].type === "text" ? targets[0] : null;
  const container = target?.containerId ? elements.find(element => element.id === target.containerId) : undefined;
  const draftKey = ["text", target?.id ?? ""];
  const [draft, setDraft, resetDraft] = useCandidateDraft(candidate, draftKey, { text: target?.originalText ?? target?.text ?? "", fontSize: String(target?.fontSize ?? ""), width: String(target?.width ?? "") });
  const { text, fontSize, width } = draft;
  if (!target || !candidate.document_sha256) return null;
  const adjustableWidth = target.containerId == null || container?.type === "arrow";
  const unsupported = target.fontFamily !== 9 || target.angle !== 0 || target.locked || (container && (container.angle !== 0 || container.locked));
  const valid = !unsupported && text.trim().length > 0 && text.length <= 2048 && Number.isFinite(Number(fontSize)) && Number(fontSize) >= 4 && Number(fontSize) <= 200 && (!adjustableWidth || (Number.isFinite(Number(width)) && Number(width) >= 1 && Number(width) <= 20000));
  return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={resetDraft} label="直接修订候选文字" title="直接修订当前标签" summary="手动修订候选文字与换行" submitLabel="预览候选文字修订" valid={valid} operations={[{ op: "set_wrapped_text", id: target.id, text, font_size: Number(fontSize), ...(adjustableWidth ? { max_width: Number(width) } : {}) }]}>
    {unsupported && <p role="status">请先解锁并使用未旋转的科学字体文字与容器；也可交给 Agent 调整。</p>}
    <label className="block">标签原文<textarea aria-label="候选标签原文" className="mt-1 min-h-20 w-full rounded border bg-background p-2" maxLength={2048} value={text} disabled={busy || unsupported} onChange={event => setDraft(current => ({ ...current, text: event.target.value }))} /></label>
    <label>字号<input aria-label="候选标签字号" type="number" min="4" max="200" step="any" className="ml-2 w-24 rounded border bg-background p-1" value={fontSize} disabled={busy || unsupported} onChange={event => setDraft(current => ({ ...current, fontSize: event.target.value }))} /></label>
    {adjustableWidth ? <label className="ml-3">换行宽度<input aria-label="候选标签换行宽度" type="number" min="1" max="20000" step="any" className="ml-2 w-24 rounded border bg-background p-1" value={width} disabled={busy || unsupported} onChange={event => setDraft(current => ({ ...current, width: event.target.value }))} /></label> : <p>按现有容器内部空间换行；内容过高时请调整字号或容器。</p>}
    {container?.type === "arrow" && <p className="text-xs text-muted-foreground">只改变标签换行与尺寸，保留连线几何和绑定；请核对预览中的遮挡。</p>}
  </CanvasCandidateRevision>;
}
