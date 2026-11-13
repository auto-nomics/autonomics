import { useState } from "react";
import { Button } from "@/components/ui/button";
import { reviewPlanningGraph } from "./canvas-planning-graph";
import type { ScientificBrief } from "./canvas-planning";

// Read the current draft without mutating the saved plan or interpreting citations.
export function CanvasPlanningEvidence({ brief, dirty, onEdit }: {
  brief: ScientificBrief; dirty: boolean; onEdit: (index: number) => void;
}) {
  const [filter, setFilter] = useState("all"), [page, setPage] = useState(0);
  const nodes = new Map(brief.nodes.map(node => [node.id, node.label]));
  const graph = reviewPlanningGraph(brief);
  const labels = (ids: string[]) => ids.map(id => `${nodes.get(id) || "未命名"} (${id})`).join("、") || "无";
  const entries = brief.edges.map((edge, index) => ({ edge, index, recorded: edge.evidence.some(value => value.trim()) }));
  const missing = entries.filter(entry => !entry.recorded).length;
  const filtered = entries.filter(entry => filter === "all" || (filter === "missing" ? !entry.recorded : entry.recorded));
  const pages = Math.max(1, Math.ceil(filtered.length / 20)), current = Math.min(page, pages - 1);
  return <details className="rounded border p-3">
    <summary className="cursor-pointer text-sm font-medium">核对关系与依据（{entries.length} 条）</summary>
    <section aria-label="计划关系依据核对" className="mt-3 space-y-3">
      <p className="text-xs text-muted-foreground">{dirty ? "当前未保存草稿" : "当前计划"} · {missing} 条关系未填写依据。请逐条核对起点、终点、关系说明与来源是否描述同一个转变。已填写不代表科学支持已验证；这里不判断引用真实性或相关性。</p>
      <details className="rounded border p-2"><summary className="cursor-pointer text-xs">连接结构：{graph.weak_components.length} 组 · {graph.isolated_node_ids.length} 个独立组件</summary><section aria-label="计划连接结构核对" className="mt-2 space-y-2 text-xs">
        <p>只统计已声明的连接，不验证科学因果。多组连接可以用于并列比较；若意图是一条信号主线，请核对起点与终点是否连通。双向及无向关系按两端相连统计；不会自动补线或阻止并列图生成。</p>
        <p>无入边节点：{labels(graph.source_node_ids)}</p><p>无出边节点：{labels(graph.sink_node_ids)}</p><p>无连接组件：{labels(graph.isolated_node_ids)}</p>
        <ol className="list-decimal space-y-1 pl-4">{graph.weak_components.map((ids, i) => <li key={i}>{labels(ids)}</li>)}</ol>
        {graph.invalid_edge_indices.length > 0 && <p>第 {graph.invalid_edge_indices.map(i => i + 1).join("、")} 条关系端点缺失，未纳入统计；请先修正端点。</p>}
      </section></details>
      <label className="flex items-center gap-2 text-xs">显示<select aria-label="计划依据筛选" className="rounded border bg-background p-1 text-sm" value={filter} onChange={event => { setFilter(event.target.value); setPage(0); }}><option value="all">全部关系</option><option value="missing">未填写依据</option><option value="recorded">已填写依据（未验证）</option></select></label>
      <div className="overflow-x-auto"><table className="w-full table-fixed text-left text-xs"><caption className="sr-only">当前绘图计划的关系端点、说明和未验证依据</caption><thead><tr className="border-b"><th className="w-[26%] p-2">起点 → 终点</th><th className="w-[25%] p-2">关系说明</th><th className="p-2">记录的依据（未验证）</th><th className="w-24 p-2">编辑</th></tr></thead><tbody>
        {filtered.slice(current * 20, (current + 1) * 20).map(({ edge, index, recorded }) => <tr key={index} aria-label={`计划关系核对 ${index + 1}`} className="border-b align-top">
          <td className="space-y-1 break-words p-2"><p>{nodes.get(edge.from) || "未找到起点"} <span className="text-muted-foreground">({edge.from})</span></p><p>{edge.direction === "undirected" ? "—" : edge.direction === "bidirectional" ? "↔" : "→"}{edge.inhibitory ? " 抑制" : ""}</p><p>{nodes.get(edge.to) || "未找到终点"} <span className="text-muted-foreground">({edge.to})</span></p></td>
          <td className="whitespace-pre-wrap break-words p-2">{edge.label || "未填写说明"}</td>
          <td className="break-words p-2">{recorded ? <details><summary className="cursor-pointer">{edge.evidence.filter(value => value.trim()).length} 条已填写依据 · 未验证</summary><ol className="mt-2 list-decimal space-y-2 pl-4">{edge.evidence.slice(0, 100).map((reference, i) => <li key={i} className="whitespace-pre-wrap break-all">{reference.length > 2000 ? `${reference.slice(0, 2000)}…（显示已截断）` : reference || "空白依据"}</li>)}</ol>{edge.evidence.length > 100 && <p>仅显示前 100 条，共 {edge.evidence.length} 条；请在编辑字段核对完整内容。</p>}</details> : "未填写依据"}</td>
          <td className="p-2"><Button type="button" size="sm" variant="outline" aria-label={`编辑计划关系 ${index + 1}`} onClick={() => onEdit(index)}>编辑</Button></td>
        </tr>)}
      </tbody></table></div>
      {!filtered.length && <p className="text-xs">没有符合当前筛选的关系。</p>}
      {pages > 1 && <div className="flex items-center gap-2"><Button type="button" size="sm" variant="outline" disabled={current === 0} onClick={() => setPage(current - 1)}>上一页关系</Button><span className="text-xs">第 {current + 1} / {pages} 页 · {filtered.length} 条</span><Button type="button" size="sm" variant="outline" disabled={current + 1 >= pages} onClick={() => setPage(current + 1)}>下一页关系</Button></div>}
    </section>
  </details>;
}
