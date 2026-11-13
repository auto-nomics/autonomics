type Row = Record<string, unknown>;
type Point = [number, number];
const row = (value: unknown): Row => value && typeof value === 'object' && !Array.isArray(value) ? value as Row : {};
const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);

function crosses(a: Point, b: Point, label: Row): boolean | null {
  const { x, y, width, height } = label, angle = label.angle ?? 0;
  if (![x, y, width, height, angle].every(finite) || Number(width) <= 0 || Number(height) <= 0 || label.opacity === 0) return null;
  const cx = Number(x) + Number(width) / 2, cy = Number(y) + Number(height) / 2;
  const c = Math.cos(Number(angle)), s = Math.sin(Number(angle));
  const local = ([px, py]: Point): Point => [c * (px - cx) + s * (py - cy), -s * (px - cx) + c * (py - cy)];
  const start = local(a), end = local(b), half = [Number(width) / 2, Number(height) / 2];
  if (Math.hypot(end[0] - start[0], end[1] - start[1]) < 1e-6) return false;
  let enter = 0, leave = 1;
  for (let axis = 0; axis < 2; axis++) {
    const low = -half[axis] + 1e-6, high = half[axis] - 1e-6, delta = end[axis] - start[axis];
    if (low >= high) return false;
    if (Math.abs(delta) < 1e-12) { if (start[axis] <= low || start[axis] >= high) return false; }
    else {
      const t1 = (low - start[axis]) / delta, t2 = (high - start[axis]) / delta;
      enter = Math.max(enter, Math.min(t1, t2)); leave = Math.min(leave, Math.max(t1, t2));
      if (leave <= enter) return false;
    }
  }
  return leave > enter;
}

/** Centerline intersections with owned label rectangles; no glyph or stroke collision claim. */
export function illustrationConnectorLabelOverlaps(document: unknown) {
  const elements = (Array.isArray(row(document).elements) ? row(document).elements as unknown[] : []).map(row).filter(e => e.isDeleted !== true);
  const index = new Map(elements.map(e => [e.id, e]));
  const pairs: { anchor_id: string; arrow_id: string; label_id: string }[] = [];
  let checked = 0, skipped = 0, omitted = 0;
  for (const anchor of elements) {
    const data = row(anchor.customData), image = index.get(data.visualAssetElementId);
    if (typeof anchor.id !== 'string' || !image || image.type !== 'image' || row(image.customData).semanticAnchorId !== anchor.id || !Array.isArray(data.visualLabelIds)) continue;
    const arrows = elements.filter(e => e.type === 'arrow' && (row(e.startBinding).elementId === anchor.id || row(e.endBinding).elementId === anchor.id));
    for (const labelId of new Set(data.visualLabelIds)) {
      const label = index.get(labelId); if (!label || label.type !== 'text' || label.containerId) continue;
      for (const arrow of arrows) {
        if (checked + skipped >= 200) { omitted++; continue; }
        const raw = arrow.points;
        if (!finite(arrow.x) || !finite(arrow.y) || (arrow.angle ?? 0) !== 0 || arrow.opacity === 0 || arrow.elbowed === true || arrow.roundness != null || (arrow.roughness ?? 0) !== 0 || !Array.isArray(raw) || raw.length < 2 || raw.length > 1000 || !raw.every(p => Array.isArray(p) && p.length === 2 && p.every(finite))) { skipped++; continue; }
        const points = (raw as Point[]).map(([x, y]): Point => [x + Number(arrow.x), y + Number(arrow.y)]);
        const intersections = points.slice(1).map((point, i) => crosses(points[i], point, label));
        if (intersections.some(value => value === null)) { skipped++; continue; }
        checked++;
        if (intersections.some(Boolean)) pairs.push({ anchor_id: anchor.id, arrow_id: String(arrow.id), label_id: String(labelId) });
      }
    }
  }
  return { method: 'unrotated_sharp_connector_centerlines_and_rotated_owned_label_rectangles', checked, skipped, omitted, count: pairs.length, pairs };
}
