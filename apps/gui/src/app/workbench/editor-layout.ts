/** A binary grid. Leaves identify groups; branches carry a persisted split ratio. */
export type EditorLayout = { group: string } | { axis: "horizontal" | "vertical"; ratio: number; first: EditorLayout; second: EditorLayout };
export type DropDirection = "left" | "right" | "top" | "bottom";
export function layoutGroups(layout: EditorLayout): string[] {
  return "group" in layout ? [layout.group] : [...layoutGroups(layout.first), ...layoutGroups(layout.second)];
}
export function splitLayout(layout: EditorLayout, target: string, added: string, direction: DropDirection): EditorLayout {
  if ("group" in layout) {
    if (layout.group !== target) return layout;
    const before = direction === "left" || direction === "top";
    return { axis: direction === "left" || direction === "right" ? "horizontal" : "vertical", ratio: 50, first: before ? { group: added } : layout, second: before ? layout : { group: added } };
  }
  return { ...layout, first: splitLayout(layout.first, target, added, direction), second: splitLayout(layout.second, target, added, direction) };
}
export function removeLayoutGroup(layout: EditorLayout, id: string): EditorLayout | null {
  if ("group" in layout) return layout.group === id ? null : layout;
  const first = removeLayoutGroup(layout.first, id), second = removeLayoutGroup(layout.second, id);
  return first && second ? { ...layout, first, second } : first ?? second;
}
export function resizeLayout(layout: EditorLayout, path: string, ratio: number): EditorLayout {
  if ("group" in layout) return layout;
  if (!path) return { ...layout, ratio: Math.max(15, Math.min(85, ratio)) };
  return path[0] === "0" ? { ...layout, first: resizeLayout(layout.first, path.slice(1), ratio) } : { ...layout, second: resizeLayout(layout.second, path.slice(1), ratio) };
}
export function validLayout(value: unknown, ids: Set<string>): value is EditorLayout {
  const seen = new Set<string>();
  const walk = (node: unknown, depth: number): boolean => {
    if (!node || typeof node !== "object" || depth > 40) return false;
    const item = node as Record<string, unknown>;
    if (typeof item.group === "string") { if (!ids.has(item.group) || seen.has(item.group)) return false; seen.add(item.group); return true; }
    return (item.axis === "horizontal" || item.axis === "vertical") && typeof item.ratio === "number" && item.ratio >= 15 && item.ratio <= 85 && walk(item.first, depth + 1) && walk(item.second, depth + 1);
  };
  return walk(value, 0) && seen.size === ids.size;
}
