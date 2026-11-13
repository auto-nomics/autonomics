import { getCommonBounds } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";

/** Insert into visible whitespace; preserve every existing object's geometry. */
export function imagePlacement(api: ExcalidrawImperativeAPI, width: number, height: number) {
  const state = api.getAppState(), zoom = state.zoom.value;
  const viewport = { left: -state.scrollX, top: -state.scrollY, right: state.width / zoom - state.scrollX, bottom: state.height / zoom - state.scrollY };
  const center = [(viewport.left + viewport.right - width) / 2, (viewport.top + viewport.bottom - height) / 2];
  const obstacles = api.getSceneElements().filter(element => !element.isDeleted).map(element => getCommonBounds([element]));
  const step = Math.max(64, Math.min(width, height) / 2 + 24);
  const candidates: { x: number; y: number; distance: number }[] = [];
  for (let dx = -6; dx <= 6; dx++) for (let dy = -6; dy <= 6; dy++) candidates.push({ x: center[0] + dx * step, y: center[1] + dy * step, distance: dx * dx + dy * dy });
  candidates.sort((a, b) => a.distance - b.distance);
  for (const point of candidates) {
    if (point.x < viewport.left + 24 || point.y < viewport.top + 48 || point.x + width > viewport.right - 24 || point.y + height > viewport.bottom - 24) continue;
    if (obstacles.every(([left, top, right, bottom]) => point.x + width + 20 <= left || point.x - 20 >= right || point.y + height + 20 <= top || point.y - 20 >= bottom)) return { x: point.x, y: point.y, pan: false };
  }
  return { x: Math.max(viewport.left, ...obstacles.map(bounds => bounds[2])) + 40, y: center[1], pan: true };
}
