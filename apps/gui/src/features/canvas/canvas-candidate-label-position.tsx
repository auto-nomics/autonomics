import { useCandidateDraft } from "./canvas-candidate-drafts";
import { CanvasCandidateRevision, type CandidateRevisionProps } from "./canvas-candidate-revision";
export function CanvasCandidateLabelPosition(props: CandidateRevisionProps & { focusedId?: string }) {
  const { candidate, focusedId, busy } = props, elements = (candidate.document.elements ?? []).filter(element => !element.isDeleted), matches = elements.filter(element => element.id === focusedId);
  const text = matches.length === 1 && matches[0].type === "text" ? matches[0] : undefined;
  const draftKey = ["label-position", text?.id ?? ""], [draft, setDraft, resetDraft] = useCandidateDraft(candidate, draftKey, { x: String(text?.x ?? ""), y: String(text?.y ?? "") });
  if (!text || !candidate.document_sha256) return null;
  const connected = (Array.isArray(text.boundElements) && text.boundElements.some(element => element.type === "arrow")) || elements.some(element => element.type === "arrow" && (element.startBinding?.elementId === text.id || element.endBinding?.elementId === text.id));
  if (text.containerId != null || connected) return <section aria-label="候选文字位置" className="shrink-0 border-t p-2 text-xs">绑定文字或附有连线的文字请随容器调整，或使用文字换行入口；此处不单独移动。</section>;
  const x = Number(draft.x), y = Number(draft.y), unsupported = text.locked || text.angle !== 0, valid = !unsupported && [draft.x, draft.y].every(value => value.trim()) && [x, y].every(value => Number.isFinite(value) && Math.abs(value) <= 1e6) && (x !== text.x || y !== text.y);
  return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={resetDraft} label="候选文字位置" title="单独调整当前文字位置" summary="手动调整候选独立文字位置" submitLabel="预览候选文字位置" valid={valid} operations={[{ op: "set_text_position", id: text.id, x, y }]}>
    <p>只移动这一段未绑定文字，保留原文、换行、字号和分组归属；不带动同组素材、锚点或连线。分面内文字须留在原分面，请在预览中检查遮挡。</p>
    {unsupported && <p role="status">请先解锁并取消文字旋转。</p>}
    <div className="flex flex-wrap gap-3">{(["x", "y"] as const).map(key => <label key={key}>{key.toUpperCase()}<input aria-label={`候选文字位置${key.toUpperCase()}`} type="number" step="any" min="-1000000" max="1000000" className="ml-2 w-28 rounded border bg-background p-1" value={draft[key]} disabled={busy || unsupported} onChange={event => setDraft(current => ({ ...current, [key]: event.target.value }))} /></label>)}</div>
  </CanvasCandidateRevision>;
}
