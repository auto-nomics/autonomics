import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as d3 from "d3";
import { Maximize, RefreshCw, Search, Settings2, ZoomIn, ZoomOut } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { openWikiEntry, useWikiRevision } from "./wiki-state";


type Node = d3.SimulationNodeDatum & { id: string; title: string; tags: string[]; degree: number };
type Edge = d3.SimulationLinkDatum<Node> & { source: string | Node; target: string | Node };
type GraphEntry = { id: string; title: string; tags?: string[] };
type GraphData = { nodes: GraphEntry[]; edges: Array<{ source: string; target: string }> };
type LinkRef = { id: string; title?: string } | string;
function refs(value: unknown): Array<{ id: string; title: string }> {
  if (!Array.isArray(value)) throw new Error("链接数据格式无效");
  return (value as LinkRef[]).flatMap(item => {
    if (typeof item === "string") return [{ id: item, title: item }];
    return item && typeof item.id === "string" ? [{ id: item.id, title: item.title || item.id }] : [];
  });
}

export function WikiGraph({ entryId, title }: { entryId: string; title: string }) {
  const [data, setData] = useState<GraphData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [depth, setDepth] = useState(1);
  const [query, setQuery] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(true);
  const settingsOpenRef = useRef(settingsOpen);
  settingsOpenRef.current = settingsOpen;
  const [showLabels, setShowLabels] = useState(true);
  const [repulsion, setRepulsion] = useState(140);
  const [linkDistance, setLinkDistance] = useState(75);
  const repulsionRef = useRef(repulsion);
  const distanceRef = useRef(linkDistance);
  repulsionRef.current = repulsion;
  distanceRef.current = linkDistance;
  const [incomingEnabled, setIncomingEnabled] = useState(true);
  const [outgoingEnabled, setOutgoingEnabled] = useState(true);
  const labelsRef = useRef(showLabels);
  labelsRef.current = showLabels;
  const queryRef = useRef(query);
  queryRef.current = query;
  const [selected, setSelected] = useState<string | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const zoomRef = useRef<d3.ZoomBehavior<SVGSVGElement, unknown> | null>(null);
  const updateLabelsRef = useRef<(() => void) | null>(null);
  const hoveredRef = useRef<string | null>(null);
  const fitRef = useRef<(() => void) | null>(null);
  const simulationRef = useRef<d3.Simulation<Node, Edge> | null>(null);
  const interactedRef = useRef(false);
  const revision = useWikiRevision();

  useEffect(() => {
    const controller = new AbortController();
    setData(null); setError(null); setSelected(null);
    if (!entryId) return () => controller.abort();
    void (async () => {
      const nodes = new Map<string, GraphEntry>([[entryId, { id: entryId, title: title || entryId }]]);
      const edges: GraphData["edges"] = [];
      let frontier = [entryId];
      for (let level = 0; level < depth && frontier.length && !controller.signal.aborted; level++) {
        const next: string[] = [];
        // Bound concurrent requests so large layers do not flood the gateway.
        for (let i = 0; i < frontier.length; i += 6) {
          const batch = frontier.slice(i, i + 6);
          const responses = await Promise.all(batch.map(async id => ({
            id,
            graph: await gatewayRequest("GET", `/wiki/entries/${encodeURIComponent(id)}/links`, undefined, { signal: controller.signal }) as { entry?: { id?: string; title?: string }; in_links?: unknown; out_links?: unknown },
          })));
          if (controller.signal.aborted) return;
          for (const { id, graph: response } of responses) {
            const existing = nodes.get(id);
            if (existing && response.entry?.title) existing.title = response.entry.title;
            for (const [direction, values] of [["in", refs(response.in_links)], ["out", refs(response.out_links)]] as const) {
              if ((direction === "in" && !incomingEnabled) || (direction === "out" && !outgoingEnabled)) continue;
              for (const ref of values) {
                if (!nodes.has(ref.id)) { nodes.set(ref.id, ref); next.push(ref.id); }
                edges.push(direction === "in" ? { source: ref.id, target: id } : { source: id, target: ref.id });
              }
            }
          }
        }
        frontier = next;
      }
      if (!controller.signal.aborted) setData({ nodes: [...nodes.values()], edges });
    })().catch((cause: unknown) => { if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => controller.abort();
  }, [entryId, title, revision, reload, depth, incomingEnabled, outgoingEnabled]);

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
      nodes: data.nodes.map(entry => ({ id: entry.id, title: entry.title, tags: entry.tags ?? [], degree: degree.get(entry.id) ?? 0 } as Node)),
      edges: edges.map(edge => ({ ...edge } as Edge)),
    };
  }, [data]);

  useEffect(() => {
    const svgElement = svgRef.current;
    if (!svgElement) return;
    const svg = d3.select(svgElement);
    svg.selectAll("*").remove();
    if (!graph || !graph.nodes.length) return;
    const width = svgElement.clientWidth || 800, height = svgElement.clientHeight || 600;
    const layer = svg.append("g");
    interactedRef.current = false;
    const zoom = d3.zoom<SVGSVGElement, unknown>().scaleExtent([0.05, 5]).on("zoom", event => {
      if (event.sourceEvent) interactedRef.current = true;
      layer.attr("transform", event.transform);
      updateLabelsRef.current?.();
    });
    zoomRef.current = zoom;
    svg.call(zoom).on("dblclick.zoom", null);
    svg.call(zoom.transform, d3.zoomIdentity);
    const edges = layer.append("g").attr("class", "wiki-graph-links").selectAll("line").data(graph.edges).join("line").attr("stroke", "var(--muted-foreground)").attr("stroke-opacity", 0.2).attr("stroke-width", 0.8);
    const nodes = layer.append("g").selectAll<SVGGElement, Node>("g").data(graph.nodes, d => d.id).join("g")
      .attr("class", "wiki-graph-node").attr("data-node-id", d => d.id).attr("role", "button").attr("tabindex", 0).attr("aria-label", d => `打开词条 ${d.title}`).style("cursor", "pointer");
    const radius = (node: Node) => node.id === entryId ? 6 : Math.min(6, 2.5 + Math.sqrt(node.degree) * 0.7);
    nodes.append("circle").attr("r", radius).attr("fill", node => node.id === entryId ? "var(--primary)" : "var(--muted-foreground)").attr("stroke", "var(--background)").attr("stroke-width", 0.8);
    nodes.append("text").text(d => d.title).attr("x", 0).attr("y", d => radius(d) + 2).attr("dy", "0.85em").attr("text-anchor", "middle").attr("fill", "var(--foreground)").attr("font-size", 6.5).attr("paint-order", "stroke").attr("stroke", "var(--background)").attr("stroke-width", 1.5).attr("stroke-linejoin", "round").style("pointer-events", "none");
    nodes.append("title").text(d => `${d.title} · ${d.degree} 条连接`);
    const updateLabels = () => {
      const scale = d3.zoomTransform(svgElement).k;
      const opacity = Math.max(0, Math.min(1, (scale - 0.7) / 0.8));
      const term = queryRef.current.trim().toLocaleLowerCase();
      nodes.select("text").attr("opacity", node => {
        if (node.id === entryId || node.id === hoveredRef.current || (term && node.title.toLocaleLowerCase().includes(term))) return 1;
        return labelsRef.current ? opacity : 0;
      }).attr("visibility", node => {
        if (node.id === entryId || node.id === hoveredRef.current || (term && node.title.toLocaleLowerCase().includes(term))) return "visible";
        return labelsRef.current && opacity > 0 ? "visible" : "hidden";
      });
    };
    hoveredRef.current = null;
    updateLabelsRef.current = updateLabels;
    updateLabels();
    const simulation = d3.forceSimulation<Node>(graph.nodes)
      .alphaDecay(0.018)
      .velocityDecay(0.4)
      // D3's degree-aware link strength prevents highly connected hubs from
      // pulling hundreds of neighbours into the same small area.
      .force("link", d3.forceLink<Node, Edge>(graph.edges).id(d => d.id).distance(distanceRef.current))
      .force("charge", d3.forceManyBody().strength(-repulsionRef.current))
      .force("collision", d3.forceCollide<Node>().radius(d => radius(d) + 9).iterations(3))
      .force("center", d3.forceCenter(width / 2, height / 2));
    simulationRef.current = simulation;
    nodes.call(d3.drag<SVGGElement, Node>()
      .on("start", (event, node) => { interactedRef.current = true; if (!event.active) simulation.alphaTarget(0.2).restart(); node.fx = node.x; node.fy = node.y; })
      .on("drag", (event, node) => { node.fx = event.x; node.fy = event.y; })
      .on("end", (event, node) => { if (!event.active) simulation.alphaTarget(0); node.fx = null; node.fy = null; }));
    nodes.on("mouseenter focus", (_event, node) => {
      hoveredRef.current = node.id;
      updateLabels();
      edges.attr("stroke-opacity", edge => (edge.source as Node).id === node.id || (edge.target as Node).id === node.id ? 0.9 : 0.08)
        .attr("stroke", edge => (edge.source as Node).id === node.id || (edge.target as Node).id === node.id ? "var(--primary)" : "var(--muted-foreground)");
    }).on("mouseleave blur", () => {
      hoveredRef.current = null;
      updateLabels();
      edges.attr("stroke-opacity", 0.2).attr("stroke", "var(--muted-foreground)");
    });
    nodes.on("click", (_event, node) => { setSelected(node.id); openWikiEntry(node.id, { title: node.title }); })
      .on("keydown", (event: KeyboardEvent, node) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); setSelected(node.id); openWikiEntry(node.id, { title: node.title }); } });
    simulation.on("tick", () => {
      edges.attr("x1", d => (d.source as Node).x ?? 0).attr("y1", d => (d.source as Node).y ?? 0).attr("x2", d => (d.target as Node).x ?? 0).attr("y2", d => (d.target as Node).y ?? 0);
      nodes.attr("transform", d => `translate(${d.x ?? 0},${d.y ?? 0})`);
    });
    const fit = () => {
      const positioned = graph.nodes.filter(node => Number.isFinite(node.x) && Number.isFinite(node.y));
      if (!positioned.length || !svgElement.clientWidth || !svgElement.clientHeight) return false;
      const [minX, maxX] = d3.extent(positioned, node => node.x!) as [number, number];
      const [minY, maxY] = d3.extent(positioned, node => node.y!) as [number, number];
      const width = svgElement.clientWidth, height = svgElement.clientHeight;
      const available = width > 600 && settingsOpenRef.current ? width - 230 : width;
      const scale = Math.max(0.05, Math.min(1.5, (available - 60) / Math.max(80, maxX - minX), (height - 60) / Math.max(80, maxY - minY)));
      svg.transition().duration(250).call(zoom.transform, d3.zoomIdentity.translate(available / 2 - (minX + maxX) / 2 * scale, height / 2 - (minY + maxY) / 2 * scale).scale(scale));
      return true;
    };
    fitRef.current = fit;
    const fitTimer = window.setTimeout(() => { if (!interactedRef.current) fit(); }, 900);
    // The graph may continue to spread after the first fit, especially for
    // dense graphs; refit the settled layout unless the user moved it.
    simulation.on("end", () => { if (!interactedRef.current) fit(); });
    const observer = new ResizeObserver(() => {
      const w = svgElement.clientWidth, h = svgElement.clientHeight;
      if (w && h) { simulation.force("center", d3.forceCenter(w / 2, h / 2)); simulation.alpha(0.15).restart(); }
    });
    observer.observe(svgElement);
    return () => { window.clearTimeout(fitTimer); observer.disconnect(); simulation.stop(); simulationRef.current = null; svg.interrupt().on(".zoom", null); zoomRef.current = null; fitRef.current = null; updateLabelsRef.current = null; hoveredRef.current = null; };
  }, [graph, entryId]);

  useEffect(() => {
    const simulation = simulationRef.current;
    if (!simulation) return;
    simulation.force<d3.ForceManyBody<Node>>("charge")?.strength(-repulsion);
    simulation.force<d3.ForceLink<Node, Edge>>("link")?.distance(linkDistance);
    simulation.alpha(0.35).restart();
  }, [repulsion, linkDistance]);

  useEffect(() => {
    const term = query.trim().toLocaleLowerCase();
    d3.select(svgRef.current).selectAll<SVGGElement, Node>(".wiki-graph-node")
      .attr("opacity", node => !term || node.title.toLocaleLowerCase().includes(term) || node.tags.some(tag => tag.toLocaleLowerCase().includes(term)) ? 1 : 0.15)
      .select("circle").attr("stroke", node => node.id === selected ? "var(--foreground)" : "var(--background)").attr("stroke-width", node => node.id === selected ? 2 : 0.8);
    updateLabelsRef.current?.();
  }, [query, selected, graph, showLabels, entryId]);

  const adjustZoom = useCallback((factor: number | "reset") => {
    if (!svgRef.current || !zoomRef.current) return;
    const svg = d3.select(svgRef.current);
    interactedRef.current = true;
    if (factor === "reset") fitRef.current?.();
    else svg.transition().duration(200).call(zoomRef.current.scaleBy, factor);
  }, []);

  return <section aria-label="知识关系图谱" className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background">
    <div className="flex h-9 shrink-0 items-center gap-1 border-b px-2 text-xs text-muted-foreground">
      <span className="min-w-0 flex-1 truncate">{entryId ? `局部图谱 / ${title || data?.nodes[0]?.title || entryId}` : "局部图谱"}</span>
      <Button size="icon-sm" variant="ghost" aria-label="缩小图谱" title="缩小" onClick={() => adjustZoom(0.8)}><ZoomOut /></Button>
      <Button size="icon-sm" variant="ghost" aria-label="放大图谱" title="放大" onClick={() => adjustZoom(1.25)}><ZoomIn /></Button>
      <Button size="icon-sm" variant="ghost" aria-label="重置图谱视图" title="适合画布" onClick={() => adjustZoom("reset")}><Maximize /></Button>
      <Button size="icon-sm" variant="ghost" aria-label="刷新图谱" title="刷新图谱" onClick={() => setReload(value => value + 1)}><RefreshCw /></Button>
      <Button size="icon-sm" variant="ghost" aria-label="图谱设置" title="图谱设置" onClick={() => setSettingsOpen(value => !value)}><Settings2 /></Button>
    </div>
    {settingsOpen && <aside aria-label="图谱设置面板" className="absolute right-3 top-12 z-10 max-h-[calc(100%-4rem)] w-56 max-w-[calc(100%-1.5rem)] overflow-y-auto rounded-lg border bg-card/95 text-xs text-card-foreground shadow-xl backdrop-blur-sm">
      <div className="border-b px-3 py-2.5 font-medium">筛选</div>
      <div className="space-y-4 p-3">
        <div className="relative"><Search className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" /><Input aria-label="搜索图谱词条" placeholder="搜索相关词条" className="h-8 pl-7 text-xs" value={query} onChange={event => setQuery(event.target.value)} /></div>
        <label className="block text-muted-foreground">关系层级 <span className="float-right text-foreground">{depth}</span>
          <select aria-label="图谱关系层级" value={depth} onChange={event => setDepth(Number(event.target.value))} className="mt-2 h-8 w-full rounded-md border bg-background px-2 text-foreground">{[1, 2, 3, 4, 5].map(level => <option key={level} value={level}>{level} 级</option>)}</select>
        </label>
        <label className="flex items-center justify-between gap-2 text-muted-foreground">入链<input type="checkbox" aria-label="显示入链" checked={incomingEnabled} onChange={event => setIncomingEnabled(event.target.checked)} /></label>
        <label className="flex items-center justify-between gap-2 text-muted-foreground">出链<input type="checkbox" aria-label="显示出链" checked={outgoingEnabled} onChange={event => setOutgoingEnabled(event.target.checked)} /></label>
        <label className="flex items-center justify-between gap-2 text-muted-foreground">显示节点标签<input type="checkbox" aria-label="显示所有节点标签" checked={showLabels} onChange={event => setShowLabels(event.target.checked)} /></label>
      </div>
      <details className="border-t" open>
        <summary className="cursor-pointer px-3 py-2.5 font-medium">布局力度</summary>
        <div className="space-y-4 px-3 pb-3">
          <label className="block text-muted-foreground">节点排斥力 <span className="float-right text-foreground">{repulsion}</span><input type="range" aria-label="节点排斥力" min="30" max="350" step="10" value={repulsion} onChange={event => setRepulsion(Number(event.target.value))} className="mt-2 w-full accent-primary" /></label>
          <label className="block text-muted-foreground">链接距离 <span className="float-right text-foreground">{linkDistance}</span><input type="range" aria-label="链接距离" min="30" max="160" step="5" value={linkDistance} onChange={event => setLinkDistance(Number(event.target.value))} className="mt-2 w-full accent-primary" /></label>
        </div>
      </details>
      <div className="border-t px-3 py-2 text-muted-foreground">{entryId ? `中心：${title || data?.nodes[0]?.title || entryId}` : "未选择词条"}{graph ? ` · ${graph.nodes.length} 个词条 · ${graph.edges.length} 条链接` : ""}</div>
    </aside>}
    <svg ref={svgRef} role="group" aria-label="知识词条关系图，滚轮缩放，拖动空白处平移，拖动节点调整位置" className="min-h-0 w-full flex-1 touch-none" />
    {!data && <div className="pointer-events-none absolute inset-0 grid place-items-center text-sm text-muted-foreground" role="status">{!entryId ? "请先打开知识库词条" : error ? `加载图谱失败：${error}` : "正在加载局部图谱…"}</div>}
    {data && graph?.edges.length === 0 && <p className="pointer-events-none absolute bottom-8 left-3 text-xs text-muted-foreground">当前词条暂无关联链接</p>}
    <p className="pointer-events-none absolute bottom-2 left-3 text-xs text-muted-foreground">滚轮缩放 · 拖动平移 · 点击节点打开词条</p>
  </section>;
}
