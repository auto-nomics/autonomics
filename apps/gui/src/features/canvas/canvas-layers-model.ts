import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";

export type CanvasLayer = { id: string; name: string; ids: string[]; hidden: boolean; locked: boolean };
export type LayerMetadata = { name?: string; elementName?: string; hidden?: boolean; opacityBeforeHide?: number; lockedBeforeHide?: boolean };
export const layerMetadata = (element: ExcalidrawElement): LayerMetadata => {
  const value = element.customData?.canvasLayer;
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
};
export const isLayerHidden = (element: ExcalidrawElement) => layerMetadata(element).hidden === true;

const elementTypes: Record<string, string> = { rectangle: "矩形", ellipse: "椭圆", diamond: "菱形", arrow: "箭头", line: "线条", freedraw: "手绘", image: "图片", text: "文字", frame: "画框", embeddable: "嵌入内容" };
export const canvasElementType = (element: ExcalidrawElement) => elementTypes[element.type] || "对象";
export function canvasElementName(element: ExcalidrawElement): string {
  const name = layerMetadata(element).elementName;
  if (typeof name === "string" && name.trim()) return name;
  return String((element.type === "text" ? element.originalText || element.text : element.type === "frame" ? element.name : null) || element.customData?.asset?.title || canvasElementType(element));
}

/** Ownership joins frames, native groups, bound labels and scientific illustrations,
 * never connectors merely attached to a node. */
export function canvasLayers(elements: readonly ExcalidrawElement[]): CanvasLayer[] {
  const live = elements.filter(element => !element.isDeleted);
  const parent = new Map(live.map(element => [element.id, element.id]));
  const root = (id: string): string => {
    const next = parent.get(id)!;
    if (next === id) return id;
    const resolved = root(next); parent.set(id, resolved); return resolved;
  };
  const join = (a: string, b: string | null | undefined) => {
    if (b && parent.has(b)) parent.set(root(a), root(b));
  };
  const groups = new Map<string, string>();
  for (const element of live) {
    for (const group of element.groupIds) {
      const owner = groups.get(group);
      if (owner) join(element.id, owner); else groups.set(group, element.id);
    }
    join(element.id, element.frameId);
    if (element.type === "text") join(element.id, element.containerId);
    join(element.id, element.customData?.semanticAnchorId);
    join(element.id, element.customData?.visualAssetElementId);
    if (element.customData?.inset?.kind === "image_inset_snapshot") join(element.id, element.customData.inset.anchor_id);
    for (const id of element.customData?.visualLabelIds ?? []) join(element.id, id);
  }
  const members = new Map<string, ExcalidrawElement[]>();
  for (const element of live) {
    const key = root(element.id);
    const list = members.get(key) ?? []; list.push(element); members.set(key, list);
  }

  const positions = new Map(live.map((element, index) => [element.id, index]));
  const units = [...members.values()].map(items => ({ items, top: items.reduce((top, element) => Math.max(top, positions.get(element.id)!), -1) }));
  return units.sort((a, b) => b.top - a.top).map(({ items }) => {
    const representative = items.find(element => element.type === "frame") ?? items.find(element => element.customData?.scientificId || element.customData?.visualAssetElementId) ?? items.find(element => element.type !== "text") ?? items[0];
    const text = items.find(element => element.type === "text");
    const name = layerMetadata(representative).name || (items.length === 1 ? layerMetadata(representative).elementName : null) || (representative.type === "frame" ? representative.name : null) ||
      (text?.type === "text" ? text.originalText || text.text : null) || representative.customData?.asset?.title || canvasElementType(representative);
    return { id: representative.id, name: String(name), ids: items.map(element => element.id), hidden: items.every(isLayerHidden), locked: items.some(element => isLayerHidden(element) ? layerMetadata(element).lockedBeforeHide : element.locked) };
  });
}

export function layerVisibility(element: ExcalidrawElement, hidden: boolean) {
  const metadata = { ...layerMetadata(element) };
  if (hidden && !metadata.hidden) {
    metadata.opacityBeforeHide = element.opacity; metadata.lockedBeforeHide = element.locked;
  }
  const restoredOpacity = typeof metadata.opacityBeforeHide === "number" && Number.isFinite(metadata.opacityBeforeHide) ? Math.max(0, Math.min(100, metadata.opacityBeforeHide)) : 100;
  const opacity = hidden ? 0 : metadata.hidden ? restoredOpacity : element.opacity;
  const locked = hidden ? true : metadata.hidden ? metadata.lockedBeforeHide ?? false : element.locked;
  if (hidden) metadata.hidden = true;
  else { delete metadata.hidden; delete metadata.opacityBeforeHide; delete metadata.lockedBeforeHide; }
  return { opacity, locked, customData: { ...element.customData, canvasLayer: metadata } };
}
export function layerLock(element: ExcalidrawElement, locked: boolean) {
  return isLayerHidden(element)
    ? { customData: { ...element.customData, canvasLayer: { ...layerMetadata(element), lockedBeforeHide: locked } } }
    : { locked };
}
