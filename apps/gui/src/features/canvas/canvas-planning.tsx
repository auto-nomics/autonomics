import { useEffect, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
export type PlanningNode = { id: string; label: string; shape: "rectangle" | "ellipse"; evidence: string[]; column?: number; row?: number; panel?: string };
type Edge = { from: string; to: string; label: string; inhibitory: boolean; direction?: "forward" | "bidirectional" | "undirected"; evidence: string[] };
type Panel = { id: string; title: string; column: number; row: number };
export type ScientificBrief = { panels: Panel[]; message: string; audience: string; layout: "grid" | "horizontal" | "vertical"; nodes: PlanningNode[]; edges: Edge[]; assumptions: string[]; relationship_note: string; questions: { id: string; question: string; answer: string; blocking?: boolean }[] };
export type Planning = { version: number; based_on_revision: number; document: ScientificBrief; issues: { field: string; message: string }[]; status: string; proposal_id: string | null };
const empty = (): ScientificBrief => ({ panels: [], message: "", audience: "", layout: "horizontal", nodes: [], edges: [], assumptions: [], relationship_note: "", questions: [] });
const lines = (value: string) => value.split("\n").filter(line => line.trim());
const input = "w-full min-w-0 rounded border bg-background px-2 py-1 text-sm";
function nextId(prefix: string, ids: string[]) { let index = 1; while (ids.includes(`${prefix}${index}`)) index++; return `${prefix}${index}`; }
export function CanvasPlanning({ projectId, canvasId, getRevision, ensureSaved, onProposed, isDirty }: {
  projectId: string; canvasId: string; getRevision: () => number | undefined;
  ensureSaved: () => Promise<boolean>; isDirty: () => boolean; onProposed: (id: string) => void;
}) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [record, setRecord] = useState<Planning | null>(null), [draft, setDraft] = useState<ScientificBrief>();
  const baseline = useRef("");
  const dirty = Boolean(draft && JSON.stringify(draft) !== baseline.current);
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/planning`;
  useEffect(() => { const resource = `canvas-plan:${projectId}:${canvasId}`; setDocumentDirty(resource, open && dirty); return () => setDocumentDirty(resource, false); }, [projectId, canvasId, open, dirty]);
  const discard = () => !dirty || window.confirm("放弃尚未保存的绘图计划修改？");
  function apply(value: Planning | null) { const brief = value ? { ...value.document, panels: value.document.panels ?? [] } : empty(); setRecord(value); baseline.current = JSON.stringify(brief); setDraft(brief); }
  async function load() {
    setBusy(true); setError("");
    try { apply(await gatewayRequest("GET", path) as Planning | null); }
    catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  function update(patch: Partial<ScientificBrief>) { setDraft(current => current ? { ...current, ...patch } : current); }
  function updateNode(index: number, patch: Partial<PlanningNode>) { if (draft) update({ nodes: draft.nodes.map((node, i) => i === index ? { ...node, ...patch } : node) }); }
  function updateEdge(index: number, patch: Partial<Edge>) { if (draft) update({ edges: draft.edges.map((edge, i) => i === index ? { ...edge, ...patch } : edge) }); }
  const status = record?.status === "ready" ? "可生成草图" : record?.status === "proposed" ? "已生成提案" : "待澄清";
  return <Dialog open={open} onOpenChange={value => { if (!busy && (value || discard())) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={busy || getRevision() === undefined} onClick={() => { setOpen(true); setDraft(undefined); void load(); }}>绘图计划</Button>
    <DialogContent showCloseButton={false} className="flex h-[90vh] w-[min(1100px,95vw)] max-w-none flex-col gap-0 overflow-hidden border bg-background p-0 sm:max-w-none">
      <div className="flex items-center justify-between gap-3 border-b p-3"><DialogTitle>科研绘图计划</DialogTitle><Button size="sm" variant="outline" disabled={busy} onClick={() => { if (discard()) setOpen(false); }}>关闭计划</Button></div>
      {error && <p role="alert" className="px-4 pt-3 text-sm text-destructive">{error}</p>}
      <div className="min-h-0 flex-1 overflow-auto p-4">
        <p className="mb-3 text-sm text-muted-foreground">与 Agent 一起明确图意、受众、组件及关系。保存计划后可继续讨论；待澄清项完成后生成可审阅的草图提案。</p>
        {record && <p role="status" className="mb-3 text-sm">计划 v{record.version} · 基于画布 v{record.based_on_revision} · {status}{dirty ? " · 有未保存修改" : ""}{record.document.questions.some(q => q.blocking === false && !q.answer.trim()) ? ` · ${record.document.questions.filter(q => q.blocking === false && !q.answer.trim()).length} 项可选偏好未确认` : ""}</p>}
        {isDirty() && <p className="mb-3 text-sm">画布有未保存修改。保存绘图计划时会先保存图稿，请核对计划与图稿是否一致。</p>}
        {record && record.based_on_revision !== getRevision() && <p className="mb-3 text-sm">画布版本已变化。请核对计划与当前图稿，保存后再生成草图。</p>}
        {record?.issues.length ? <section aria-label="计划待澄清项" className="mb-4 rounded border p-3"><p className="mb-1 text-sm font-medium">已保存计划的待澄清项（保存后重新校验）</p><ul className="list-disc space-y-1 pl-5 text-sm">{record.issues.map((issue, index) => <li key={`${issue.field}:${index}`}>{issue.message}</li>)}</ul></section> : null}
        {!draft ? <p role="status">{busy ? "加载计划…" : "请刷新计划重试。"}</p> : <fieldset disabled={busy} className="flex min-w-0 flex-col gap-4">
          <label className="text-sm font-medium">核心图意<textarea aria-label="核心图意" className={`${input} mt-1 min-h-20`} maxLength={8000} value={draft.message} onChange={event => update({ message: event.target.value })} placeholder="这张图希望读者理解什么？" /></label>
          <div className="grid gap-4 md:grid-cols-2"><label className="text-sm">受众<input aria-label="计划受众" className={`${input} mt-1`} maxLength={1000} value={draft.audience} onChange={event => update({ audience: event.target.value })} /></label><label className="text-sm">构图<select aria-label="计划构图" className={`${input} mt-1`} value={draft.layout} onChange={event => {
            const layout = event.target.value as ScientificBrief["layout"];
            update({ layout, nodes: draft.nodes.map((node, index) => { const { column, row, ...rest } = node; return layout === "grid" ? { ...rest, column: column ?? index % 31, row: row ?? Math.floor(index / 31) } : rest; }) });
          }}><option value="horizontal">水平流程</option><option value="vertical">垂直分层</option><option value="grid">人工网格</option></select></label></div>
          <section aria-label="计划面板" className="space-y-2"><div className="flex items-center justify-between"><h3 className="text-sm font-medium">面板</h3><Button size="sm" variant="outline" disabled={draft.panels.length >= 12} onClick={() => {
            const id = nextId("p", draft.panels.map(p => p.id));
            update({ panels: [...draft.panels, { id, title: "", column: draft.panels.length % 3, row: Math.floor(draft.panels.length / 3) }], nodes: draft.nodes.map(node => !draft.panels.length ? { ...node, panel: id } : node) });
          }}>添加面板</Button></div><p className="text-xs text-muted-foreground">用于比较条件或展示步骤。各面板独立构图；不会推断跨面板关系。添加面板后，请为每个组件指定所属面板。</p>
            {draft.panels.map((panel, index) => <article key={panel.id} className="flex flex-wrap items-end gap-2 rounded border p-2"><label className="min-w-40 flex-1 text-xs">标题<input aria-label={`面板标题 ${index + 1}`} className={input} maxLength={120} value={panel.title} onChange={event => update({ panels: draft.panels.map((p, i) => i === index ? { ...p, title: event.target.value } : p) })} /></label>{([['column', '列'], ['row', '行']] as const).map(([field, title]) => <label key={field} className="w-20 text-xs">{title}<input aria-label={`面板${title} ${index + 1}`} className={input} type="number" min={0} max={10} value={panel[field]} onChange={event => update({ panels: draft.panels.map((p, i) => i === index ? { ...p, [field]: Number(event.target.value) } : p) })} /></label>)}<Button size="sm" variant="ghost" onClick={() => update({ panels: draft.panels.filter(p => p.id !== panel.id), nodes: draft.nodes.map(node => node.panel === panel.id ? { ...node, panel: undefined } : node) })}>移除面板</Button></article>)}
          </section>
          <section aria-label="计划组件" className="space-y-2"><div className="flex items-center justify-between"><h3 className="text-sm font-medium">组件</h3><Button size="sm" variant="outline" disabled={draft.nodes.length >= 100} onClick={() => update({ nodes: [...draft.nodes, { id: nextId("n", draft.nodes.map(n => n.id)), label: "", shape: "rectangle", evidence: [], ...(draft.panels.length ? { panel: draft.panels[0].id } : {}), ...(draft.layout === "grid" ? { column: draft.nodes.length % 31, row: Math.floor(draft.nodes.length / 31) } : {}) }] })}>添加组件</Button></div>
            {draft.nodes.map((node, index) => <article key={node.id} className="space-y-2 rounded border p-2"><div className="flex items-center gap-2"><input aria-label={`组件名称 ${index + 1}`} className={input} maxLength={120} value={node.label} onChange={event => updateNode(index, { label: event.target.value })} /><select aria-label={`组件形状 ${index + 1}`} className="rounded border bg-background p-1 text-sm" value={node.shape} onChange={event => updateNode(index, { shape: event.target.value as PlanningNode["shape"] })}><option value="rectangle">矩形</option><option value="ellipse">椭圆</option></select><Button size="sm" variant="ghost" onClick={() => update({ nodes: draft.nodes.filter(n => n.id !== node.id), edges: draft.edges.filter(e => e.from !== node.id && e.to !== node.id) })}>移除组件</Button></div>
              {draft.panels.length > 0 && <label className="block text-xs">所属面板<select aria-label={`组件面板 ${index + 1}`} className={input} value={node.panel ?? ""} onChange={event => updateNode(index, { panel: event.target.value || undefined })}><option value="">请选择面板</option>{draft.panels.map((panel, i) => <option key={panel.id} value={panel.id}>{panel.title || `未命名面板 ${i + 1}`}</option>)}</select></label>}
              {draft.layout === "grid" && <div className="flex gap-2"><label className="text-xs">列<input aria-label={`组件列 ${index + 1}`} className={input} type="number" min={0} max={30} value={node.column ?? ""} onChange={event => updateNode(index, { column: event.target.value === "" ? undefined : Number(event.target.value) })} /></label><label className="text-xs">行<input aria-label={`组件行 ${index + 1}`} className={input} type="number" min={0} max={30} value={node.row ?? ""} onChange={event => updateNode(index, { row: event.target.value === "" ? undefined : Number(event.target.value) })} /></label></div>}
              <details><summary className="cursor-pointer text-xs text-muted-foreground">依据与来源</summary><textarea aria-label={`组件依据 ${index + 1}`} className={`${input} mt-1`} value={node.evidence.join("\n")} onChange={event => updateNode(index, { evidence: lines(event.target.value) })} placeholder="每行一条依据或来源；没有依据时留空。" /></details></article>)}
          </section>
          <section aria-label="计划关系" className="space-y-2"><div className="flex items-center justify-between"><h3 className="text-sm font-medium">关系</h3><Button size="sm" variant="outline" disabled={draft.nodes.length < 2 || draft.edges.length >= 200} onClick={() => update({ edges: [...draft.edges, { from: draft.nodes[0].id, to: draft.nodes[1].id, label: "", inhibitory: false, evidence: [] }] })}>添加关系</Button></div>
            {draft.edges.map((edge, index) => <article key={index} className="space-y-2 rounded border p-2"><div className="grid items-center gap-2 sm:grid-cols-2">{([['from', '关系来源'], ['to', '关系目标']] as const).map(([field, title]) => <select key={field} aria-label={`${title} ${index + 1}`} className={input} value={edge[field]} onChange={event => updateEdge(index, { [field]: event.target.value })}>{draft.nodes.map((node, i) => <option key={node.id} value={node.id}>{node.label || `未命名组件 ${i + 1}`}</option>)}</select>)}</div><div className="flex items-center gap-2"><input aria-label={`关系说明 ${index + 1}`} className={input} maxLength={120} value={edge.label} onChange={event => updateEdge(index, { label: event.target.value })} /><select aria-label={`关系方向 ${index + 1}`} className={input} value={edge.direction ?? "forward"} disabled={edge.inhibitory} onChange={event => updateEdge(index, { direction: event.target.value as Edge["direction"] })}><option value="forward">单向</option><option value="bidirectional">双向</option><option value="undirected">无向</option></select><label className="flex shrink-0 items-center gap-1 text-xs"><input aria-label={`抑制关系 ${index + 1}`} type="checkbox" disabled={(edge.direction ?? "forward") !== "forward" && !edge.inhibitory} checked={edge.inhibitory} onChange={event => updateEdge(index, { inhibitory: event.target.checked })} />抑制</label><Button size="sm" variant="ghost" onClick={() => update({ edges: draft.edges.filter((_, i) => i !== index) })}>移除关系</Button></div><details><summary className="cursor-pointer text-xs text-muted-foreground">依据与来源</summary><textarea aria-label={`关系依据 ${index + 1}`} className={`${input} mt-1`} value={edge.evidence.join("\n")} onChange={event => updateEdge(index, { evidence: lines(event.target.value) })} /></details></article>)}
            {!draft.edges.length && <label className="block text-sm">组件关系说明<textarea aria-label="组件关系说明" className={`${input} mt-1`} maxLength={4000} value={draft.relationship_note} onChange={event => update({ relationship_note: event.target.value })} placeholder="若仅并列展示多个组件，请明确说明；不要补写未知机制。" /></label>}
          </section>
          <section aria-label="计划问题" className="space-y-2"><div className="flex items-center justify-between"><h3 className="text-sm font-medium">待澄清问题与答案</h3><Button size="sm" variant="outline" disabled={draft.questions.length >= 20} onClick={() => update({ questions: [...draft.questions, { id: nextId("q", draft.questions.map(q => q.id)), question: "", answer: "", blocking: true }] })}>添加问题</Button></div>
            <p className="text-xs text-muted-foreground">物种、机制分支和科学范围等未知项应在生成前确认。语言、配色等可选偏好可留空，不会自动视为已确认。</p>
            {draft.questions.map((question, index) => <article key={question.id} className="space-y-2 rounded border p-2"><label className="flex items-center gap-2 text-xs"><input type="checkbox" aria-label={`生成前必须确认 ${index + 1}`} checked={question.blocking !== false} onChange={event => update({ questions: draft.questions.map((q, i) => i === index ? { ...q, blocking: event.target.checked } : q) })} />生成草图前必须确认</label><input aria-label={`澄清问题 ${index + 1}`} className={input} maxLength={4000} value={question.question} onChange={event => update({ questions: draft.questions.map((q, i) => i === index ? { ...q, question: event.target.value } : q) })} /><textarea aria-label={`问题答案 ${index + 1}`} className={`${input} min-h-20`} maxLength={4000} value={question.answer} placeholder="填写答案后，请核对组件和关系是否已反映答案。" onChange={event => update({ questions: draft.questions.map((q, i) => i === index ? { ...q, answer: event.target.value } : q) })} /><Button size="sm" variant="ghost" onClick={() => update({ questions: draft.questions.filter((_, i) => i !== index) })}>移除问题</Button></article>)}
          </section>
          <label className="text-sm">假设与待核对事项<textarea aria-label="计划假设" className={`${input} mt-1 min-h-20`} value={draft.assumptions.join("\n")} onChange={event => update({ assumptions: lines(event.target.value) })} placeholder="每行一项，明确哪些内容需要核对。" /></label>
        </fieldset>}
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2 border-t p-3"><Button size="sm" variant="outline" disabled={busy} onClick={() => { if (discard()) void load(); }}>刷新计划</Button><Button size="sm" variant="outline" disabled={busy || !draft || !dirty && !isDirty() && Boolean(record && record.based_on_revision === getRevision())} onClick={async () => {
        if (!draft || busy) return; setBusy(true); setError("");
        try { if (!await ensureSaved()) throw new Error("请先解决画布保存错误。"); apply(await gatewayRequest("PUT", path, { expected_revision: getRevision(), expected_plan_version: record?.version ?? 0, document: draft }) as Planning); }
        catch (cause) { setError(`保存计划失败：${String(cause)}`); } finally { setBusy(false); }
      }}>{busy ? "处理中…" : "保存绘图计划"}</Button><Button size="sm" disabled={busy || dirty || isDirty() || !record || record.issues.length > 0 || record.based_on_revision !== getRevision()} onClick={async () => {
        if (!record || busy) return; setBusy(true); setError("");
        try { const proposal = await gatewayRequest("POST", `${path}/sketch`, { expected_revision: getRevision(), expected_plan_version: record.version, summary: "根据绘图计划生成草图" }) as { id: string }; setOpen(false); onProposed(proposal.id); }
        catch (cause) { setError(String(cause)); } finally { setBusy(false); }
      }}>生成草图提案</Button></div>
    </DialogContent>
  </Dialog>;
}
