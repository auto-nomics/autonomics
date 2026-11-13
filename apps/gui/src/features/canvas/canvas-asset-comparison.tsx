import { useEffect, useState } from "react";
import { assetGeometry, assetReviewTarget } from "./canvas-asset-review";
import { inspectAssetSource } from "./canvas-source-manifest";
type Target = ReturnType<typeof assetReviewTarget>;
type Source = Awaited<ReturnType<typeof inspectAssetSource>>;
const format = (n: number) => String(Math.round(n * 1000) / 1000);
function AssetDetails({ target, source, title }: { target: Target; source: Source | null; title: string }) {
  if (!target.image || !source) return <section aria-label={title}><h3 className="font-medium">{title}</h3><p>{target.status === "unverified_reference" ? "素材归属不一致，未关联其他图片。" : target.status === "missing_object" ? "此版本没有该对象。" : "此版本没有关联素材。"}</p></section>;
  const geometry = assetGeometry(target.image), history = source.replacement_history;
  return <section aria-label={title} className="min-w-0 space-y-1 break-words"><h3 className="font-medium">{title}</h3>
    <p>素材：{source.name}</p><p>作者：{source.authors.join("、") || "未记录"}</p><p>许可：{source.licenses.join("、") || "未记录"}</p>
    <p>位置与尺寸（约）：{geometry.position ? `${format(geometry.position[0])}, ${format(geometry.position[1])} · ${format(geometry.position[2])} × ${format(geometry.position[3])}` : "无法解释"}</p>
    <p>旋转：{geometry.angle === null ? "未记录" : `${format(geometry.angle * 180 / Math.PI)}°`}</p>
    <p>镜像：{!geometry.scale ? "无法解释" : geometry.scale.every(v => v === 1) ? "无" : [geometry.scale[0] === -1 ? "水平" : "", geometry.scale[1] === -1 ? "垂直" : ""].filter(Boolean).join("、")}</p>
    <p>裁剪：{geometry.crop === "uncropped" ? "未裁剪" : geometry.fractions ? `相对区域 ${geometry.fractions.map(v => `${format(v * 100)}%`).join(" / ")}` : "无法解释裁剪记录"}</p>
    <p>文件核对：{source.file_status === "missing" ? "文件缺失" : source.file_status === "invalid_encoding" ? "文件编码异常" : source.hash_matches_recorded === true ? "与记录哈希一致" : source.hash_matches_recorded === false ? "与记录哈希不符" : "文件可用，记录不足以核对"}</p>
    {source.source_urls.slice(0, 2).map(url => <p key={url}><a href={url} target="_blank" rel="noreferrer" className="break-all underline">来源链接</a></p>)}{source.source_urls.length > 2 && <p>另有 {source.source_urls.length - 2} 条来源，见素材来源导出。</p>}
    <details><summary className="cursor-pointer">换源记录（{history.total ?? "格式无法解释"}）</summary><div className="space-y-2 pl-2">{history.entries.slice(-5).map(entry => <article key={entry.index} className="border-l pl-2"><p>历史素材：{entry.previous_source.name}</p><p>历史作者：{entry.previous_source.authors.join("、") || "未记录"}</p><p>历史许可：{entry.previous_source.licenses.join("、") || "未记录"}</p><p>换源结果文件：{entry.result_file.file_status === "missing" ? "缺失" : entry.result_file.hash_matches_file_id === true ? "与文件指纹一致" : entry.result_file.hash_matches_file_id === false ? "与文件指纹不符" : "未完成核对"}</p><p>旧文件：{entry.previous_source.file_status === "missing" ? "缺失" : entry.previous_source.hash_matches_recorded === true ? "与记录哈希一致" : entry.previous_source.hash_matches_recorded === false ? "与记录哈希不符" : "未完成核对"}</p>{entry.metadata_status === "unsupported" && <p>此条历史记录格式无法解释。</p>}</article>)}{history.status === "not_recorded" && <p>未记录换源历史。</p>}{history.status === "unsupported" && <p>历史记录格式无法解释。</p>}{(history.total ?? 0) > 5 && <p>仅显示最近 5 条，完整导出仍有数量限制；原始记录见源文件。</p>}</div></details>
  </section>;
}
export function CanvasAssetComparison({ before, after, focusedId, comparisonLabel }: { before: unknown; after: unknown; focusedId: string; comparisonLabel: string }) {
  const [expanded, setExpanded] = useState(false);
  const [observed, setObserved] = useState<{ before: unknown; after: unknown; focusedId: string; sources?: [Source | null, Source | null]; error?: string }>();
  const previous = assetReviewTarget(before, focusedId), candidate = assetReviewTarget(after, focusedId);
  const visible = previous.image || candidate.image || previous.status === "unverified_reference" || candidate.status === "unverified_reference";
  useEffect(() => {
    if (!expanded || !visible) return;
    let cancelled = false;
    const load = (target: Target) => target.image ? inspectAssetSource(target.image, target.files) : Promise.resolve(null);
    void Promise.all([load(previous), load(candidate)]).then(sources => { if (!cancelled) setObserved({ before, after, focusedId, sources }); }).catch(error => { if (!cancelled) setObserved({ before, after, focusedId, error: String(error) }); });
    return () => { cancelled = true; };
  }, [expanded, before, after, focusedId, Boolean(visible)]);
  if (!visible) return null;
  const current = observed && observed.before === before && observed.after === after && observed.focusedId === focusedId ? observed : undefined;
  return <details aria-label="所选素材核对" className="shrink-0 border-b text-xs" onToggle={event => setExpanded(event.currentTarget.open)}><summary className="cursor-pointer p-2">核对所选素材的来源与变换</summary><div className="max-h-56 space-y-2 overflow-auto p-2" data-asset-review-id={focusedId}>
    <p>对照：{comparisonLabel}。来源为已记录信息，权属和科学适配性尚未核验；裁剪比例相同不代表生物学区域对应。</p>
    {current?.error ? <p role="alert">读取素材信息失败：{current.error}</p> : current?.sources ? <div className="grid grid-cols-2 gap-4"><AssetDetails target={previous} source={current.sources[0]} title="对照素材信息" /><AssetDetails target={candidate} source={current.sources[1]} title="候选素材信息" /></div> : <p role="status">正在核对所选素材…</p>}
  </div></details>;
}
