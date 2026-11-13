import { Button } from "@/components/ui/button";
import { useCandidateDraft } from "./canvas-candidate-drafts";
import { CanvasCandidateRevision, type CandidateRevisionProps } from "./canvas-candidate-revision";
import { relationSources } from "./canvas-source-manifest";
export function CanvasCandidateEvidence(props: CandidateRevisionProps & { focusedId?: string }) {
  const { candidate, focusedId, busy } = props;
  const elements = (candidate.document.elements ?? []).filter(element => !element.isDeleted), focused = elements.filter(element => element.id === focusedId), selected = focused.length === 1 ? focused[0] : undefined;
  const matches = selected ? elements.filter(element => element.id === (selected.type === "text" ? selected.containerId : selected.id)) : [];
  const arrow = matches.length === 1 && matches[0].type === "arrow" ? matches[0] : undefined, data = arrow?.customData, raw = data?.evidence ?? null;
  const editable = raw === null || (Array.isArray(raw) && raw.length <= 100 && raw.every(value => typeof value === "string" && value.length <= 2000 && !/^\s*data:/i.test(value)));
  const initial: string[] = editable && Array.isArray(raw) ? [...raw] as string[] : [];
  const draftKey = ["evidence", arrow?.id ?? ""], [references, setReferences, resetDraft] = useCandidateDraft(candidate, draftKey, initial);
  if (!arrow || !candidate.document_sha256 || (data != null && (typeof data !== "object" || Array.isArray(data))) || data?.annotationKind != null || data?.inset != null) return null;
  const report = relationSources({ elements: [arrow] }).relations[0];
  const shown = report.references.slice(0, 20);
  const recorded = <section aria-label="候选已记录依据" className="space-y-1"><p className="font-medium">已记录依据</p>{!report.reference_count && <p>{raw === null ? "未记录依据。" : report.evidence_status === "unsupported" ? "当前依据格式未支持，请核对完整源文件。" : "已记录空依据列表。"}</p>}{shown.map((reference, index) => <div key={index} className="whitespace-pre-wrap break-all border-l pl-2">{reference.kind === "text" ? reference.text : reference.kind === "structured" ? Object.entries(reference.fields ?? {}).map(([key, value]) => <p key={key}>{key}: {value}</p>) : "此项格式未支持，请核对源文件。"}{reference.truncated && <p>此项展示已截断。</p>}{reference.omitted_fields && <p>部分结构化字段未展示。</p>}</div>)}{report.reference_count > shown.length && <p>共 {report.reference_count} 条，仅显示前 {shown.length} 条。</p>}</section>;
  if (!editable) return <details aria-label="候选关系依据" className="shrink-0 border-t text-xs"><summary className="cursor-pointer p-2">查看当前关系依据</summary><div className="space-y-2 p-2">{recorded}<p>包含结构化、超限或未支持的依据，本入口只读，避免覆盖完整信息。请通过源文档或 Agent 核对后修订。记录存在不代表真实性或科学支持已验证。</p></div></details>;
  const valid = !arrow.locked && references.length <= 100 && references.every(value => value.trim().length > 0 && value.length <= 2000 && !/^\s*data:/i.test(value)) && JSON.stringify(references) !== JSON.stringify(initial);
  return <CanvasCandidateRevision {...props} draftKey={draftKey} onCommitted={resetDraft} label="候选关系依据" title="查看与修订关系依据" summary="手动修订候选关系依据" submitLabel="预览候选依据修订" valid={valid} operations={[{ op: "set_relation_evidence", id: arrow.id, expected_evidence: raw, evidence: references }]}>
    {recorded}
    <p>每项填写一条引用或来源说明，并核对它是否支持当前关系。只替换此关系的依据，保留方向、文字、连接对象及历史计划；不核验引用真实性或科学支持。</p>
    {arrow.locked && <p role="status">请先解锁连线，再修订依据。</p>}
    {references.map((reference, index) => <div key={index} className="flex items-start gap-2"><textarea aria-label={`候选关系引用 ${index + 1}`} className="min-h-20 min-w-0 flex-1 rounded border bg-background p-2" value={reference} maxLength={2000} disabled={busy || arrow.locked} onChange={event => setReferences(current => current.map((value, position) => position === index ? event.target.value : value))} /><Button type="button" size="sm" variant="outline" aria-label={`移除候选引用 ${index + 1}`} disabled={busy || arrow.locked} onClick={() => setReferences(current => current.filter((_, position) => position !== index))}>移除</Button></div>)}
    <Button type="button" size="sm" variant="outline" disabled={busy || arrow.locked || references.length >= 100} onClick={() => setReferences(current => [...current, ""])}>添加候选引用</Button>
    {initial.length > 0 && references.length === 0 && <p role="status">此修订将清空该关系的依据列表；预览后再采用。</p>}
  </CanvasCandidateRevision>;
}
