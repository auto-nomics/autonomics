import { Button } from "@/components/ui/button";
import { useCandidateDraft } from "./canvas-candidate-drafts";
import { CanvasCandidateRevision, type CandidateRevisionProps } from "./canvas-candidate-revision";
export function CanvasCandidateArtwork(props: CandidateRevisionProps & { focusedId?: string }) {
  const { candidate, focusedId, busy } = props;
  const elements = (candidate.document.elements ?? []).filter(element => !element.isDeleted), focused = elements.filter(element => element.id === focusedId), selected = focused.length === 1 ? focused[0] : undefined;
  const images = selected ? elements.filter(element => element.type === "image" && (element.id === selected.id || (element.id === selected.customData?.visualAssetElementId && element.customData?.semanticAnchorId === selected.id))) : [];
  const image = images.length === 1 && images[0].type === "image" ? images[0] : undefined;
  const owners = image ? elements.filter(element => element.id === image.customData?.semanticAnchorId && element.customData?.visualAssetElementId === image.id) : [];
  const anchor = owners.length === 1 ? owners[0] : undefined;
  const draftKey = ["artwork-scale", image?.id ?? ""], [factor, setFactor, resetDraft] = useCandidateDraft(candidate, draftKey, "1");
  if (!image || !anchor || !image.fileId || !candidate.document_sha256) return null;
  const number = Number(factor), valid = !image.locked && !anchor.locked && factor.trim().length > 0 && Number.isFinite(number) && number > 0 && number !== 1;
  const group=anchor.customData?.visualAnchorFit?.scope==='illustration_group';
  const width = (group?anchor.width:image.width) * number, height = (group?anchor.height:image.height) * number;
  return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={resetDraft} label="候选插图等比例缩放" title={group?"等比例缩放整个组合":"等比例缩放当前插图"} summary="手动等比例缩放候选插图" submitLabel="预览候选插图缩放" valid={valid} operations={[{ op: "scale_illustration", id: image.id, file_id: image.fileId, factor: number }]}>
    <p>{group?"保持组合中心和各图元的相对布局，等比例缩放主图与全部附属图元，更新整体贴合记录和连线。所有成员须有当前文件的有效贴合快照。":"保持插图中心和宽高比，沿用已核对的贴合范围并更新语义锚点与相连连线。"}原始素材、裁剪、镜像和来源保留；文字标签保持原位，请在预览中检查遮挡。</p>
    <p>缺少有效贴合范围、已超出原分面或连接关系不支持时会拒绝生成，请交给 Agent 核对后处理。</p>
    {(image.locked || anchor.locked) && <p role="status">请先解锁插图与语义组件。</p>}
    <label className="block">缩放倍数<input aria-label="候选插图缩放倍数" type="number" min="0" step="any" className="ml-2 w-28 rounded border bg-background p-1" value={factor} disabled={busy || image.locked || anchor.locked} onChange={event => setFactor(event.target.value)} /></label>
    <div className="flex gap-2"><Button type="button" size="sm" variant="outline" disabled={busy || image.locked || anchor.locked} onClick={() => setFactor("0.9")}>缩小 10%</Button><Button type="button" size="sm" variant="outline" disabled={busy || image.locked || anchor.locked} onClick={() => setFactor("1.1")}>放大 10%</Button></div>
    {valid && Number.isFinite(width) && Number.isFinite(height) && <p aria-label="插图缩放后尺寸">预计尺寸：{width.toFixed(2)} × {height.toFixed(2)} 画布单位。</p>}
  </CanvasCandidateRevision>;
}
