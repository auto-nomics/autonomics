import type { ScientificBrief } from "./canvas-planning";

// Topology observations only; bidirectional/undirected edges are incident at both ends.
export function reviewPlanningGraph(brief: Pick<ScientificBrief, "nodes" | "edges">) {
  const index = new Map(brief.nodes.map((node, i) => [node.id, i]));
  const neighbors = brief.nodes.map(() => [] as number[]);
  const incoming = brief.nodes.map(() => 0), outgoing = brief.nodes.map(() => 0);
  const invalid_edge_indices: number[] = [];
  brief.edges.forEach((edge, i) => {
    const from = index.get(edge.from), to = index.get(edge.to);
    if (from === undefined || to === undefined) { invalid_edge_indices.push(i); return; }
    neighbors[from].push(to); neighbors[to].push(from); incoming[to]++; outgoing[from]++;
    if (edge.direction === "undirected" || edge.direction === "bidirectional") { incoming[from]++; outgoing[to]++; }
  });
  const seen = new Set<number>(), weak_components: string[][] = [];
  for (let start = 0; start < brief.nodes.length; start++) {
    if (seen.has(start)) continue;
    const stack = [start], members: number[] = []; seen.add(start);
    while (stack.length) {
      const node = stack.pop()!; members.push(node);
      for (const neighbor of neighbors[node]) if (!seen.has(neighbor)) { seen.add(neighbor); stack.push(neighbor); }
    }
    weak_components.push(members.sort((a, b) => a - b).map(i => brief.nodes[i].id));
  }
  const ids = (predicate: (i: number) => boolean) => brief.nodes.filter((_, i) => predicate(i)).map(node => node.id);
  return { scope: "declared_graph_only", verification: "not_verified", weak_components,
    source_node_ids: ids(i => incoming[i] === 0 && outgoing[i] > 0),
    sink_node_ids: ids(i => incoming[i] > 0 && outgoing[i] === 0),
    isolated_node_ids: ids(i => neighbors[i].length === 0), invalid_edge_indices };
}
