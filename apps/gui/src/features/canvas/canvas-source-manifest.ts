/** Source inventory for the exact exported scene; never includes image bytes or user notes. */
type ObjectValue = Record<string, unknown>;
const object = (value: unknown): ObjectValue => value && typeof value === "object" && !Array.isArray(value) ? value as ObjectValue : {};
const string = (value: unknown): string | null => typeof value === "string" && value.trim() ? value : null;
const strings = (value: unknown): string[] => Array.isArray(value) ? value.map(string).filter((v): v is string => v !== null) : string(value) ? [string(value)!] : [];
const identifier = (value: unknown) => string(value) ?? (typeof value === "number" && Number.isFinite(value) ? String(value) : null);
const unique = (values: string[]) => [...new Set(values)];
function urls(...values: unknown[]) {
  return unique(values.flatMap(strings).filter(value => { try { const url = new URL(value); return ["http:", "https:"].includes(url.protocol) && !url.username && !url.password; } catch { return false; } }));
}
export async function sha256(bytes: Uint8Array) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", Uint8Array.from(bytes).buffer))].map(v => v.toString(16).padStart(2, "0")).join("");
}
async function fileHash(file: ObjectValue) {
  const dataURL = string(file.dataURL); if (!dataURL) return { actual_sha256: null, file_status: "missing" as const };
  try {
    const match = /^data:([^,]*),([\s\S]*)$/.exec(dataURL); if (!match) throw new Error("invalid data URL");
    const bytes = /;base64(?:;|$)/i.test(match[1]) ? Uint8Array.from(atob(match[2]), character => character.charCodeAt(0)) : new TextEncoder().encode(decodeURIComponent(match[2]));
    return { actual_sha256: await sha256(bytes), file_status: "available" as const };
  } catch { return { actual_sha256: null, file_status: "invalid_encoding" as const }; }
}
function insetChange(row: ObjectValue) {
  if (!["inset", "inset_refresh"].includes(String(row.kind))) return {};
  const rectangle = (value: unknown, keys: string[]) => {
    const rect = object(value);
    return Object.fromEntries(keys.map(key => [key, typeof rect[key] === "number" && Number.isFinite(rect[key]) ? rect[key] : null]));
  };
  return { ...(row.kind === "inset_refresh" ? { previous_source_revision: typeof row.previous_source_revision === "number" && Number.isSafeInteger(row.previous_source_revision) && row.previous_source_revision > 0 ? row.previous_source_revision : null, previous_file_id: string(row.previous_file_id), previous_sha256: string(row.previous_sha256) } : {}), source_element_id: string(row.source_element_id), source_revision: typeof row.source_revision === "number" && Number.isSafeInteger(row.source_revision) && row.source_revision > 0 ? row.source_revision : null, region: rectangle(row.region, ["x", "y", "width", "height"]), crop: rectangle(row.crop, ["x", "y", "width", "height", "naturalWidth", "naturalHeight"]) };
}
function changes(value: unknown) {
  return (Array.isArray(value) ? value : []).map(entry => {
    const row = object(entry);
    return { kind: string(row.kind), ...insetChange(row), ...(["path_style", "path_edit"].includes(String(row.kind)) ? { edits: (Array.isArray(row.edits) ? row.edits : []).map(value => { const edit = object(value); const paint = (v: unknown) => typeof v === "string" && (v === "none" || /^#[a-f\d]{6}$/i.test(v)) ? v : null; return { part_id: string(edit.part_id), ...(Array.isArray(edit.points) ? { points: edit.points.map(value => { const point = object(value); return { index: typeof point.index === "number" && Number.isSafeInteger(point.index) && point.index >= 0 ? point.index : null, position: Array.isArray(point.position) && point.position.length === 2 && point.position.every(v => typeof v === "number" && Number.isFinite(v) && Math.abs(v) <= 1e6) ? point.position as number[] : null }; }) } : {}), ...(Array.isArray(edit.translate) && edit.translate.length === 2 && edit.translate.every(v => typeof v === "number" && Number.isFinite(v) && Math.abs(v) <= 100000) ? { translate: edit.translate as number[] } : {}), fill: paint(edit.fill), stroke: paint(edit.stroke), opacity: typeof edit.opacity === "number" && Number.isFinite(edit.opacity) && edit.opacity >= 0 && edit.opacity <= 1 ? edit.opacity : null }; }) } : {}), colors: Object.fromEntries(Object.entries(object(row.colors)).filter(([from, to]) => /^#[a-f\d]{6}$/i.test(from) && typeof to === "string" && /^#[a-f\d]{6}$/i.test(to))), based_on_file_id: string(row.based_on_file_id), based_on_sha256: string(row.based_on_sha256), result_sha256: string(row.result_sha256), created_at: string(row.created_at) };
  });
}
export async function createSourceManifest(input: { scene: string; title: string; project_id?: string; figure_id?: string; based_on_revision?: number; unsaved_changes: boolean }) {
  const document = object(JSON.parse(input.scene)), files = object(document.files);
  const elements = (Array.isArray(document.elements) ? document.elements : []).map(object).filter(element => element.type === "image" && element.isDeleted !== true);
  const hashes = new Map<string, ReturnType<typeof fileHash>>();
  const assets = await Promise.all(elements.map(async element => {
    const data = object(element.customData), asset = object(data.asset), fileId = string(element.fileId), file = object(fileId ? files[fileId] : undefined);
    const key = fileId ?? `missing:${string(element.id)}`; if (!hashes.has(key)) hashes.set(key, fileHash(file));
    const hash = await hashes.get(key)!;
    const authors = unique([...strings(asset.creators), ...strings(asset.author), ...strings(data.creators), ...strings(data.author)]), licenses = unique([...strings(asset.licenses), ...strings(asset.license), ...strings(data.licenses), ...strings(data.license)]);
    const sourceURLs = urls(asset.source_url, asset.svg_url, data.source_url, data.source, object(data.file).url);
    const normalization = strings(data.normalization), modifications = changes(data.modifications);
    const name = string(asset.title) ?? string(asset.name) ?? string(element.id) ?? "Image";
    const recordedHash = string(data.sha256);
    const missing = [...(!authors.length ? ["authors"] : []), ...(!licenses.length ? ["licenses"] : []), ...(!sourceURLs.length ? ["source_urls"] : [])];
    return { element_id: string(element.id), semantic_anchor_id: string(data.semanticAnchorId), mime_type: string(file.mimeType), file_id: fileId, provider: string(data.source), asset_id: identifier(asset.id), name, authors, author_urls: urls(asset.author_url), licenses, license_urls: urls(asset.license_url, asset.license_urls, data.license_url, data.license_urls), source_urls: sourceURLs, source_sha256: string(data.source_sha256), recorded_sha256: recordedHash, ...hash, hash_matches_recorded: hash.actual_sha256 && recordedHash && /^[a-f\d]{64}$/i.test(recordedHash) ? hash.actual_sha256 === recordedHash.toLowerCase() : null, normalization, modifications, missing_fields: missing };
  }));
  const credits = unique(assets.map(asset => `${asset.name} — ${asset.authors.length ? asset.authors.join(", ") : "author not recorded"}; ${asset.licenses.length ? asset.licenses.join(", ") : "license not recorded"}${asset.license_urls.length ? ` (${asset.license_urls.join(", ")})` : ""}; ${asset.source_urls.join(", ") || "source not recorded"}${asset.normalization.length || asset.modifications.length ? `; changes: ${[...asset.normalization, ...asset.modifications.map(change => change.kind ?? "unspecified")].join("; ")}` : ""}`));
  return { schema: "autonomics.figure.asset-sources/v1", exported_at: new Date().toISOString(), title: input.title, project_id: input.project_id ?? null, figure_id: input.figure_id ?? null, based_on_revision: input.based_on_revision ?? null, unsaved_changes: input.unsaved_changes, scene_json_sha256: await sha256(new TextEncoder().encode(input.scene)), assets, credits };
}
export type SourceManifest = Awaited<ReturnType<typeof createSourceManifest>>;
export function embedSvgSources(svg: SVGSVGElement, manifest: SourceManifest) {
  const metadata = document.createElementNS("http://www.w3.org/2000/svg", "metadata"); metadata.id = "autonomics-asset-sources";
  metadata.textContent = JSON.stringify(manifest); svg.insertBefore(metadata, svg.firstChild);
}
// PNG international-text chunk: uncompressed UTF-8 keeps names and licence text intact.
const keyword = "autonomics.asset-sources";
function crc32(bytes: Uint8Array) {
  let crc = 0xffffffff;
  for (const byte of bytes) { crc ^= byte; for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0); }
  return (crc ^ 0xffffffff) >>> 0;
}
export async function embedPngSources(blob: Blob, manifest: SourceManifest) {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  if (bytes.length < 20 || [137, 80, 78, 71, 13, 10, 26, 10].some((v, i) => bytes[i] !== v)) throw new Error("无效的 PNG 导出");
  const data = new TextEncoder().encode(`${keyword}\0\0\0\0\0${JSON.stringify(manifest)}`), type = new TextEncoder().encode("iTXt");
  const chunk = new Uint8Array(data.length + 12), view = new DataView(chunk.buffer); view.setUint32(0, data.length); chunk.set(type, 4); chunk.set(data, 8); view.setUint32(data.length + 8, crc32(chunk.subarray(4, data.length + 8)));
  const parts: Uint8Array[] = [bytes.slice(0, 8)]; let offset = 8, ended = false;
  while (offset + 12 <= bytes.length) {
    const length = new DataView(bytes.buffer, bytes.byteOffset + offset, 4).getUint32(0), end = offset + 12 + length;
    if (end > bytes.length) throw new Error("无效的 PNG 数据块");
    const name = new TextDecoder().decode(bytes.subarray(offset + 4, offset + 8));
    if (name === "IEND") { parts.push(chunk, bytes.slice(offset, end)); ended = true; break; }
    const old = name === "iTXt" && length >= keyword.length + 1 && new TextDecoder().decode(bytes.subarray(offset + 8, offset + 8 + keyword.length + 1)) === `${keyword}\0`;
    if (!old) parts.push(bytes.slice(offset, end)); offset = end;
  }
  if (!ended) throw new Error("PNG 缺少结束数据块");
  return new Blob(parts.map(part => Uint8Array.from(part).buffer), { type: "image/png" });
}
