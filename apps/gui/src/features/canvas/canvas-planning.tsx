import { mergePlanDraft, type PlanMergeConflict } from "./canvas-plan-merge";
import { planDifferenceLabel, planValuePreview, samePlanValue } from "./canvas-plan-diff";
import { CanvasPlanDraftChanges } from "./canvas-plan-draft-changes";
import { CanvasPlanChanges } from "./canvas-plan-changes";
import { CanvasPlanAgent } from "./canvas-plan-agent";
import { useEffect, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { CanvasPlanningEvidence } from "./canvas-planning-evidence";
export type PlanningNode = { id: string; label: string; shape: "rectangle" | "ellipse"; evidence: string[]; column?: number; row?: number; panel?: string };
type Edge = { from: string; to: string; label: string; inhibitory: boolean; direction?: "forward" | "bidirectional" | "undirected"; line_style?: "solid" | "dashed" | "dotted"; evidence: string[] };
type Panel = { id: string; title: string; column: number; row: number };
export type ScientificBrief = { panels: Panel[]; message: string; audience: string; layout: "grid" | "horizontal" | "vertical"; nodes: PlanningNode[]; edges: Edge[]; assumptions: string[]; relationship_note: string; questions: { id: string; question: string; answer: string; blocking?: boolean }[] };
type LayoutBounds = { x: number; y: number; width: number; height: number };
export type PlanLabelContext = { kind: "node"; node_id: string; panel_id: string | null } | { kind: "relation"; from_node_id: string; to_node_id: string; from_panel_id: string | null; to_panel_id: string | null; cross_panel: boolean | null } | { kind: "panel"; panel_id: string };
export type LayoutReview = { status: string; scope: string; bounds: LayoutBounds | null; aspect_ratio: number | null; panels: { id: string; bounds: LayoutBounds }[]; error: string | null; long_labels?: { plan_field?: string | null; context?: PlanLabelContext | null; preview: string; lines: number; width: number; height: number }[]; long_label_count?: number; long_labels_omitted?: number };
export type Planning = { version: number; based_on_revision: number; document: ScientificBrief; issues: { field: string; message: string }[]; status: string; proposal_id: string | null; layout_review?: LayoutReview | null };
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
  const [externalVersion,setExternalVersion]=useState(0);
  const [mergeConflicts,setMergeConflicts]=useState<{version:number;count:number;details:PlanMergeConflict[];base:ScientificBrief;local:ScientificBrief;remote:Planning}|null>(null);
  const [mergeChoices,setMergeChoices]=useState<Record<string,"local"|"remote">>({});
  const stale=externalVersion>(record?.version??0);
  const planBody = useRef<HTMLDivElement>(null);
  const edgeInputs = useRef(new Map<number, HTMLInputElement>());
  const dirty = Boolean(draft && baseline.current && !samePlanValue(draft,JSON.parse(baseline.current)));
  const handoffState=useRef({version:record?.version,dirty,revision:getRevision(),sceneDirty:isDirty()});
  handoffState.current={version:record?.version,dirty,revision:getRevision(),sceneDirty:isDirty()};
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/planning`;
  useEffect(() => {
    if(!open||busy)return;
    let active=true,pending=false;
    const check=async()=>{
      if(pending||document.visibilityState!=="visible")return;
      pending=true;
      try {
        const latest=await gatewayRequest("GET",path) as Planning|null;
        if(active&&latest&&latest.version>(record?.version??0))setExternalVersion(latest.version);
      } catch { /* A transient check failure never replaces a local draft. Explicit writes retain server CAS checks. */ }
      finally {pending=false;}
    };
    const interval=window.setInterval(()=>{void check();},5000);
    const visible=()=>{void check();};
    window.addEventListener("focus",visible);document.addEventListener("visibilitychange",visible);
    return ()=>{active=false;window.clearInterval(interval);window.removeEventListener("focus",visible);document.removeEventListener("visibilitychange",visible);};
  },[open,busy,path,record?.version]);
  useEffect(() => { const resource = `canvas-plan:${projectId}:${canvasId}`; setDocumentDirty(resource, open && dirty); return () => setDocumentDirty(resource, false); }, [projectId, canvasId, open, dirty]);
  const discard = () => !dirty || window.confirm("放弃尚未保存的绘图计划修改？");
  function apply(value: Planning | null) { const brief = value ? { ...value.document, panels: value.document.panels ?? [] } : empty(); setRecord(value); setExternalVersion(0); setMergeConflicts(null); setMergeChoices({}); baseline.current = JSON.stringify(brief); setDraft(brief); }
  async function load() {
    setBusy(true); setError("");
    try { apply(await gatewayRequest("GET", path) as Planning | null); }
    catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  async function mergeLatest() {
    if(!draft||!record||!stale||busy)return;
    setBusy(true);setError("");setMergeConflicts(null);setMergeChoices({});
    try {
      const latest=await gatewayRequest("GET",path) as Planning|null;
      if(!latest||latest.version<=record.version)throw new Error("没有可合并的新版本，请重新刷新核对。");
      setExternalVersion(latest.version);
      const merged=mergePlanDraft(JSON.parse(baseline.current) as ScientificBrief,draft,{...latest.document,panels:latest.document.panels??[]});
      if(!merged.document){setMergeConflicts({version:latest.version,count:merged.conflictCount,details:merged.details,base:JSON.parse(baseline.current) as ScientificBrief,local:structuredClone(draft),remote:structuredClone(latest)});throw new Error(`合并存在 ${merged.conflictCount} 处冲突：${merged.conflicts.slice(0,10).map(planDifferenceLabel).join("、")}${merged.conflictCount>10?"…":""}。当前草稿已保留。`);}
      apply(latest);setDraft(merged.document);
    } catch(cause){setError(String(cause));} finally {setBusy(false);}
  }
  async function resolveMerge() {
    const snapshot=mergeConflicts;
    if(!snapshot||!draft||busy||snapshot.count!==snapshot.details.length||!snapshot.details.every(item=>mergeChoices[item.field]))return;
    if(!samePlanValue(draft,snapshot.local)){setError("本地草稿已变化，请重新合并检查后再取舍。");return;}
    setBusy(true);setError("");
    try {
      const latest=await gatewayRequest("GET",path) as Planning|null;
      if(latest&&latest.version>(record?.version??0))setExternalVersion(latest.version);
      if(!latest||latest.version!==snapshot.version||latest.based_on_revision!==snapshot.remote.based_on_revision||!samePlanValue(latest.document,snapshot.remote.document))throw new Error("远端计划已变化，请重新合并检查后再取舍。");
      const merged=mergePlanDraft(snapshot.base,snapshot.local,{...latest.document,panels:latest.document.panels??[]},mergeChoices);
      if(!merged.document)throw new Error("仍有未处理的冲突，请重新核对。");
      apply(latest);setDraft(merged.document);
    }catch(cause){setError(String(cause));}finally{setBusy(false);}
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
      <div ref={planBody} className="min-h-0 flex-1 overflow-auto p-4">
        <p className="mb-3 text-sm text-muted-foreground">与 Agent 一起明确图意、受众、组件及关系。保存计划后可继续讨论；待澄清项完成后生成可审阅的草图提案。</p>
        {stale&&<div role="status" aria-label="计划远端版本变化" className="mb-3 rounded border p-3 text-sm"><p>已有新的计划 v{externalVersion}，当前显示 v{record?.version??0}。请刷新或合并后再保存、生成草图或交给 Agent 审阅；当前未保存草稿仍保留。</p>{dirty&&record&&<><Button size="sm" variant="outline" className="mt-2" disabled={busy} onClick={()=>void mergeLatest()}>合并未冲突修改</Button><p className="mt-1 text-xs text-muted-foreground">以当前已保存版本为共同基线合并字段；冲突时保留草稿。合并后仍须核对关系与依据，再保存为新版本。</p></>}</div>}
        {mergeConflicts&&<section aria-label="计划合并冲突详情" className="mb-3 rounded border p-3 text-sm"><p>合并检查时的冲突快照 · 本地基于 v{record?.version??0} · 远端 v{mergeConflicts.version}</p><p className="text-xs text-muted-foreground">当前草稿未改动。下表保留检查时的值，之后的编辑不改变此快照；调整草稿后可重新合并检查。</p><div className="mt-2 overflow-x-auto"><table className="w-full table-fixed text-left text-xs"><thead><tr><th className="w-1/5 p-2">位置</th><th className="p-2">共同版本</th><th className="p-2">本地草稿</th><th className="p-2">远端版本</th><th className="p-2">取舍</th></tr></thead><tbody>{mergeConflicts.details.map(conflict=><tr key={conflict.field} className="border-t"><th className="break-words p-2 align-top">{planDifferenceLabel(conflict.field)}</th>{(["base","local","remote"] as const).map(key=><td key={key} className="whitespace-pre-wrap break-words p-2 align-top">{planValuePreview(conflict[key],conflict.field)}</td>)}<td className="p-2 align-top"><select aria-label={`冲突 ${planDifferenceLabel(conflict.field)} 取舍`} className="w-full rounded border bg-background p-1" disabled={busy} value={mergeChoices[conflict.field]??""} onChange={event=>setMergeChoices(current=>{const next={...current};if(event.target.value)next[conflict.field]=event.target.value as "local"|"remote";else delete next[conflict.field];return next;})}><option value="">请选择</option><option value="local">保留本地</option><option value="remote">采用远端</option></select></td></tr>)}</tbody></table></div>{mergeConflicts.count>mergeConflicts.details.length&&<p>另有 {mergeConflicts.count-mergeConflicts.details.length} 处未显示，请先缩小冲突范围。</p>}<Button size="sm" variant="outline" className="mt-2" disabled={busy||mergeConflicts.count!==mergeConflicts.details.length||!mergeConflicts.details.every(item=>mergeChoices[item.field])} onClick={()=>void resolveMerge()}>按取舍载入合并草稿</Button><p className="mt-1 text-xs text-muted-foreground">列表取舍会采用完整列表，包括预览中截断的部分。载入前会重新核对本地草稿与远端版本，仍需另行检查和保存。</p></section>}
        {record && <p role="status" className="mb-3 text-sm">计划 v{record.version} · 基于画布 v{record.based_on_revision} · {status}{dirty ? " · 有未保存修改" : ""}{record.document.questions.some(q => q.blocking === false && !q.answer.trim()) ? ` · ${record.document.questions.filter(q => q.blocking === false && !q.answer.trim()).length} 项可选偏好未确认` : ""}</p>}
        {dirty&&draft&&<CanvasPlanDraftChanges baseline={baseline.current} draft={draft} version={record?.version??0} />}
        {record && <CanvasPlanChanges key={`${projectId}:${canvasId}:${record.version}`} projectId={projectId} canvasId={canvasId} record={record} dirty={dirty} busy={busy||stale} onRestore={document => { if (!dirty && !busy && !stale) setDraft(structuredClone({ ...document, panels: document.panels ?? [] })); }} />}
        {record?.layout_review && <section aria-label="计划布局诊断" className="mb-3 rounded border p-3 text-sm"><p className="font-medium">已保存计划 v{record.version} 的编译布局</p><p className="text-xs text-muted-foreground">不含后续图注和素材，不代表像素碰撞或科学内容已验证。{dirty ? " 草稿已修改，保存后更新尺寸。" : ""}</p>{record.layout_review.bounds && record.layout_review.aspect_ratio != null ? <><p>宽 {record.layout_review.bounds.width.toFixed(0)} × 高 {record.layout_review.bounds.height.toFixed(0)} · 宽高比 {record.layout_review.aspect_ratio.toFixed(2)}</p>{record.layout_review.panels.length > 0 && <svg role="img" aria-label="已保存分面位置示意" className="my-2 h-40 w-full rounded bg-muted/30" viewBox={`${record.layout_review.bounds.x} ${record.layout_review.bounds.y} ${record.layout_review.bounds.width} ${record.layout_review.bounds.height}`} preserveAspectRatio="xMidYMid meet">{record.layout_review.panels.map((panel, index) => { const title = record.document.panels.find(item => item.id === panel.id)?.title || panel.id; return <g key={panel.id}><title>{title}</title><rect x={panel.bounds.x} y={panel.bounds.y} width={panel.bounds.width} height={panel.bounds.height} fill="currentColor" fillOpacity={0.08} stroke="currentColor" strokeWidth={1} vectorEffect="non-scaling-stroke" /><text x={panel.bounds.x + panel.bounds.width / 2} y={panel.bounds.y + panel.bounds.height / 2} fill="currentColor" textAnchor="middle" dominantBaseline="central" fontSize={Math.min(panel.bounds.width / 6, panel.bounds.height / 3, (record.layout_review?.bounds?.height ?? panel.bounds.height) / 12)}>{index + 1}</text></g>; })}</svg>}{record.layout_review.panels.length > 0 && <ul className="mt-1 list-disc pl-4">{record.layout_review.panels.map((panel, index) => <li key={panel.id}>{index + 1}. {record.document.panels.find(item => item.id === panel.id)?.title || panel.id}：({panel.bounds.x.toFixed(0)}, {panel.bounds.y.toFixed(0)}) · {panel.bounds.width.toFixed(0)} × {panel.bounds.height.toFixed(0)}</li>)}</ul>}</> : <p>暂不能编译：{record.layout_review.error ?? "请补充计划结构。"}</p>}</section>}
        {isDirty() && <p className="mb-3 text-sm">画布有未保存修改。保存绘图计划时会先保存图稿，请核对计划与图稿是否一致。</p>}
        {record && record.based_on_revision !== getRevision() && <p className="mb-3 text-sm">画布版本已变化。请核对计划与当前图稿，保存后再生成草图。</p>}
        {Boolean(record?.layout_review?.long_label_count) && <section aria-label="计划长标签提示" className="mb-3 rounded border p-3 text-sm"><p className="font-medium">已保存计划中有 {record!.layout_review!.long_label_count} 段文字达到四行</p><p className="text-xs text-muted-foreground">基于编译换行的可读性提示，不判断科学内容，不阻止生成。可在确认前检查标签和布局；必要的分子名称、状态和限定须保留，来源记录标题不必逐字放入标签。{dirty ? " 草稿已修改，保存后更新提示。" : ""}</p><ul className="mt-2 space-y-1">{record!.layout_review!.long_labels?.map((label,index) => <li key={index} className="break-words">{label.lines} 行 · {label.width.toFixed(0)} × {label.height.toFixed(0)}：{label.preview}{label.preview.length >= 160 ? "…" : ""}{label.plan_field && <Button size="sm" variant="outline" disabled={dirty || busy} aria-label={`定位长标签 ${index + 1}`} onClick={() => { const target = Array.from(planBody.current?.querySelectorAll<HTMLInputElement>("[data-plan-field]") ?? []).find(input => input.dataset.planField === label.plan_field); target?.scrollIntoView({block:"center"}); target?.focus(); target?.select(); }}>定位标签</Button>}</li>)}</ul>{Boolean(record!.layout_review!.long_labels_omitted) && <p>另有 {record!.layout_review!.long_labels_omitted} 项未显示。</p>}</section>}
        {record && <CanvasPlanAgent key={`${projectId}:${canvasId}:${record.version}:${record.based_on_revision}`} projectId={projectId} canvasId={canvasId} record={record} disabled={busy||dirty||stale||isDirty()} unchanged={() => handoffState.current.version===record.version&&!handoffState.current.dirty&&!handoffState.current.sceneDirty&&handoffState.current.revision===record.based_on_revision} onBusy={setBusy} onPrepared={() => setOpen(false)} />}
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
            {draft.panels.map((panel, index) => <article key={panel.id} className="flex flex-wrap items-end gap-2 rounded border p-2"><label className="min-w-40 flex-1 text-xs">标题<input data-plan-field={`panels[${index}].title`} aria-label={`面板标题 ${index + 1}`} className={input} maxLength={120} value={panel.title} onChange={event => update({ panels: draft.panels.map((p, i) => i === index ? { ...p, title: event.target.value } : p) })} /></label>{([['column', '列'], ['row', '行']] as const).map(([field, title]) => <label key={field} className="w-20 text-xs">{title}<input aria-label={`面板${title} ${index + 1}`} className={input} type="number" min={0} max={10} value={panel[field]} onChange={event => update({ panels: draft.panels.map((p, i) => i === index ? { ...p, [field]: Number(event.target.value) } : p) })} /></label>)}<Button size="sm" variant="ghost" onClick={() => update({ panels: draft.panels.filter(p => p.id !== panel.id), nodes: draft.nodes.map(node => node.panel === panel.id ? { ...node, panel: undefined } : node) })}>移除面板</Button></article>)}
          </section>
          <section aria-label="计划组件" className="space-y-2"><div className="flex items-center justify-between"><h3 className="text-sm font-medium">组件</h3><Button size="sm" variant="outline" disabled={draft.nodes.length >= 100} onClick={() => update({ nodes: [...draft.nodes, { id: nextId("n", draft.nodes.map(n => n.id)), label: "", shape: "rectangle", evidence: [], ...(draft.panels.length ? { panel: draft.panels[0].id } : {}), ...(draft.layout === "grid" ? { column: draft.nodes.length % 31, row: Math.floor(draft.nodes.length / 31) } : {}) }] })}>添加组件</Button></div>
              <p className="text-xs text-muted-foreground">若活化、修饰或定位状态决定图意，请在组件名称中写明；依据和假设不会自动成为可见标签。</p>
            {draft.nodes.map((node, index) => <article key={node.id} className="space-y-2 rounded border p-2"><div className="flex items-center gap-2"><input data-plan-field={`nodes[${index}].label`} aria-label={`组件名称 ${index + 1}`} className={input} maxLength={120} value={node.label} onChange={event => updateNode(index, { label: event.target.value })} /><select aria-label={`组件形状 ${index + 1}`} className="rounded border bg-background p-1 text-sm" value={node.shape} onChange={event => updateNode(index, { shape: event.target.value as PlanningNode["shape"] })}><option value="rectangle">矩形</option><option value="ellipse">椭圆</option></select><Button size="sm" variant="ghost" onClick={() => update({ nodes: draft.nodes.filter(n => n.id !== node.id), edges: draft.edges.filter(e => e.from !== node.id && e.to !== node.id) })}>移除组件</Button></div>
              {draft.panels.length > 0 && <label className="block text-xs">所属面板<select aria-label={`组件面板 ${index + 1}`} className={input} value={node.panel ?? ""} onChange={event => updateNode(index, { panel: event.target.value || undefined })}><option value="">请选择面板</option>{draft.panels.map((panel, i) => <option key={panel.id} value={panel.id}>{panel.title || `未命名面板 ${i + 1}`}</option>)}</select></label>}
              {draft.layout === "grid" && <div className="flex gap-2"><label className="text-xs">列<input aria-label={`组件列 ${index + 1}`} className={input} type="number" min={0} max={30} value={node.column ?? ""} onChange={event => updateNode(index, { column: event.target.value === "" ? undefined : Number(event.target.value) })} /></label><label className="text-xs">行<input aria-label={`组件行 ${index + 1}`} className={input} type="number" min={0} max={30} value={node.row ?? ""} onChange={event => updateNode(index, { row: event.target.value === "" ? undefined : Number(event.target.value) })} /></label></div>}
              <details><summary className="cursor-pointer text-xs text-muted-foreground">依据与来源</summary><textarea aria-label={`组件依据 ${index + 1}`} className={`${input} mt-1`} value={node.evidence.join("\n")} onChange={event => updateNode(index, { evidence: lines(event.target.value) })} placeholder="每行一条依据或来源；没有依据时留空。" /></details></article>)}
          </section>
          <CanvasPlanningEvidence brief={draft} dirty={dirty} onEdit={index => { const field = edgeInputs.current.get(index); field?.scrollIntoView({ block: "center" }); field?.focus(); }} />
          <section aria-label="计划关系" className="space-y-2"><div className="flex items-center justify-between"><h3 className="text-sm font-medium">关系</h3><Button size="sm" variant="outline" disabled={draft.nodes.length < 2 || draft.edges.length >= 200} onClick={() => update({ edges: [...draft.edges, { from: draft.nodes[0].id, to: draft.nodes[1].id, label: "", inhibitory: false, evidence: [] }] })}>添加关系</Button></div><p className="text-xs text-muted-foreground">线型仅表示视觉样式；如用于可选步骤或不确定关系，请在说明和图例中明确含义并保留依据。</p>
            {draft.edges.map((edge, index) => <article key={index} className="space-y-2 rounded border p-2"><div className="grid items-center gap-2 sm:grid-cols-2">{([['from', '关系来源'], ['to', '关系目标']] as const).map(([field, title]) => <select key={field} aria-label={`${title} ${index + 1}`} className={input} value={edge[field]} onChange={event => updateEdge(index, { [field]: event.target.value })}>{draft.nodes.map((node, i) => <option key={node.id} value={node.id}>{node.label || `未命名组件 ${i + 1}`}</option>)}</select>)}</div><div className="flex items-center gap-2"><input ref={field => { if (field) edgeInputs.current.set(index, field); else edgeInputs.current.delete(index); }} data-plan-field={`edges[${index}].label`} aria-label={`关系说明 ${index + 1}`} className={input} maxLength={120} value={edge.label} onChange={event => updateEdge(index, { label: event.target.value })} /><select aria-label={`关系方向 ${index + 1}`} className={input} value={edge.direction ?? "forward"} disabled={edge.inhibitory} onChange={event => updateEdge(index, { direction: event.target.value as Edge["direction"] })}><option value="forward">单向</option><option value="bidirectional">双向</option><option value="undirected">无向</option></select><select aria-label={`连线样式 ${index + 1}`} className={input} value={edge.line_style ?? "solid"} onChange={event => updateEdge(index, { line_style: event.target.value as Edge["line_style"] })}><option value="solid">实线</option><option value="dashed">虚线</option><option value="dotted">点线</option></select><label className="flex shrink-0 items-center gap-1 text-xs"><input aria-label={`抑制关系 ${index + 1}`} type="checkbox" disabled={(edge.direction ?? "forward") !== "forward" && !edge.inhibitory} checked={edge.inhibitory} onChange={event => updateEdge(index, { inhibitory: event.target.checked })} />抑制</label><Button size="sm" variant="ghost" onClick={() => update({ edges: draft.edges.filter((_, i) => i !== index) })}>移除关系</Button></div><details><summary className="cursor-pointer text-xs text-muted-foreground">依据与来源</summary><textarea aria-label={`关系依据 ${index + 1}`} className={`${input} mt-1`} value={edge.evidence.join("\n")} onChange={event => updateEdge(index, { evidence: lines(event.target.value) })} /></details></article>)}
            {!draft.edges.length && <label className="block text-sm">组件关系说明<textarea aria-label="组件关系说明" className={`${input} mt-1`} maxLength={4000} value={draft.relationship_note} onChange={event => update({ relationship_note: event.target.value })} placeholder="若仅并列展示多个组件，请明确说明；不要补写未知机制。" /></label>}
          </section>
          <section aria-label="计划问题" className="space-y-2"><div className="flex items-center justify-between"><h3 className="text-sm font-medium">待澄清问题与答案</h3><Button size="sm" variant="outline" disabled={draft.questions.length >= 20} onClick={() => update({ questions: [...draft.questions, { id: nextId("q", draft.questions.map(q => q.id)), question: "", answer: "", blocking: true }] })}>添加问题</Button></div>
            <p className="text-xs text-muted-foreground">物种、机制分支和科学范围等未知项应在生成前确认。必答问题填写答案后仍保持勾选；清空答案会再次阻止生成。语言、配色等可选偏好可取消勾选并留空，不会自动视为已确认。</p>
            {draft.questions.map((question, index) => <article key={question.id} className="space-y-2 rounded border p-2"><label className="flex items-center gap-2 text-xs"><input type="checkbox" aria-label={`生成前必须确认 ${index + 1}`} checked={question.blocking !== false} onChange={event => update({ questions: draft.questions.map((q, i) => i === index ? { ...q, blocking: event.target.checked } : q) })} />生成草图前必须确认</label><input aria-label={`澄清问题 ${index + 1}`} className={input} maxLength={4000} value={question.question} onChange={event => update({ questions: draft.questions.map((q, i) => i === index ? { ...q, question: event.target.value } : q) })} /><textarea aria-label={`问题答案 ${index + 1}`} className={`${input} min-h-20`} maxLength={4000} value={question.answer} placeholder="填写答案后，请核对组件和关系是否已反映答案。" onChange={event => update({ questions: draft.questions.map((q, i) => i === index ? { ...q, answer: event.target.value } : q) })} /><Button size="sm" variant="ghost" onClick={() => update({ questions: draft.questions.filter((_, i) => i !== index) })}>移除问题</Button></article>)}
          </section>
          <label className="text-sm">假设与待核对事项<textarea aria-label="计划假设" className={`${input} mt-1 min-h-20`} value={draft.assumptions.join("\n")} onChange={event => update({ assumptions: lines(event.target.value) })} placeholder="每行一项，明确哪些内容需要核对。" /></label>
        </fieldset>}
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2 border-t p-3"><Button size="sm" variant="outline" disabled={busy} onClick={() => { if (discard()) void load(); }}>刷新计划</Button><Button size="sm" variant="outline" disabled={busy || stale || !draft || !dirty && !isDirty() && Boolean(record && record.based_on_revision === getRevision())} onClick={async () => {
        if (!draft || busy || stale) return; setBusy(true); setError("");
        try { if (!await ensureSaved()) throw new Error("请先解决画布保存错误。"); apply(await gatewayRequest("PUT", path, { expected_revision: getRevision(), expected_plan_version: record?.version ?? 0, document: draft }) as Planning); }
        catch (cause) { setError(`保存计划失败：${String(cause)}`); } finally { setBusy(false); }
      }}>{busy ? "处理中…" : "保存绘图计划"}</Button><Button size="sm" disabled={busy || stale || dirty || isDirty() || !record || record.issues.length > 0 || record.based_on_revision !== getRevision()} onClick={async () => {
        if (!record || busy || stale) return; setBusy(true); setError("");
        try { const proposal = await gatewayRequest("POST", `${path}/sketch`, { expected_revision: getRevision(), expected_plan_version: record.version, summary: "根据绘图计划生成草图" }) as { id: string }; setOpen(false); onProposed(proposal.id); }
        catch (cause) { setError(String(cause)); } finally { setBusy(false); }
      }}>生成草图提案</Button></div>
    </DialogContent>
  </Dialog>;
}
