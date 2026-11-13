import { useCandidateDraft } from "./canvas-candidate-drafts";
import { CanvasCandidateRevision, type CandidateRevisionProps } from "./canvas-candidate-revision";
export function CanvasCandidateImage(props: CandidateRevisionProps & { focusedId?: string }) {
  const { candidate, focusedId, busy } = props;
  const elements = (candidate.document.elements ?? []).filter(element => !element.isDeleted), matches = elements.filter(element => element.id === focusedId);
  const image = matches.length === 1 && matches[0].type === "image" ? matches[0] : undefined;
  const draftKey = ["image-geometry", image?.id ?? ""], [draft, setDraft, resetDraft] = useCandidateDraft(candidate, draftKey, { x: String(image?.x ?? ""), y: String(image?.y ?? ""), width: String(image?.width ?? ""), height: String(image?.height ?? ""), proportional: true });
  if (!image || !candidate.document_sha256) return null;
  const owned = image.customData?.semanticAnchorId != null || image.customData?.inset != null || (image.boundElements?.length ?? 0) > 0 || elements.some(element => element.customData?.visualAssetElementId === image.id || element.type === "text" && element.containerId === image.id || element.type === "arrow" && (element.startBinding?.elementId === image.id || element.endBinding?.elementId === image.id));
  if (owned) return null;
  const keys = ["x", "y", "width", "height"] as const, values = { x: Number(draft.x), y: Number(draft.y), width: Number(draft.width), height: Number(draft.height) };
  const valid = !image.locked && keys.every(key => draft[key].trim() && Number.isFinite(values[key]) && Math.abs(values[key]) <= 1e6) && values.width > 0 && values.height > 0 && keys.some(key => values[key] !== image[key]);
  const update = (key: typeof keys[number], value: string) => setDraft(current => {
    const next = { ...current, [key]: value }, number = Number(value);
    if (current.proportional && (key === "width" || key === "height") && value.trim() && Number.isFinite(number) && number > 0 && image.width > 0 && image.height > 0) {
      if (key === "width") next.height = String(number * image.height / image.width);
      else next.width = String(number * image.width / image.height);
    }
    return next;
  });
  return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={resetDraft} label="候选独立图片位置与尺寸" title="调整独立图片位置与尺寸" summary="手动调整候选独立图片" submitLabel="预览候选图片调整" valid={valid} operations={[{ op: "set_image_geometry", id: image.id, ...values }]}>
    <p>只调整当前独立图片的显示框；保持原始素材、裁剪、镜像、旋转和来源，其他同组对象不移动。请在预览中核对分面归属与遮挡。</p>
    {image.locked && <p role="status">请先解锁图片。</p>}
    <label className="flex items-center gap-2"><input type="checkbox" aria-label="图片保持宽高比" checked={draft.proportional} disabled={busy || image.locked} onChange={event => setDraft(current => ({ ...current, proportional: event.target.checked }))} />保持原图片宽高比（修改宽或高时联动）</label>
    <div className="flex flex-wrap gap-3">{keys.map(key => <label key={key}>{({ x: "X", y: "Y", width: "宽度", height: "高度" })[key]}<input aria-label={`候选图片${({ x: "X", y: "Y", width: "宽度", height: "高度" })[key]}`} type="number" step="any" min={key === "width" || key === "height" ? 0 : -1000000} max="1000000" className="ml-2 w-28 rounded border bg-background p-1" value={draft[key]} disabled={busy || image.locked} onChange={event => update(key, event.target.value)} /></label>)}</div>
  </CanvasCandidateRevision>;
}
