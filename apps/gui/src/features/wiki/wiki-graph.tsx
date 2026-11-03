import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as d3 from "d3";
import { Maximize, RefreshCw, Search, ZoomIn, ZoomOut } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { openWikiEntry, useWikiRevision } from "./wiki-state";
import type { EntrySummary } from "./wiki-api";

type Node = d3.SimulationNodeDatum & { id: string; title: string; tags: string[]; degree: number };
type Edge = d3.SimulationLinkDatum<Node> & { source: string | Node; target: string | Node };
type GraphPage = { nodes: EntrySummary[]; edges: Array<{ source: string; target: string }>; next_offset: number | null };
type GraphData = { nodes: EntrySummary[]; edges: GraphPage["edges"] };

export function WikiGraph() {
  const [data, setData] = useState<GraphData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState(0);
  const [reload, setReload] = useState(0);
  const [query, setQuery] = useState("");
  const [isolated, setIsolated] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const zoomRef = useRef<d3.ZoomBehavior<SVGSVGElement, unknown> | null>(null);
  const revision = useWikiRevision();

  useEffect(() => {
    const controller = new AbortController();
    setData(null); setError(null); setProgress(0);
    void (async () => {
      const nodes: EntrySummary[] = [];
      const edges: GraphPage["edges"] = [];
      let offset = 0;
      do {
        const page = await gatewayRequest("GET", `/wiki/graph?limit=200&offset=${offset}`, undefined, { signal: controller.signal }) as GraphPage;
        if (!Array.isArray(page.nodes) || !Array.isArray(page.edges) || !(page.next_offset === null || (Number.isInteger(page.next_offset) && page.next_offset > offset))) throw new Error("图谱数据格式无效");
        if (controller.signal.aborted) return;
        nodes.push(...page.nodes); edges.push(...page.edges);
        setProgress(nodes.length);
        if (page.next_offset === null) break;
        offset = page.next_offset;
      } while (!controller.signal.aborted);
      if (!controller.signal.aborted) setData({ nodes, edges });
    })().catch((cause: unknown) => { if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => controller.abort();
  }, [revision, reload]);

  const graph = useMemo(() => {
    if (!data) return null;
    const all = new Map(data.nodes.map(entry => [entry.id, entry]));
    const keys = new Set<string>();
    const edges = data.edges.filter(edge => {
      const key = JSON.stringify([edge.source, edge.target]);
      if (!all.has(edge.source) || !all.has(edge.target) || keys.has(key)) return false;
      keys.add(key); return true;
    });
    const degree = new Map<string, number>();
    for (const edge of edges) { degree.set(edge.source, (degree.get(edge.source) ?? 0) + 1); if (edge.source !== edge.target) degree.set(edge.target, (degree.get(edge.target) ?? 0) + 1); }
    return {
      nodes: data.nodes.filter(entry => isolated || degree.has(entry.id)).map(entry => ({ id: entry.id, title: entry.title, tags: entry.tags ?? [], degree: degree.get(entry.id) ?? 0 } as Node)),
      edges: edges.map(edge => ({ ...edge } as Edge)),
    };
  }, [data, isolated]);

  useEffect(() => {
    const svgElement = svgRef.current;
    if (!svgElement) return;
    const svg = d3.select(svgElement);
    svg.selectAll("*").remove();
    if (!graph || !graph.nodes.length) return;
    const width = svgElement.clientWidth || 800, height = svgElement.clientHeight || 600;
    const layer = svg.append("g");
    const zoom = d3.zoom<SVGSVGElement, unknown>().scaleExtent([0.15, 5]).on("zoom", event => layer.attr("transform", event.transform));
    zoomRef.current = zoom;
    svg.call(zoom).on("dblclick.zoom", null);
    const edges = layer.append("g").attr("class", "wiki-graph-links").selectAll("line").data(graph.edges).join("line").attr("stroke", "var(--muted-foreground)").attr("stroke-opacity", 0.35).attr("stroke-width", 1);
    const nodes = layer.append("g").selectAll<SVGGElement, Node>("g").data(graph.nodes, d => d.id).join("g")
      .attr("class", "wiki-graph-node").attr("data-node-id", d => d.id).attr("role", "button").attr("tabindex", 0).attr("aria-label", d => `打开词条 ${d.title}`).style("cursor", "pointer");
    nodes.append("circle").attr("r", d => Math.min(14, 4 + Math.sqrt(d.degree) * 1.8)).attr("fill", "var(--primary)").attr("stroke", "var(--background)").attr("stroke-width", 1.5);
    nodes.append("text").text(d => d.title).attr("x", d => Math.min(14, 4 + Math.sqrt(d.degree) * 1.8) + 5).attr("dy", "0.35em").attr("fill", "var(--foreground)").attr("font-size", 11).attr("paint-order", "stroke").attr("stroke", "var(--background)").attr("stroke-width", 3).attr("stroke-linejoin", "round");
    nodes.append("title").text(d => `${d.title} · ${d.degree} 条连接`);
    const simulation = d3.forceSimulation<Node>(graph.nodes)
      .force("link", d3.forceLink<Node, Edge>(graph.edges).id(d => d.id).distance(75).strength(0.3))
      .force("charge", d3.forceManyBody().strength(-85))
      .force("collision", d3.forceCollide<Node>().radius(d => Math.min(14, 4 + Math.sqrt(d.degree) * 1.8) + 8))
      .force("center", d3.forceCenter(width / 2, height / 2));
    nodes.call(d3.drag<SVGGElement, Node>()
      .on("start", (event, node) => { if (!event.active) simulation.alphaTarget(0.2).restart(); node.fx = node.x; node.fy = node.y; })
      .on("drag", (event, node) => { node.fx = event.x; node.fy = event.y; })
      .on("end", (event, node) => { if (!event.active) simulation.alphaTarget(0); node.fx = null; node.fy = null; }));
    nodes.on("mouseenter focus", (_event, node) => {
      edges.attr("stroke-opacity", edge => (edge.source as Node).id === node.id || (edge.target as Node).id === node.id ? 0.9 : 0.08)
        .attr("stroke", edge => (edge.source as Node).id === node.id || (edge.target as Node).id === node.id ? "var(--primary)" : "var(--muted-foreground)");
    }).on("mouseleave blur", () => edges.attr("stroke-opacity", 0.35).attr("stroke", "var(--muted-foreground)"));
    nodes.on("click", (_event, node) => { setSelected(node.id); openWikiEntry(node.id, { title: node.title }); })
      .on("keydown", (event: KeyboardEvent, node) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); setSelected(node.id); openWikiEntry(node.id, { title: node.title }); } });
    simulation.on("tick", () => {
      edges.attr("x1", d => (d.source as Node).x ?? 0).attr("y1", d => (d.source as Node).y ?? 0).attr("x2", d => (d.target as Node).x ?? 0).attr("y2", d => (d.target as Node).y ?? 0);
      nodes.attr("transform", d => `translate(${d.x ?? 0},${d.y ?? 0})`);
    });
    const observer = new ResizeObserver(() => {
      const w = svgElement.clientWidth, h = svgElement.clientHeight;
      if (w && h) { simulation.force("center", d3.forceCenter(w / 2, h / 2)); simulation.alpha(0.15).restart(); }
    });
    observer.observe(svgElement);
    return () => { observer.disconnect(); simulation.stop(); svg.on(".zoom", null); zoomRef.current = null; };
  }, [graph]);

  useEffect(() => {
    const term = query.trim().toLocaleLowerCase();
    d3.select(svgRef.current).selectAll<SVGGElement, Node>(".wiki-graph-node")
      .attr("opacity", node => !term || node.title.toLocaleLowerCase().includes(term) || node.tags.some(tag => tag.toLocaleLowerCase().includes(term)) ? 1 : 0.15)
      .select("circle").attr("stroke", node => node.id === selected ? "var(--foreground)" : "var(--background)").attr("stroke-width", node => node.id === selected ? 3 : 1.5);
  }, [query, selected, graph]);

  const adjustZoom = useCallback((factor: number | "reset") => {
    if (!svgRef.current || !zoomRef.current) return;
    const svg = d3.select(svgRef.current);
    if (factor === "reset") svg.transition().duration(200).call(zoomRef.current.transform, d3.zoomIdentity);
    else svg.transition().duration(200).call(zoomRef.current.scaleBy, factor);
  }, []);

  return <section aria-label="知识关系图谱" className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background">
    <div className="flex shrink-0 flex-wrap items-center gap-2 border-b bg-muted/30 px-3 py-2">
      <div className="relative w-48 max-w-full"><Search className="pointer-events-none absolute left-2 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" /><Input aria-label="搜索图谱词条" placeholder="搜索词条或标签" className="h-8 pl-8 text-xs" value={query} onChange={event => setQuery(event.target.value)} /></div>
      <label className="flex items-center gap-1.5 text-xs text-muted-foreground"><input type="checkbox" checked={isolated} onChange={event => setIsolated(event.target.checked)} />显示孤立词条</label>
      <span className="text-xs text-muted-foreground">{graph ? `${graph.nodes.length} 个词条 · ${graph.edges.length} 条链接` : `已加载 ${progress} 个词条`}</span>
      <div className="ml-auto flex items-center gap-1">
        <Button size="icon-sm" variant="ghost" aria-label="缩小图谱" title="缩小" onClick={() => adjustZoom(0.8)}><ZoomOut /></Button>
        <Button size="icon-sm" variant="ghost" aria-label="放大图谱" title="放大" onClick={() => adjustZoom(1.25)}><ZoomIn /></Button>
        <Button size="icon-sm" variant="ghost" aria-label="重置图谱视图" title="重置视图" onClick={() => adjustZoom("reset")}><Maximize /></Button>
        <Button size="icon-sm" variant="ghost" aria-label="刷新图谱" title="刷新图谱" onClick={() => setReload(value => value + 1)}><RefreshCw /></Button>
      </div>
    </div>
    <svg ref={svgRef} role="group" aria-label="知识词条关系图，滚轮缩放，拖动空白处平移，拖动节点调整位置" className="min-h-0 w-full flex-1 touch-none" />
    {!data && <div className="pointer-events-none absolute inset-0 grid place-items-center text-sm text-muted-foreground" role="status">{error ? `加载图谱失败：${error}` : `正在加载图谱… ${progress} 个词条`}</div>}
    {data && !graph?.nodes.length && <div className="pointer-events-none absolute inset-0 grid place-items-center text-sm text-muted-foreground">{isolated ? "知识库暂无词条" : "暂无相互链接的词条"}</div>}
    <p className="pointer-events-none absolute bottom-2 left-3 text-xs text-muted-foreground">滚轮缩放 · 拖动平移 · 点击节点打开词条</p>
  </section>;
}
