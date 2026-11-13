type Row = Record<string, unknown>;
const row = (v: unknown): Row => v && typeof v === "object" && !Array.isArray(v) ? v as Row : {};
const list = (v: unknown): Row[] => Array.isArray(v) ? v.map(row) : [];
const canonical = (value: unknown): string => {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(row(value)[key])}`).join(",")}}`;
  return JSON.stringify(value) ?? "undefined";
};
const normalize = (v: string) => v.replace(/\s/gu, "");
export type PlanLabelReview = { key: string; expected: string; actual: string; original?: string; id?: string; status: "same" | "changed" | "missing" | "ambiguous" };
function historicalPlan(document: unknown) {
  const elements = list(row(document).elements).filter(e => !e.isDeleted);
  const snapshots = elements.map(e => row(e.customData).planningSource).filter(v => v !== undefined);
  if (!snapshots.length) return { reason: "候选没有附带确认计划快照。" };
  const snapshotKey = canonical(snapshots[0]);
  if (snapshots.some(v => canonical(v) !== snapshotKey)) return { reason: "候选包含不一致的计划快照，无法确定对照来源。" };
  const source = row(snapshots[0]), plan = row(source.document);
  if (!Number.isSafeInteger(source.version) || !Array.isArray(plan.nodes) || !Array.isArray(plan.edges)) return { reason: "计划快照格式不完整。" };
  return { elements, source, plan };
}
/** Compare historical embedded plan labels, never infer a current scientific plan. */
export function planLabelReview(document: unknown): { available: boolean; reason?: string; version?: number; items: PlanLabelReview[]; omitted: number } {
  const snapshot = historicalPlan(document);
  const unavailable = (reason: string) => ({ available: false, reason, items: [], omitted: 0 });
  if (snapshot.reason !== undefined) return unavailable(snapshot.reason);
  const { elements, source, plan } = snapshot;
  const nodes = list(plan.nodes), edges = list(plan.edges);
  if (nodes.some(n => typeof n.id !== "string" || typeof n.label !== "string") || edges.some(e => typeof e.from !== "string" || typeof e.to !== "string" || typeof e.label !== "string")) return unavailable("计划标签格式不完整。");
  const anchors = (id: unknown) => elements.filter(e => row(e.customData).scientificId === id);
  const items: PlanLabelReview[] = [];
  function inspect(key: string, expected: string, matches: Row[]) {
    if (items.length >= 200) return;
    const anchor = matches.length === 1 ? matches[0] : undefined;
    const labelIds = anchor && row(anchor.customData).visualLabelIds;
    const labels = anchor ? elements.filter(e => e.type === "text" && (e.containerId === anchor.id || (Array.isArray(labelIds) && labelIds.includes(e.id)))) : [];
    const actual = labels.length === 1 ? String(labels[0].text ?? labels[0].originalText ?? "") : "";
    const original = labels.length === 1 && typeof labels[0].originalText === "string" ? labels[0].originalText : actual;
    const status = matches.length > 1 || labels.length > 1 ? "ambiguous" : !anchor || !labels.length ? "missing" : (normalize(expected) === normalize(actual) && normalize(expected) === normalize(original)) ? "same" : "changed";
    items.push({ key, expected: expected.slice(0, 2000), actual: actual.slice(0, 2000), ...(normalize(original) !== normalize(actual) ? {original: original.slice(0, 2000)} : {}), id: typeof anchor?.id === "string" ? anchor.id : undefined, status });
  }
  nodes.slice(0, 200).forEach((n, i) => inspect(`节点 ${i + 1}`, n.label as string, anchors(n.id)));
  edges.filter(e => e.label !== "").slice(0, 200 - items.length).forEach((e, i) => {
    const from = anchors(e.from), to = anchors(e.to);
    inspect(`关系 ${i + 1}`, e.label as string, from.length === 1 && to.length === 1 ? elements.filter(a => a.type === "arrow" && row(a.startBinding).elementId === from[0].id && row(a.endBinding).elementId === to[0].id) : []);
  });
  return { available: true, version: source.version as number, items, omitted: Math.max(0, nodes.length + edges.filter(e => e.label !== "").length - items.length) };
}

// Snapshot differences describe editing history, not scientific authority.
const historicalComparisonScope = {
  comparison_basis: "embedded_historical_plan_snapshot",
  expected_field_meaning: "historical_snapshot_value_not_a_required_correction",
  scientific_correctness_assessed: false,
  restoration_authorized: false,
  guidance: "差异只说明当前图稿与历史计划快照不同，不判断哪一版科学上正确。当前文字或关系可能已经审阅修订；局部排版任务保留当前内容。不得仅因差异恢复旧表述、推荐补回旧位点或改写计划；科学修订须另有用户授权与来源依据。",
} as const;

