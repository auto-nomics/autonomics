type Row = Record<string, unknown>;
const row = (v: unknown): Row => v && typeof v === "object" && !Array.isArray(v) ? v as Row : {};
/** Resolve only the explicitly focused image or a reciprocal semantic ownership. */
export function assetReviewTarget(document: unknown, id: string) {
  const scene = row(document), elements = (Array.isArray(scene.elements) ? scene.elements : []).map(row).filter(e => e.isDeleted !== true);
  const target = elements.find(e => e.id === id);
  if (!target) return { status: "missing_object" as const, image: null, files: row(scene.files) };
  if (target.type === "image") return { status: "image" as const, image: target, files: row(scene.files) };
  const visual = row(target.customData).visualAssetElementId;
  if (visual == null) return { status: "no_image" as const, image: null, files: row(scene.files) };
  const image = elements.find(e => e.id === visual && e.type === "image" && row(e.customData).semanticAnchorId === id);
  return { status: image ? "image" as const : "unverified_reference" as const, image: image ?? null, files: row(scene.files) };
}
export function assetGeometry(image: Row) {
  const number = (value: unknown) => typeof value === "number" && Number.isFinite(value) ? value : null;
  const position = [image.x, image.y, image.width, image.height].map(number);
  const angle = number(image.angle), scale = image.scale;
  const crop = row(image.crop), values = [crop.x, crop.y, crop.width, crop.height, crop.naturalWidth, crop.naturalHeight].map(number);
  let fractions: number[] | null = null;
  if (values.every(v => v !== null)) {
    const [x, y, w, h, nw, nh] = values as number[];
    if (x >= 0 && y >= 0 && w > 0 && h > 0 && nw > 0 && nh > 0 && x + w <= nw + 1e-6 && y + h <= nh + 1e-6) fractions = [x / nw, y / nh, w / nw, h / nh];
  }
  return { position: position.every(v => v !== null) ? position as number[] : null, angle, scale: Array.isArray(scale) && scale.length === 2 && scale.every(v => v === 1 || v === -1) ? scale as number[] : null, crop: image.crop == null ? "uncropped" as const : fractions ? "normalized_region" as const : "unsupported" as const, fractions };
}
