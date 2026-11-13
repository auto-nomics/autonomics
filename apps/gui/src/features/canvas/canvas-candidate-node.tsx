import { useCandidateDraft } from "./canvas-candidate-drafts";
import { CanvasCandidateRevision, type CandidateRevisionProps } from "./canvas-candidate-revision";
export function CanvasCandidateNode(props: CandidateRevisionProps & { focusedId?: string }) {
  const { candidate, focusedId, busy } = props;
  const elements = (candidate.document.elements ?? []).filter(element => !element.isDeleted), focused = elements.filter(element => element.id === focusedId);
  const focusedElement = focused.length === 1 ? focused[0] : undefined;
  const nodes = focusedElement ? elements.filter(element => element.id === (focusedElement.type === "text" ? focusedElement.containerId : focusedElement.id)) : [];
  const node = nodes.length === 1 && ["rectangle", "ellipse", "diamond"].includes(nodes[0].type) ? nodes[0] : undefined;
  const original = node ? [node.x, node.y, node.width, node.height] : [];
  const draftKey = ["node", node?.id ?? ""];
  const [values, setValues, resetDraft] = useCandidateDraft(candidate, draftKey, original.map(String));
  if (!node || !candidate.document_sha256) return null;
  const owned = ["visualAssetElementId", "visualLabelIds", "visualAssetGroupId", "semanticAnchorId", "inset", "annotationKind"].some(key => node.customData?.[key] != null);
  if (owned) {
    const illustrated = typeof node.customData?.visualAssetElementId === "string" && node.customData?.inset == null && node.customData?.annotationKind == null;
    if (!illustrated) return <section aria-label="候选组件位置与尺寸" className="shrink-0 border-t p-2 text-xs"><p>此组件使用素材或注释专用布局，请通过对应编辑工具或 Agent 调整。</p></section>;
    const position = values.slice(0, 2).map(Number);
    const valid = !node.locked && node.angle === 0 && position.length === 2 && values.slice(0, 2).every(value => value.trim().length > 0) && position.every(value => Number.isFinite(value) && Math.abs(value) <= 1e6) && position.some((value, index) => value !== original[index]);
    return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={resetDraft} label="候选素材组件位置" title="整体移动素材组件" summary="手动整体移动候选素材组件" submitLabel="预览素材组件移动" valid={valid} operations={[{ op: "translate", element_ids: [node.id], dx: position[0] - node.x, dy: position[1] - node.y }]}>
      <p>按画布单位整体移动组件、素材和标签，并更新相连连线。保留尺寸、文字和来源；请预览核对遮挡与连线路径。组件须留在原分面。</p>
      {(node.locked || node.angle !== 0) && <p role="status">请先解锁并取消组件旋转，再调整位置。</p>}
      <div className="flex flex-wrap gap-3">{(["X", "Y"] as const).map((name, index) => <label key={name}>{name}<input aria-label={`候选素材组件${name}`} type="number" step="any" min="-1000000" max="1000000" className="ml-2 w-28 rounded border bg-background p-1" value={values[index] ?? ""} disabled={busy || Boolean(node.locked) || node.angle !== 0} onChange={event => setValues(current => current.map((value, position) => position === index ? event.target.value : value))} /></label>)}</div>
    </CanvasCandidateRevision>;
  }
  const unsupported = node.locked || node.angle !== 0;
  const numbers = values.map(Number), valid = !unsupported && values.length === 4 && values.every(value => value.trim().length > 0) && numbers.every(value => Number.isFinite(value) && Math.abs(value) <= 1e6) && numbers[2] >= 1 && numbers[3] >= 1 && numbers.some((value, index) => value !== original[index]);
  return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={resetDraft} label="候选组件位置与尺寸" title="调整当前组件的位置与尺寸" summary="手动调整候选组件位置与尺寸" submitLabel="预览候选组件调整" valid={valid} operations={[{ op: "set_node_geometry", id: node.id, x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3] }]}>
    <p>按画布单位调整。保留标签原文和字号，重新换行并更新相连的原生连线；请在预览中检查间距和遮挡。分面内的组件须留在原分面。</p>
    {unsupported && <p role="status">请先解锁并取消组件旋转，再调整位置与尺寸。</p>}
    <div className="flex flex-wrap gap-3">{(["X", "Y", "宽度", "高度"] as const).map((name, index) => <label key={name}>{name}<input aria-label={`候选组件${name}`} type="number" step="any" min={index >= 2 ? 1 : -1000000} max="1000000" className="ml-2 w-28 rounded border bg-background p-1" value={values[index] ?? ""} disabled={busy || unsupported} onChange={event => setValues(current => current.map((value, position) => position === index ? event.target.value : value))} /></label>)}</div>
  </CanvasCandidateRevision>;
}