/** Compact feedback independently limits differing labels and marks incomplete text. */
export function planLabelFeedback(document: unknown) {
  const review = planLabelReview(document);
  const differences = review.items.filter(item => item.status !== "same");
  return { ...review, ...historicalComparisonScope, checked: review.items.length, differences: differences.length,
    feedback_omitted: Math.max(0, differences.length - 20),
    items: differences.slice(0, 20).map(item => ({ ...item,
      text_truncated: item.expected.length > 600 || item.actual.length > 600 || (item.original?.length ?? 0) > 600,
      expected: item.expected.slice(0, 600), actual: item.actual.slice(0, 600), ...(item.original === undefined ? {} : {original: item.original.slice(0, 600)}) })) };
}

export type RelationState = { direction: string | null; inhibitory: boolean | null; line_style: string | null };
export type PlanRelationItem = { key: string; id?: string; expected: RelationState; actual: RelationState | null; metadata_mismatch: boolean; status: "same" | "changed" | "missing" | "ambiguous" | "unsupported" };
/** Compare explicit native endpoints/styles with the immutable historical plan. */
export function planRelationReview(document: unknown): { available: boolean; reason?: string; version?: number; items: PlanRelationItem[]; omitted: number } {
  const snapshot = historicalPlan(document), unavailable = (reason: string) => ({ available: false, reason, items: [], omitted: 0 });
  if (snapshot.reason !== undefined) return unavailable(snapshot.reason);
  const { elements, source, plan } = snapshot, nodes = list(plan.nodes), edges = list(plan.edges);
  if (nodes.some(node => typeof node.id !== "string" || !node.id.trim()) || edges.some(edge => typeof edge.from !== "string" || typeof edge.to !== "string" || !nodes.some(node => node.id === edge.from) || !nodes.some(node => node.id === edge.to) || ![undefined, "forward", "bidirectional", "undirected"].includes(edge.direction as string | undefined) || (edge.inhibitory !== undefined && typeof edge.inhibitory !== "boolean") || ![undefined, "solid", "dashed", "dotted"].includes(edge.line_style as string | undefined) || (edge.inhibitory === true && edge.direction !== undefined && edge.direction !== "forward"))) return unavailable("计划关系格式不完整或方向与抑制声明冲突。");
  const anchors = (id: unknown) => elements.filter(element => row(element.customData).scientificId === id);
  const items = edges.slice(0, 200).map((edge, index): PlanRelationItem => {
    const expected: RelationState = { direction: edge.direction as string ?? "forward", inhibitory: edge.inhibitory as boolean ?? false, line_style: edge.line_style as string ?? "solid" };
    const from = anchors(edge.from), to = anchors(edge.to), parallel = edges.filter(other => other.from === edge.from && other.to === edge.to).length > 1;
    const matches = from.length === 1 && to.length === 1 ? elements.filter(element => element.type === "arrow" && row(element.startBinding).elementId === from[0].id && row(element.endBinding).elementId === to[0].id && row(element.customData).annotationKind == null && row(element.customData).inset == null) : [];
    const base = { key: `关系 ${index + 1}`, expected, actual: null, metadata_mismatch: false };
    if (parallel || nodes.filter(node => node.id === edge.from).length > 1 || nodes.filter(node => node.id === edge.to).length > 1 || from.length > 1 || to.length > 1 || matches.length > 1) return { ...base, status: "ambiguous" };
    if (matches.length !== 1) return { ...base, status: "missing" };
    const arrow = matches[0], start = arrow.startArrowhead ?? null, end = arrow.endArrowhead ?? null;
    const direction = start === null && (end === "arrow" || end === "bar") ? "forward" : start === "arrow" && end === "arrow" ? "bidirectional" : start === null && end === null ? "undirected" : null;
    const actual: RelationState = { direction, inhibitory: direction === null ? null : end === "bar", line_style: ["solid", "dashed", "dotted"].includes(arrow.strokeStyle as string) ? arrow.strokeStyle as string : null };
    const data = row(arrow.customData), mismatch = direction !== null && ((data.direction !== undefined && data.direction !== direction) || (data.inhibitory !== undefined && data.inhibitory !== actual.inhibitory));
    return { ...base, id: typeof arrow.id === "string" ? arrow.id : undefined, actual, metadata_mismatch: mismatch, status: direction === null || actual.line_style === null ? "unsupported" : mismatch || canonical(expected) !== canonical(actual) ? "changed" : "same" };
  });
  return { available: true, version: source.version as number, items, omitted: Math.max(0, edges.length - items.length) };
}
export function planRelationFeedback(document: unknown) {
  const review = planRelationReview(document), differences = review.items.filter(item => item.status !== "same");
  return { ...review, ...historicalComparisonScope, checked: review.items.length, differences: differences.length, feedback_omitted: Math.max(0, differences.length - 20), items: differences.slice(0, 20) };
}
